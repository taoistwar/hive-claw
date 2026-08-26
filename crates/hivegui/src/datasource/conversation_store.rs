//! Encrypted local conversation persistence for US13/T127.
//!
//! The canonical [`ConversationStore::from_store`] path uses only the v4
//! tables owned by migrations. Protected title, message, Tool payload, and
//! execution state values are encrypted before any SQLite write. The legacy
//! [`ConversationStore::new`] constructor remains an in-memory fixture seam
//! for the early crypto contract tests and never creates application tables.

#![warn(missing_docs)]

use std::{collections::BTreeSet, sync::Arc};

use chrono::{DateTime, Datelike, Duration, Utc};
use parking_lot::Mutex;
use serde_json::Value;
use sqlx::{Pool, Sqlite};
use thiserror::Error;
use uuid::Uuid;

use crate::datasource::{
    crypto::Crypto,
    query_count::QueryCountObserver,
    query_plan::{
        CONVERSATION_BUNDLE_EXECUTIONS_SQL, CONVERSATION_BUNDLE_MESSAGES_SQL,
        CONVERSATION_EXPIRED_IDS_SQL, CONVERSATION_RECENT_SQL, CONVERSATION_RUNNING_EXECUTIONS_SQL,
    },
    store::Store,
};

/// Default retention is exactly one hundred calendar years. The numeric day
/// value remains part of the public configuration contract.
pub const DEFAULT_RETENTION_DAYS: i64 = 100 * 365;

/// Errors surfaced by the conversation persistence boundary.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum ConversationStoreError {
    /// Retention must be positive.
    #[error("invalid_retention")]
    InvalidRetention,
    /// Session identifiers use UUID wire format.
    #[error("invalid_session_id")]
    InvalidSessionId(String),
    /// Execution identifiers use UUID wire format.
    #[error("invalid_execution_id")]
    InvalidExecutionId(String),
    /// One public field failed validation.
    #[error("invalid_input:{field}:{reason}")]
    InvalidInput {
        /// Stable public field.
        field: &'static str,
        /// Stable non-sensitive reason.
        reason: &'static str,
    },
    /// Encryption or serialization failed without exposing input.
    #[error("crypto")]
    Crypto,
    /// SQLite failed without exposing SQL or stored values.
    #[error("database")]
    Database,
}

impl ConversationStoreError {
    /// Stable field for UI error placement.
    pub fn field(&self) -> &str {
        match self {
            Self::InvalidRetention => "retention_filter",
            Self::InvalidSessionId(_) => "session_id",
            Self::InvalidExecutionId(_) => "execution_id",
            Self::InvalidInput { field, .. } => field,
            Self::Crypto | Self::Database => "",
        }
    }

    /// Stable reason code.
    pub fn reason(&self) -> &str {
        match self {
            Self::InvalidRetention => "out_of_range",
            Self::InvalidSessionId(_) | Self::InvalidExecutionId(_) => "invalid_uuid",
            Self::InvalidInput { reason, .. } => reason,
            Self::Crypto => "crypto",
            Self::Database => "database",
        }
    }

    /// Stable top-level category.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::InvalidRetention
            | Self::InvalidSessionId(_)
            | Self::InvalidExecutionId(_)
            | Self::InvalidInput { .. } => "invalid_input",
            Self::Crypto => "crypto",
            Self::Database => "database",
        }
    }
}

/// Lifecycle state returned by the compatibility surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionLifecycle {
    /// Newly constructed fixture session.
    Open,
    /// Canonical persisted session.
    Active,
    /// Terminal session.
    Closed,
}

impl SessionLifecycle {
    /// Stable wire value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Active => "active",
            Self::Closed => "closed",
        }
    }
}

/// Persisted or fixture chat-session metadata.
#[derive(Debug, Clone)]
pub struct ChatSession {
    id: String,
    entry_agent_id: i64,
    current_agent_id: Option<i64>,
    title_encrypted: Vec<u8>,
    lifecycle: SessionLifecycle,
    created_at: String,
    updated_at: String,
    expires_at: String,
    retention_days: i64,
}

impl ChatSession {
    /// UUID session id.
    pub fn id(&self) -> &str {
        &self.id
    }
    /// Entry Agent id; fixture-only sessions use zero.
    pub fn entry_agent_id(&self) -> i64 {
        self.entry_agent_id
    }
    /// Current Agent id.
    pub fn current_agent_id(&self) -> Option<i64> {
        self.current_agent_id
    }
    /// Lifecycle.
    pub fn lifecycle(&self) -> SessionLifecycle {
        self.lifecycle
    }
    /// Created timestamp.
    pub fn created_at(&self) -> &str {
        &self.created_at
    }
    /// Updated timestamp.
    pub fn updated_at(&self) -> &str {
        &self.updated_at
    }
    /// Expiry timestamp.
    pub fn expires_at(&self) -> &str {
        &self.expires_at
    }
    /// Configured retention in days.
    pub fn retention_days(&self) -> i64 {
        self.retention_days
    }
    /// Encrypted title bytes.
    pub fn title_encrypted(&self) -> &[u8] {
        &self.title_encrypted
    }
}

/// One encrypted chat message.
#[derive(Debug, Clone)]
pub struct ChatMessage {
    id: String,
    session_id: String,
    content_encrypted: Vec<u8>,
    tool_calls_encrypted: Option<Vec<u8>>,
    created_at: String,
}

impl ChatMessage {
    /// Message id.
    pub fn id(&self) -> &str {
        &self.id
    }
    /// Owning session.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }
    /// Created timestamp.
    pub fn created_at(&self) -> &str {
        &self.created_at
    }
    /// Encrypted content.
    pub fn content_encrypted(&self) -> &[u8] {
        &self.content_encrypted
    }
    /// Encrypted Tool payload.
    pub fn tool_calls_encrypted(&self) -> Option<&[u8]> {
        self.tool_calls_encrypted.as_deref()
    }
}

/// Stable execution state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionState {
    /// Work is in progress.
    Running,
    /// Work completed.
    Completed,
    /// Work was cancelled.
    Cancelled,
    /// Work failed.
    Failed,
}

impl ExecutionState {
    /// Stable wire value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
        }
    }
}

/// One encrypted Agent execution record.
#[derive(Debug, Clone)]
pub struct AgentExecutionRecord {
    id: String,
    session_id: String,
    current_agent_id: Option<i64>,
    state_encrypted: Vec<u8>,
    started_at: String,
    ended_at: Option<String>,
}

impl AgentExecutionRecord {
    /// Execution UUID.
    pub fn id(&self) -> &str {
        &self.id
    }
    /// Owning session UUID.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }
    /// Current Agent.
    pub fn current_agent_id(&self) -> Option<i64> {
        self.current_agent_id
    }
    /// Started timestamp.
    pub fn started_at(&self) -> &str {
        &self.started_at
    }
    /// Terminal timestamp.
    pub fn ended_at(&self) -> Option<&str> {
        self.ended_at.as_deref()
    }
    /// Encrypted execution state.
    pub fn state_encrypted(&self) -> &[u8] {
        &self.state_encrypted
    }
}

/// Confirmation-bound expired-session preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetentionPreview {
    filter: String,
    session_ids: Vec<String>,
}

impl RetentionPreview {
    /// Number of rows that would be removed.
    pub fn affected_count(&self) -> usize {
        self.session_ids.len()
    }
    /// Canonical retention filter.
    pub fn filter(&self) -> &str {
        &self.filter
    }
}

/// Messages and executions loaded for one session in a fixed two-query batch.
#[derive(Debug, Clone)]
pub struct SessionBundle {
    session_id: String,
    messages: Vec<ChatMessage>,
    executions: Vec<AgentExecutionRecord>,
}

impl SessionBundle {
    /// Session UUID.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }
    /// Messages in sequence order.
    pub fn messages(&self) -> &[ChatMessage] {
        &self.messages
    }
    /// Executions in start order.
    pub fn executions(&self) -> &[AgentExecutionRecord] {
        &self.executions
    }
}

/// Conversation persistence handle.
#[derive(Debug, Clone)]
pub struct ConversationStore {
    crypto: Arc<Crypto>,
    pool: Pool<Sqlite>,
    retention_days: i64,
    canonical: bool,
    observer: Option<QueryCountObserver>,
    sessions: Arc<Mutex<Vec<ChatSession>>>,
    messages: Arc<Mutex<Vec<ChatMessage>>>,
    executions: Arc<Mutex<Vec<AgentExecutionRecord>>>,
}

impl ConversationStore {
    /// Construct the compatibility fixture boundary. This constructor never
    /// creates application tables; use [`Self::from_store`] for production.
    pub fn new(
        pool: Pool<Sqlite>,
        crypto: Arc<Crypto>,
        retention_days: Option<i64>,
    ) -> Result<Self, ConversationStoreError> {
        Self::build(pool, crypto, retention_days, false, None)
    }

    /// Construct the canonical v4 production boundary.
    pub fn from_store(
        store: &Store,
        retention_days: Option<i64>,
    ) -> Result<Self, ConversationStoreError> {
        Self::build(
            store.pool().clone(),
            Arc::new(store.crypto().clone()),
            retention_days,
            true,
            store.query_count_observer().cloned(),
        )
    }

    fn build(
        pool: Pool<Sqlite>,
        crypto: Arc<Crypto>,
        retention_days: Option<i64>,
        canonical: bool,
        observer: Option<QueryCountObserver>,
    ) -> Result<Self, ConversationStoreError> {
        let retention_days = retention_days.unwrap_or(DEFAULT_RETENTION_DAYS);
        if retention_days <= 0 {
            return Err(ConversationStoreError::InvalidRetention);
        }
        Ok(Self {
            crypto,
            pool,
            retention_days,
            canonical,
            observer,
            sessions: Arc::new(Mutex::new(Vec::new())),
            messages: Arc::new(Mutex::new(Vec::new())),
            executions: Arc::new(Mutex::new(Vec::new())),
        })
    }

    /// Configured retention days.
    pub fn retention_days(&self) -> i64 {
        self.retention_days
    }
    /// Underlying pool.
    pub fn pool(&self) -> &Pool<Sqlite> {
        &self.pool
    }
    /// Underlying AEAD handle.
    pub fn crypto(&self) -> &Crypto {
        &self.crypto
    }

    /// Create an encrypted fixture-only session without runtime DDL.
    pub async fn create_session(&self, title: &str) -> Result<ChatSession, ConversationStoreError> {
        let session = self.session_value(0, None, title, SessionLifecycle::Open, Utc::now())?;
        self.sessions.lock().push(session.clone());
        Ok(session)
    }

    /// Create a canonical encrypted session for one persisted Agent.
    pub async fn create_session_for_agent(
        &self,
        agent_id: i64,
        title: &str,
    ) -> Result<ChatSession, ConversationStoreError> {
        if !self.canonical || agent_id <= 0 {
            return Err(ConversationStoreError::InvalidInput {
                field: "entry_agent_id",
                reason: "not_found",
            });
        }
        let exists = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM agents WHERE id = ?")
            .bind(agent_id)
            .fetch_one(&self.pool)
            .await
            .map_err(|_| ConversationStoreError::Database)?;
        if exists != 1 {
            return Err(ConversationStoreError::InvalidInput {
                field: "entry_agent_id",
                reason: "not_found",
            });
        }
        let session = self.session_value(
            agent_id,
            Some(agent_id),
            title,
            SessionLifecycle::Active,
            Utc::now(),
        )?;
        sqlx::query(
            "INSERT INTO chat_sessions \
             (id, entry_agent_id, current_agent_id, title_encrypted, status, execution_id, created_at, updated_at, expires_at) \
             VALUES (?, ?, ?, ?, 'active', NULL, ?, ?, ?)",
        )
        .bind(&session.id)
        .bind(agent_id)
        .bind(agent_id)
        .bind(&session.title_encrypted)
        .bind(&session.created_at)
        .bind(&session.updated_at)
        .bind(&session.expires_at)
        .execute(&self.pool)
        .await
        .map_err(|_| ConversationStoreError::Database)?;
        self.sessions.lock().push(session.clone());
        Ok(session)
    }

    fn session_value(
        &self,
        entry_agent_id: i64,
        current_agent_id: Option<i64>,
        title: &str,
        lifecycle: SessionLifecycle,
        now: DateTime<Utc>,
    ) -> Result<ChatSession, ConversationStoreError> {
        let title_encrypted = self
            .crypto
            .encrypt(title.as_bytes())
            .map_err(|_| ConversationStoreError::Crypto)?;
        let expires_at = retention_expiry(now, self.retention_days)?;
        let now = now.to_rfc3339();
        Ok(ChatSession {
            id: Uuid::new_v4().to_string(),
            entry_agent_id,
            current_agent_id,
            title_encrypted,
            lifecycle,
            created_at: now.clone(),
            updated_at: now,
            expires_at: expires_at.to_rfc3339(),
            retention_days: self.retention_days,
        })
    }

    /// Append an encrypted user message and optional encrypted Tool payload.
    pub async fn append_message(
        &self,
        session_id: &str,
        content: &str,
        tool_calls: Option<&Value>,
    ) -> Result<ChatMessage, ConversationStoreError> {
        validate_session_id(session_id)?;
        let content_encrypted = self
            .crypto
            .encrypt(content.as_bytes())
            .map_err(|_| ConversationStoreError::Crypto)?;
        let tool_calls_encrypted = tool_calls
            .map(|value| {
                serde_json::to_vec(value)
                    .map_err(|_| ConversationStoreError::Crypto)
                    .and_then(|bytes| {
                        self.crypto
                            .encrypt(&bytes)
                            .map_err(|_| ConversationStoreError::Crypto)
                    })
            })
            .transpose()?;
        let created_at = Utc::now().to_rfc3339();
        let mut id = Uuid::new_v4().to_string();
        if self.canonical {
            let mut transaction = self
                .pool
                .begin()
                .await
                .map_err(|_| ConversationStoreError::Database)?;
            let sequence = sqlx::query_scalar::<_, i64>(
                "SELECT COALESCE(MAX(seq) + 1, 0) FROM chat_messages WHERE session_id = ?",
            )
            .bind(session_id)
            .fetch_one(&mut *transaction)
            .await
            .map_err(|_| ConversationStoreError::Database)?;
            let result = sqlx::query(
                "INSERT INTO chat_messages \
                 (session_id, seq, role, content_encrypted, tool_calls_encrypted, created_at) \
                 VALUES (?, ?, 'user', ?, ?, ?)",
            )
            .bind(session_id)
            .bind(sequence)
            .bind(&content_encrypted)
            .bind(&tool_calls_encrypted)
            .bind(&created_at)
            .execute(&mut *transaction)
            .await
            .map_err(|_| ConversationStoreError::Database)?;
            transaction
                .commit()
                .await
                .map_err(|_| ConversationStoreError::Database)?;
            id = result.last_insert_rowid().to_string();
        }
        let message = ChatMessage {
            id,
            session_id: session_id.to_string(),
            content_encrypted,
            tool_calls_encrypted,
            created_at,
        };
        self.messages.lock().push(message.clone());
        Ok(message)
    }

    /// Record encrypted execution state using the session's current Agent.
    pub async fn record_execution(
        &self,
        session_id: &str,
        state: ExecutionState,
    ) -> Result<AgentExecutionRecord, ConversationStoreError> {
        validate_session_id(session_id)?;
        let current_agent_id = if self.canonical {
            sqlx::query_scalar::<_, Option<i64>>(
                "SELECT current_agent_id FROM chat_sessions WHERE id = ?",
            )
            .bind(session_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| ConversationStoreError::Database)?
            .flatten()
            .ok_or_else(|| ConversationStoreError::InvalidSessionId(session_id.to_string()))?
        } else {
            0
        };
        self.record_execution_state(
            session_id,
            current_agent_id,
            state,
            &serde_json::json!({"status": state.as_str()}),
        )
        .await
    }

    /// Record an encrypted structured execution snapshot.
    pub async fn record_execution_state(
        &self,
        session_id: &str,
        current_agent_id: i64,
        state: ExecutionState,
        payload: &Value,
    ) -> Result<AgentExecutionRecord, ConversationStoreError> {
        validate_session_id(session_id)?;
        let state_bytes =
            serde_json::to_vec(payload).map_err(|_| ConversationStoreError::Crypto)?;
        let state_encrypted = self
            .crypto
            .encrypt(&state_bytes)
            .map_err(|_| ConversationStoreError::Crypto)?;
        let id = Uuid::new_v4().to_string();
        let started_at = Utc::now().to_rfc3339();
        let ended_at = (state != ExecutionState::Running).then(|| started_at.clone());
        if self.canonical {
            sqlx::query(
                "INSERT INTO agent_executions \
                 (execution_id, session_id, current_agent_id, status, state_encrypted, started_at, finished_at, error_kind) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, NULL)",
            )
            .bind(&id)
            .bind(session_id)
            .bind(current_agent_id)
            .bind(state.as_str())
            .bind(&state_encrypted)
            .bind(&started_at)
            .bind(&ended_at)
            .execute(&self.pool)
            .await
            .map_err(|_| ConversationStoreError::Database)?;
        }
        let record = AgentExecutionRecord {
            id,
            session_id: session_id.to_string(),
            current_agent_id: (current_agent_id > 0).then_some(current_agent_id),
            state_encrypted,
            started_at,
            ended_at,
        };
        self.executions.lock().push(record.clone());
        Ok(record)
    }

    /// Delete one session; v4 foreign keys cascade children.
    pub async fn delete_session(&self, session_id: &str) -> Result<u64, ConversationStoreError> {
        validate_session_id(session_id)?;
        let affected = if self.canonical {
            sqlx::query("DELETE FROM chat_sessions WHERE id = ?")
                .bind(session_id)
                .execute(&self.pool)
                .await
                .map_err(|_| ConversationStoreError::Database)?
                .rows_affected()
        } else {
            u64::from(
                self.sessions
                    .lock()
                    .iter()
                    .any(|session| session.id == session_id),
            )
        };
        self.sessions
            .lock()
            .retain(|session| session.id != session_id);
        self.messages
            .lock()
            .retain(|message| message.session_id != session_id);
        self.executions
            .lock()
            .retain(|execution| execution.session_id != session_id);
        Ok(affected)
    }

    /// Preview expired sessions, binding confirmation to the exact id set.
    pub async fn preview_retention(
        &self,
        retention_filter: &str,
    ) -> Result<RetentionPreview, ConversationStoreError> {
        if retention_filter != "expired" {
            return Err(ConversationStoreError::InvalidInput {
                field: "retention_filter",
                reason: "unsupported_filter",
            });
        }
        self.record_query("t127.chat_sessions.expired");
        let session_ids = sqlx::query_scalar::<_, String>(CONVERSATION_EXPIRED_IDS_SQL)
            .bind(Utc::now().to_rfc3339())
            .fetch_all(&self.pool)
            .await
            .map_err(|_| ConversationStoreError::Database)?;
        Ok(RetentionPreview {
            filter: retention_filter.to_string(),
            session_ids,
        })
    }

    /// Apply an exact retention preview transactionally.
    pub async fn apply_retention(
        &self,
        preview: &RetentionPreview,
    ) -> Result<u64, ConversationStoreError> {
        if preview.filter != "expired" {
            return Err(ConversationStoreError::InvalidInput {
                field: "retention_filter",
                reason: "unsupported_filter",
            });
        }
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|_| ConversationStoreError::Database)?;
        let now = Utc::now().to_rfc3339();
        let current = sqlx::query_scalar::<_, String>(CONVERSATION_EXPIRED_IDS_SQL)
            .bind(&now)
            .fetch_all(&mut *transaction)
            .await
            .map_err(|_| ConversationStoreError::Database)?;
        if current != preview.session_ids {
            return Err(ConversationStoreError::InvalidInput {
                field: "retention_filter",
                reason: "stale_preview",
            });
        }
        let result = sqlx::query("DELETE FROM chat_sessions WHERE expires_at <= ?")
            .bind(&now)
            .execute(&mut *transaction)
            .await
            .map_err(|_| ConversationStoreError::Database)?;
        transaction
            .commit()
            .await
            .map_err(|_| ConversationStoreError::Database)?;
        let removed = preview.session_ids.iter().cloned().collect::<BTreeSet<_>>();
        self.sessions
            .lock()
            .retain(|session| !removed.contains(&session.id));
        self.messages
            .lock()
            .retain(|message| !removed.contains(&message.session_id));
        self.executions
            .lock()
            .retain(|execution| !removed.contains(&execution.session_id));
        Ok(result.rows_affected())
    }

    /// Recover every legacy `running` execution exactly once.
    pub async fn recover_interrupted_running(&self) -> Result<u64, ConversationStoreError> {
        let replacement = self
            .crypto
            .encrypt(br#"{"status":"failed","error_kind":"interrupted"}"#)
            .map_err(|_| ConversationStoreError::Crypto)?;
        self.record_query("t127.agent_executions.running");
        let running = sqlx::query_scalar::<_, String>(CONVERSATION_RUNNING_EXECUTIONS_SQL)
            .fetch_all(&self.pool)
            .await
            .map_err(|_| ConversationStoreError::Database)?;
        if running.is_empty() {
            return Ok(0);
        }
        let result = sqlx::query(
            "UPDATE agent_executions SET status = 'failed', state_encrypted = ?, \
             finished_at = ?, error_kind = 'interrupted' WHERE status = 'running'",
        )
        .bind(replacement)
        .bind(Utc::now().to_rfc3339())
        .execute(&self.pool)
        .await
        .map_err(|_| ConversationStoreError::Database)?;
        Ok(result.rows_affected())
    }

    /// Load messages and executions using exactly two static queries.
    pub async fn load_session_bundles(
        &self,
        session_ids: &[String],
    ) -> Result<Vec<SessionBundle>, ConversationStoreError> {
        for session_id in session_ids {
            validate_session_id(session_id)?;
        }
        if session_ids.is_empty() {
            return Ok(Vec::new());
        }
        let ids_json =
            serde_json::to_string(session_ids).map_err(|_| ConversationStoreError::Database)?;
        self.record_query("t127.chat_messages.bundle");
        let message_rows = sqlx::query_as::<_, (i64, String, Vec<u8>, Option<Vec<u8>>, String)>(
            CONVERSATION_BUNDLE_MESSAGES_SQL,
        )
        .bind(&ids_json)
        .fetch_all(&self.pool)
        .await
        .map_err(|_| ConversationStoreError::Database)?;
        self.record_query("t127.agent_executions.bundle");
        let execution_rows = sqlx::query_as::<
            _,
            (String, String, Option<i64>, Vec<u8>, String, Option<String>),
        >(CONVERSATION_BUNDLE_EXECUTIONS_SQL)
        .bind(&ids_json)
        .fetch_all(&self.pool)
        .await
        .map_err(|_| ConversationStoreError::Database)?;

        let mut bundles = session_ids
            .iter()
            .cloned()
            .map(|session_id| SessionBundle {
                session_id,
                messages: Vec::new(),
                executions: Vec::new(),
            })
            .collect::<Vec<_>>();
        for row in message_rows {
            if let Some(bundle) = bundles.iter_mut().find(|bundle| bundle.session_id == row.1) {
                bundle.messages.push(ChatMessage {
                    id: row.0.to_string(),
                    session_id: row.1,
                    content_encrypted: row.2,
                    tool_calls_encrypted: row.3,
                    created_at: row.4,
                });
            }
        }
        for row in execution_rows {
            if let Some(bundle) = bundles.iter_mut().find(|bundle| bundle.session_id == row.1) {
                bundle.executions.push(AgentExecutionRecord {
                    id: row.0,
                    session_id: row.1,
                    current_agent_id: row.2,
                    state_encrypted: row.3,
                    started_at: row.4,
                    ended_at: row.5,
                });
            }
        }
        Ok(bundles)
    }

    /// Load the twenty most recently updated sessions.
    pub async fn list_recent_sessions(&self) -> Result<Vec<ChatSession>, ConversationStoreError> {
        self.record_query("t127.chat_sessions.recent");
        let rows =
            sqlx::query_as::<_, (String, i64, Option<i64>, Vec<u8>, String, String, String)>(
                CONVERSATION_RECENT_SQL,
            )
            .bind(Utc::now().to_rfc3339())
            .fetch_all(&self.pool)
            .await
            .map_err(|_| ConversationStoreError::Database)?;
        Ok(rows
            .into_iter()
            .map(|row| ChatSession {
                id: row.0,
                entry_agent_id: row.1,
                current_agent_id: row.2,
                title_encrypted: row.3,
                lifecycle: SessionLifecycle::Active,
                created_at: row.4,
                updated_at: row.5,
                expires_at: row.6,
                retention_days: self.retention_days,
            })
            .collect())
    }

    /// Number of sessions created through this handle.
    pub fn session_count(&self) -> usize {
        self.sessions.lock().len()
    }
    /// Number of messages created through this handle.
    pub fn message_count(&self) -> usize {
        self.messages.lock().len()
    }
    /// Number of executions created through this handle.
    pub fn execution_count(&self) -> usize {
        self.executions.lock().len()
    }

    fn record_query(&self, query_id: &'static str) {
        if let Some(observer) = &self.observer {
            observer.record_checked_query(query_id);
        }
    }
}

fn validate_session_id(session_id: &str) -> Result<Uuid, ConversationStoreError> {
    Uuid::parse_str(session_id)
        .map_err(|_| ConversationStoreError::InvalidSessionId(session_id.to_string()))
}

fn retention_expiry(
    now: DateTime<Utc>,
    retention_days: i64,
) -> Result<DateTime<Utc>, ConversationStoreError> {
    if retention_days == DEFAULT_RETENTION_DAYS {
        now.with_year(now.year() + 100)
            .ok_or(ConversationStoreError::InvalidRetention)
    } else {
        now.checked_add_signed(Duration::days(retention_days))
            .ok_or(ConversationStoreError::InvalidRetention)
    }
}
