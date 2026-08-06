//! T127 conversation store (encrypted ChatSession / ChatMessage /
//! AgentExecution). The 100-year default retention lives in the
//! [`DEFAULT_RETENTION_DAYS`] constant; callers can override the
//! retention through the explicit `retention_days` parameter. The
//! store never persists plaintext for the protected fields; every
//! value is encrypted via the [`crate::datasource::crypto::Crypto`]
//! AEAD before it reaches SQLite.

#![warn(missing_docs)]

use std::sync::Arc;

use chrono::Utc;
use parking_lot::Mutex;
use serde_json::Value;
use sqlx::{Pool, Sqlite};
use thiserror::Error;
use uuid::Uuid;

use crate::datasource::crypto::Crypto;

/// Default retention period in days. The T127 contract fixes this
/// at 100 years = 36,500 days so existing conversations never
/// expire silently; callers can shorten it through the
/// `retention_days` constructor argument.
pub const DEFAULT_RETENTION_DAYS: i64 = 100 * 365;

/// Errors surfaced by the conversation store.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum ConversationStoreError {
    /// The supplied retention value is invalid.
    #[error("retention days must be greater than zero")]
    InvalidRetention,
    /// The supplied session id is not a valid UUID.
    #[error("session id {0:?} is not a valid UUID")]
    InvalidSessionId(String),
    /// The supplied execution id is not a valid UUID.
    #[error("execution id {0:?} is not a valid UUID")]
    InvalidExecutionId(String),
    /// Underlying crypto failure.
    #[error("crypto error: {0}")]
    Crypto(String),
    /// Underlying database failure.
    #[error("database error: {0}")]
    Database(String),
}

impl ConversationStoreError {
    /// Stable wire string.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::InvalidRetention => "invalid_retention",
            Self::InvalidSessionId(_) => "invalid_session_id",
            Self::InvalidExecutionId(_) => "invalid_execution_id",
            Self::Crypto(_) => "crypto",
            Self::Database(_) => "database",
        }
    }
}

/// Lifecycle state of one session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionLifecycle {
    /// Session is being created.
    Open,
    /// Session is in progress.
    Active,
    /// Session is closed.
    Closed,
}

impl SessionLifecycle {
    /// Canonical wire string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Active => "active",
            Self::Closed => "closed",
        }
    }
}

/// One persisted chat session (metadata only; the title field is
/// encrypted before storage).
#[derive(Debug, Clone)]
pub struct ChatSession {
    id: String,
    title_encrypted: Vec<u8>,
    lifecycle: SessionLifecycle,
    created_at: String,
    updated_at: String,
    retention_days: i64,
}

impl ChatSession {
    /// Session id.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Lifecycle.
    pub fn lifecycle(&self) -> SessionLifecycle {
        self.lifecycle
    }

    /// Created-at (RFC 3339).
    pub fn created_at(&self) -> &str {
        &self.created_at
    }

    /// Updated-at (RFC 3339).
    pub fn updated_at(&self) -> &str {
        &self.updated_at
    }

    /// Retention days.
    pub fn retention_days(&self) -> i64 {
        self.retention_days
    }

    /// Borrow the encrypted title.
    pub fn title_encrypted(&self) -> &[u8] {
        &self.title_encrypted
    }
}

/// One persisted chat message.
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

    /// Session id.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Created-at.
    pub fn created_at(&self) -> &str {
        &self.created_at
    }

    /// Encrypted content.
    pub fn content_encrypted(&self) -> &[u8] {
        &self.content_encrypted
    }

    /// Encrypted tool calls.
    pub fn tool_calls_encrypted(&self) -> Option<&[u8]> {
        self.tool_calls_encrypted.as_deref()
    }
}

/// One persisted agent execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionState {
    /// The execution is still running.
    Running,
    /// The execution completed successfully.
    Completed,
    /// The execution was cancelled.
    Cancelled,
    /// The execution failed.
    Failed,
}

impl ExecutionState {
    /// Canonical wire string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
        }
    }
}

/// One persisted agent execution.
#[derive(Debug, Clone)]
pub struct AgentExecutionRecord {
    id: String,
    session_id: String,
    state_encrypted: Vec<u8>,
    started_at: String,
    ended_at: Option<String>,
}

impl AgentExecutionRecord {
    /// Execution id.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Session id.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Started-at.
    pub fn started_at(&self) -> &str {
        &self.started_at
    }

    /// Ended-at.
    pub fn ended_at(&self) -> Option<&str> {
        self.ended_at.as_deref()
    }

    /// Encrypted state.
    pub fn state_encrypted(&self) -> &[u8] {
        &self.state_encrypted
    }
}

/// Conversation store handle.
#[derive(Debug, Clone)]
pub struct ConversationStore {
    crypto: Arc<Crypto>,
    pool: Pool<Sqlite>,
    retention_days: i64,
    /// in-memory cache of sessions for tests
    sessions: Arc<Mutex<Vec<ChatSession>>>,
    messages: Arc<Mutex<Vec<ChatMessage>>>,
    executions: Arc<Mutex<Vec<AgentExecutionRecord>>>,
}

impl ConversationStore {
    /// Build a new conversation store. The retention is fixed at
    /// construction time; `retention_days` is a positive integer
    /// (use [`DEFAULT_RETENTION_DAYS`] for the 100-year default).
    pub fn new(
        pool: Pool<Sqlite>,
        crypto: Arc<Crypto>,
        retention_days: Option<i64>,
    ) -> Result<Self, ConversationStoreError> {
        let retention = retention_days.unwrap_or(DEFAULT_RETENTION_DAYS);
        if retention <= 0 {
            return Err(ConversationStoreError::InvalidRetention);
        }
        Ok(Self {
            pool,
            crypto,
            retention_days: retention,
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

    /// Underlying crypto.
    pub fn crypto(&self) -> &Crypto {
        &self.crypto
    }

    /// Create a new session. The title is encrypted before storage.
    pub async fn create_session(&self, title: &str) -> Result<ChatSession, ConversationStoreError> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        let title_encrypted = self
            .crypto
            .encrypt(title.as_bytes())
            .map_err(|e| ConversationStoreError::Crypto(e.to_string()))?;
        let session = ChatSession {
            id,
            title_encrypted,
            lifecycle: SessionLifecycle::Open,
            created_at: now.clone(),
            updated_at: now,
            retention_days: self.retention_days,
        };
        self.sessions.lock().push(session.clone());
        // Persist to SQLite
        let title_blob = session.title_encrypted.clone();
        let id_clone = session.id.clone();
        let created = session.created_at.clone();
        let updated = session.updated_at.clone();
        let pool = self.pool.clone();
        let retention = self.retention_days;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS chat_sessions (\
             id TEXT PRIMARY KEY, title_encrypted BLOB NOT NULL, \
             lifecycle TEXT NOT NULL, created_at TEXT NOT NULL, \
             updated_at TEXT NOT NULL, retention_days INTEGER NOT NULL)",
        )
        .execute(&pool)
        .await
        .map_err(|e| ConversationStoreError::Database(e.to_string()))?;
        sqlx::query(
            "INSERT INTO chat_sessions (id, title_encrypted, lifecycle, created_at, updated_at, retention_days) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&id_clone)
        .bind(&title_blob)
        .bind(SessionLifecycle::Open.as_str())
        .bind(&created)
        .bind(&updated)
        .bind(retention)
        .execute(&pool)
        .await
        .map_err(|e| ConversationStoreError::Database(e.to_string()))?;
        Ok(session)
    }

    /// Append a message to a session. The content and tool calls
    /// are both encrypted before storage.
    pub async fn append_message(
        &self,
        session_id: &str,
        content: &str,
        tool_calls: Option<&Value>,
    ) -> Result<ChatMessage, ConversationStoreError> {
        Uuid::parse_str(session_id)
            .map_err(|_| ConversationStoreError::InvalidSessionId(session_id.to_string()))?;
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        let content_encrypted = self
            .crypto
            .encrypt(content.as_bytes())
            .map_err(|e| ConversationStoreError::Crypto(e.to_string()))?;
        let tool_calls_encrypted = if let Some(value) = tool_calls {
            let bytes = serde_json::to_vec(value)
                .map_err(|e| ConversationStoreError::Crypto(e.to_string()))?;
            Some(
                self.crypto
                    .encrypt(&bytes)
                    .map_err(|e| ConversationStoreError::Crypto(e.to_string()))?,
            )
        } else {
            None
        };
        let message = ChatMessage {
            id,
            session_id: session_id.to_string(),
            content_encrypted,
            tool_calls_encrypted,
            created_at: now,
        };
        self.messages.lock().push(message.clone());
        Ok(message)
    }

    /// Record a new agent execution. The state is encrypted before
    /// storage.
    pub async fn record_execution(
        &self,
        session_id: &str,
        state: ExecutionState,
    ) -> Result<AgentExecutionRecord, ConversationStoreError> {
        Uuid::parse_str(session_id)
            .map_err(|_| ConversationStoreError::InvalidSessionId(session_id.to_string()))?;
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        let state_bytes = state.as_str().as_bytes();
        let state_encrypted = self
            .crypto
            .encrypt(state_bytes)
            .map_err(|e| ConversationStoreError::Crypto(e.to_string()))?;
        let record = AgentExecutionRecord {
            id,
            session_id: session_id.to_string(),
            state_encrypted,
            started_at: now,
            ended_at: None,
        };
        self.executions.lock().push(record.clone());
        Ok(record)
    }

    /// Count sessions in the in-memory cache (for the
    /// `impact_count` UI badge).
    pub fn session_count(&self) -> usize {
        self.sessions.lock().len()
    }

    /// Count messages in the in-memory cache.
    pub fn message_count(&self) -> usize {
        self.messages.lock().len()
    }

    /// Count executions in the in-memory cache.
    pub fn execution_count(&self) -> usize {
        self.executions.lock().len()
    }
}
