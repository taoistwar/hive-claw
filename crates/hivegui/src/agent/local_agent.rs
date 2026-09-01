//! Local agent runtime (T125 boundary).
//!
//! T125 implementation: public session startup, per-turn immutable
//! Agent/Tool/Skill/Capability snapshots, and direct-child-only
//! routing. The runtime is intentionally local-only; remote
//! execution is never wired in.

#![warn(missing_docs)]

use std::{collections::BTreeMap, future::Future, pin::Pin, sync::Arc};

use chrono::{Duration as ChronoDuration, Utc};
use parking_lot::RwLock;
use serde::Deserialize;
use serde_json::Value;
use sqlx::{Pool, Sqlite};
use thiserror::Error;
use uuid::Uuid;

use crate::datasource::entity_store::{AgentRecord, AgentStore};
use crate::{
    agent::agent_content::{CapabilityDescriptor, SkillDescriptor, TurnContent},
    agent::session::{AgentMessage, AgentSession, CancelToken, SessionState},
    runtime::tool_adapter::{PersistedToolExecutor, ToolExecutionContext},
};

/// Stable handle to a single conversation session.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct SessionHandle(pub String);

impl SessionHandle {
    /// Borrow the underlying identifier.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Errors surfaced by the local agent runtime.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum LocalAgentError {
    /// `start_session` was called but no default root Agent exists.
    #[error("no default root agent; create one before starting a session")]
    NoDefaultRoot,
    /// The Agent hierarchy is malformed.
    #[error("agent hierarchy is malformed: {0}")]
    InvalidHierarchy(String),
    /// The supplied user_message was empty or larger than 1MiB.
    #[error("user_message rejected: {0}")]
    InvalidUserMessage(&'static str),
    /// One public command argument failed validation.
    #[error("invalid input for {field}: {reason}")]
    InvalidInput {
        /// Stable public field name.
        field: &'static str,
        /// Stable non-sensitive reason code.
        reason: &'static str,
    },
    /// The Agent rejected the operation.
    #[error("agent operation rejected: {0}")]
    AgentRejected(&'static str),
    /// A tool call routed to a remote / network target.
    #[error("tool {0} is not local; remote calls are forbidden")]
    RemoteToolForbidden(String),
    /// Database or store failure.
    #[error("store error: {0}")]
    Store(String),
    /// A persisted local Tool failed at its stable public boundary.
    #[error("tool execution failed: {0}")]
    ToolExecution(&'static str),
}

impl LocalAgentError {
    /// Stable field name for a public input failure, or an empty string when
    /// the error is not field-scoped.
    pub fn field(&self) -> &str {
        match self {
            Self::InvalidUserMessage(_) => "user_message",
            Self::InvalidInput { field, .. } => field,
            _ => "",
        }
    }

    /// Stable reason code for a public input failure, or an empty string when
    /// the error is not an input-validation failure.
    pub fn reason(&self) -> &str {
        match self {
            Self::InvalidUserMessage(reason) => reason,
            Self::InvalidInput { reason, .. } => reason,
            _ => "",
        }
    }
}

/// A validated local action accepted from one LLM decision.
///
/// The action is deliberately transport-free: parsing and snapshot validation
/// happen before this value reaches a scheduler, and the scheduler remains a
/// HiveGUI-local boundary.
#[derive(Debug, Clone, PartialEq)]
pub enum ScheduledAgentAction {
    /// Deliver a final assistant reply to the current session.
    Reply {
        /// Reply body produced by the configured local LLM.
        content: String,
    },
    /// Start one Tool that belongs to the immutable turn snapshot.
    ToolCall {
        /// Persisted Tool primary key.
        tool_id: i64,
        /// JSON input passed to the Tool adapter after scheduling.
        input: Value,
    },
    /// Route the next turn to one direct child Agent.
    RouteChild {
        /// Stable direct-child identifier.
        identifier: String,
    },
}

/// Boxed local scheduling future.
pub type LocalAgentScheduleFuture =
    Pin<Box<dyn Future<Output = Result<(), LocalAgentError>> + Send + 'static>>;

/// Future returned by an injected local LLM decision boundary.
pub type LocalAgentDecisionFuture =
    Pin<Box<dyn Future<Output = Result<String, LocalAgentError>> + Send + 'static>>;

/// Local decision source used by the Agent loop.
///
/// Production adapters resolve the configured local Provider before entering
/// this boundary. Tests inject a deterministic model. Neither implementation
/// receives a HiveWeb URL or fallback client.
pub trait LocalAgentDecisionModel: Send + Sync {
    /// Produce the next strict JSON decision for this turn.
    fn decide(
        &self,
        snapshot: TurnSnapshot,
        messages: Vec<AgentMessage>,
        tool_observation: Option<Value>,
    ) -> LocalAgentDecisionFuture;
}

/// Terminal result from one local Agent turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalAgentTurnResult {
    reply: String,
    tool_calls: usize,
}

impl LocalAgentTurnResult {
    /// Final assistant reply.
    pub fn reply(&self) -> &str {
        &self.reply
    }

    /// Number of persisted local Tools executed before the reply.
    pub fn tool_calls(&self) -> usize {
        self.tool_calls
    }
}

/// Scheduling seam used after an LLM decision has been parsed and validated.
///
/// Implementations enqueue local work and return when the action has been
/// accepted. They must not wait for external LLM, network, Tool, Workflow, or
/// Plugin execution to finish.
pub trait LocalAgentActionScheduler: Send + Sync {
    /// Accept one validated local action for asynchronous execution.
    fn schedule(&self, action: ScheduledAgentAction) -> LocalAgentScheduleFuture;
}

#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum AgentDecision {
    Reply { content: String },
    ToolCall { tool_id: i64, input: Value },
    RouteChild { identifier: String },
}

/// Immutable Tool metadata exposed to one local model turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnTool {
    /// Persisted Tool id used by the decision wire format.
    pub id: i64,
    /// Stable Tool identifier.
    pub identifier: String,
    /// Human-readable Tool name.
    pub name: String,
    /// Non-secret description supplied to the local model.
    pub description: String,
    /// Draft-7 JSON input schema.
    pub input_schema: String,
}

/// Per-turn snapshot of Agent, Tool, Skill and Capability state.
#[derive(Debug, Clone)]
pub struct TurnSnapshot {
    /// The Agent this snapshot belongs to.
    pub agent: AgentRecord,
    /// Resolved tool ids.
    pub tool_ids: Vec<i64>,
    /// Resolved explicit and always-on Tool metadata.
    pub tools: Vec<TurnTool>,
    /// Resolved skill ids.
    pub skill_ids: Vec<i64>,
    /// Resolved global always-on skill ids.
    pub always_skill_ids: Vec<i64>,
    /// Resolved capability names.
    pub capability_names: Vec<String>,
    /// The child Agents that this Agent is allowed to route to.
    pub direct_children: Vec<AgentRecord>,
    /// Immutable system prompt, Skill content and Capability metadata.
    pub content: TurnContent,
}

impl TurnSnapshot {
    /// Build a snapshot from a typed [`AgentStore`].
    pub async fn load(store: &AgentStore, agent_id: i64) -> Result<Self, LocalAgentError> {
        let agent = store
            .load_resource_snapshots(&[agent_id])
            .await
            .map_err(|e| LocalAgentError::Store(e.to_string()))?
            .into_iter()
            .next()
            .ok_or(LocalAgentError::InvalidHierarchy(format!(
                "agent {agent_id} not found"
            )))?;
        let tool_rows = sqlx::query_as::<_, (i64, String, String, String, String)>(
            "SELECT tools.id, tools.identifier, tools.name, tools.description, tools.input_schema \
             FROM tools LEFT JOIN agent_tools \
               ON agent_tools.tool_id = tools.id AND agent_tools.agent_id = ? \
             WHERE tools.is_always = 1 OR agent_tools.agent_id IS NOT NULL \
             ORDER BY tools.id ASC",
        )
        .bind(agent_id)
        .fetch_all(store.pool())
        .await
        .map_err(|error| LocalAgentError::Store(error.to_string()))?;
        let tools = tool_rows
            .into_iter()
            .map(
                |(id, identifier, name, description, input_schema)| TurnTool {
                    id,
                    identifier,
                    name,
                    description,
                    input_schema,
                },
            )
            .collect::<Vec<_>>();
        let tool_ids = tools.iter().map(|tool| tool.id).collect::<Vec<_>>();
        let skill_ids = agent.skill_ids().to_vec();
        let always_skill_ids = agent.always_skill_ids().to_vec();
        let capability_names = agent.capability_names().to_vec();
        let skill_rows = sqlx::query_as::<_, (i64, String, String, bool)>(
            "SELECT skills.id, skills.identifier, skills.content, skills.is_always \
             FROM skills LEFT JOIN agent_skills \
               ON agent_skills.skill_id = skills.id AND agent_skills.agent_id = ? \
             WHERE skills.is_always = 1 OR agent_skills.agent_id IS NOT NULL \
             ORDER BY CASE WHEN agent_skills.agent_id IS NULL THEN 1 ELSE 0 END, skills.id ASC",
        )
        .bind(agent_id)
        .fetch_all(store.pool())
        .await
        .map_err(|error| LocalAgentError::Store(error.to_string()))?;
        let mut content = TurnContent::new().with_system_prompt(agent.system_prompt());
        for (_, identifier, body, _) in skill_rows {
            content = content.with_skill(SkillDescriptor::new(identifier, body));
        }
        let capability_rows = sqlx::query_as::<_, (String, bool)>(
            "SELECT capabilities.name, capabilities.is_dangerous \
             FROM agent_capabilities \
             JOIN capabilities ON capabilities.name = agent_capabilities.capability_name \
             WHERE agent_capabilities.agent_id = ? ORDER BY capabilities.name ASC",
        )
        .bind(agent_id)
        .fetch_all(store.pool())
        .await
        .map_err(|error| LocalAgentError::Store(error.to_string()))?;
        for (name, is_dangerous) in capability_rows {
            content = content.with_capability(CapabilityDescriptor::new(name, is_dangerous));
        }
        let children = store
            .list_children(agent_id)
            .await
            .map_err(|e| LocalAgentError::Store(e.to_string()))?;
        Ok(Self {
            agent,
            tool_ids,
            tools,
            skill_ids,
            always_skill_ids,
            capability_names,
            direct_children: children,
            content,
        })
    }
}

/// Local-only agent runtime.
#[derive(Debug, Clone)]
pub struct LocalAgentRuntime {
    pool: Pool<Sqlite>,
    store: AgentStore,
    sessions: Arc<RwLock<BTreeMap<SessionHandle, AgentSession>>>,
    snapshots: Arc<RwLock<BTreeMap<SessionHandle, TurnSnapshot>>>,
    execution_sessions: Arc<RwLock<BTreeMap<String, SessionHandle>>>,
}

impl LocalAgentRuntime {
    /// Build a new runtime against the given SQLite pool.
    pub fn new(pool: Pool<Sqlite>) -> Result<Self, LocalAgentError> {
        let store =
            AgentStore::new(pool.clone()).map_err(|e| LocalAgentError::Store(e.to_string()))?;
        Ok(Self {
            pool,
            store,
            sessions: Arc::new(RwLock::new(BTreeMap::new())),
            snapshots: Arc::new(RwLock::new(BTreeMap::new())),
            execution_sessions: Arc::new(RwLock::new(BTreeMap::new())),
        })
    }

    /// Underlying pool.
    pub fn pool(&self) -> &Pool<Sqlite> {
        &self.pool
    }

    /// Underlying Agent store.
    pub fn store(&self) -> &AgentStore {
        &self.store
    }

    /// Start a new session against the unique default root Agent.
    pub async fn start_session(
        &self,
        user_message: &str,
    ) -> Result<(SessionHandle, CancelToken), LocalAgentError> {
        let user_message = validated_user_message(user_message)?;
        let default = self
            .store
            .default_agent()
            .await
            .map_err(|e| LocalAgentError::Store(e.to_string()))?
            .ok_or(LocalAgentError::NoDefaultRoot)?;
        let snapshot = TurnSnapshot::load(&self.store, default.id()).await?;
        let session_id = Uuid::new_v4().to_string();
        let execution_id = Uuid::new_v4().to_string();
        let handle = SessionHandle(session_id.clone());
        let mut session = AgentSession::new(std::path::Path::new(":memory:"))
            .map_err(|e| LocalAgentError::Store(e.to_string()))?;
        let token = session
            .start(&user_message)
            .map_err(|e| LocalAgentError::Store(e.to_string()))?;
        session
            .append_message(AgentMessage::user(user_message))
            .map_err(|e| LocalAgentError::Store(e.to_string()))?;

        let now = Utc::now();
        let now_text = now.to_rfc3339();
        let expires_at = (now + ChronoDuration::days(100 * 365)).to_rfc3339();
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|error| LocalAgentError::Store(error.to_string()))?;
        sqlx::query(
            "INSERT INTO chat_sessions \
             (id, entry_agent_id, current_agent_id, title_encrypted, status, execution_id, created_at, updated_at, expires_at) \
             VALUES (?, ?, ?, ?, 'active', ?, ?, ?, ?)",
        )
        .bind(&session_id)
        .bind(default.id())
        .bind(default.id())
        .bind(Vec::<u8>::new())
        .bind(&execution_id)
        .bind(&now_text)
        .bind(&now_text)
        .bind(&expires_at)
        .execute(&mut *transaction)
        .await
        .map_err(|error| LocalAgentError::Store(error.to_string()))?;
        sqlx::query(
            "INSERT INTO agent_executions \
             (execution_id, session_id, current_agent_id, status, state_encrypted, started_at) \
             VALUES (?, ?, ?, 'running', NULL, ?)",
        )
        .bind(&execution_id)
        .bind(&session_id)
        .bind(default.id())
        .bind(&now_text)
        .execute(&mut *transaction)
        .await
        .map_err(|error| LocalAgentError::Store(error.to_string()))?;
        transaction
            .commit()
            .await
            .map_err(|error| LocalAgentError::Store(error.to_string()))?;

        self.sessions.write().insert(handle.clone(), session);
        self.snapshots.write().insert(handle.clone(), snapshot);
        self.execution_sessions
            .write()
            .insert(execution_id, handle.clone());
        Ok((handle, token))
    }

    /// Accept another user turn for a persisted session and return the new
    /// execution UUID. Validation completes before either table is modified.
    pub async fn continue_session(
        &self,
        session_id: &str,
        user_message: &str,
    ) -> Result<String, LocalAgentError> {
        validate_uuid(session_id, "session_id")?;
        let user_message = validated_user_message(user_message)?;
        let handle = SessionHandle(session_id.to_string());
        if !self.sessions.read().contains_key(&handle) {
            return Err(LocalAgentError::AgentRejected("session not found"));
        }
        let current_agent_id = self
            .snapshot(&handle)
            .ok_or(LocalAgentError::AgentRejected("snapshot missing"))?
            .agent
            .id();
        let execution_id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|error| LocalAgentError::Store(error.to_string()))?;
        let updated = sqlx::query(
            "UPDATE chat_sessions SET execution_id = ?, current_agent_id = ?, status = 'active', updated_at = ? WHERE id = ?",
        )
        .bind(&execution_id)
        .bind(current_agent_id)
        .bind(&now)
        .bind(session_id)
        .execute(&mut *transaction)
        .await
        .map_err(|error| LocalAgentError::Store(error.to_string()))?;
        if updated.rows_affected() != 1 {
            return Err(LocalAgentError::AgentRejected("session not found"));
        }
        sqlx::query(
            "INSERT INTO agent_executions \
             (execution_id, session_id, current_agent_id, status, state_encrypted, started_at) \
             VALUES (?, ?, ?, 'running', NULL, ?)",
        )
        .bind(&execution_id)
        .bind(session_id)
        .bind(current_agent_id)
        .bind(&now)
        .execute(&mut *transaction)
        .await
        .map_err(|error| LocalAgentError::Store(error.to_string()))?;
        transaction
            .commit()
            .await
            .map_err(|error| LocalAgentError::Store(error.to_string()))?;
        self.begin_turn(&handle, &user_message)?;
        self.execution_sessions
            .write()
            .insert(execution_id.clone(), handle);
        Ok(execution_id)
    }

    /// Stop one running execution. Returns `true` when this call changed the
    /// persisted terminal state and `false` when it was already terminal.
    pub async fn stop_execution(&self, execution_id: &str) -> Result<bool, LocalAgentError> {
        validate_uuid(execution_id, "execution_id")?;
        let row = sqlx::query_as::<_, (String, String)>(
            "SELECT session_id, status FROM agent_executions WHERE execution_id = ?",
        )
        .bind(execution_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| LocalAgentError::Store(error.to_string()))?
        .ok_or(LocalAgentError::AgentRejected("execution not found"))?;
        if row.1 != "running" {
            return Ok(false);
        }
        let now = Utc::now().to_rfc3339();
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|error| LocalAgentError::Store(error.to_string()))?;
        sqlx::query(
            "UPDATE agent_executions SET status = 'cancelled', finished_at = ? \
             WHERE execution_id = ? AND status = 'running'",
        )
        .bind(&now)
        .bind(execution_id)
        .execute(&mut *transaction)
        .await
        .map_err(|error| LocalAgentError::Store(error.to_string()))?;
        sqlx::query("UPDATE chat_sessions SET status = 'cancelled', updated_at = ? WHERE id = ?")
            .bind(&now)
            .bind(&row.0)
            .execute(&mut *transaction)
            .await
            .map_err(|error| LocalAgentError::Store(error.to_string()))?;
        transaction
            .commit()
            .await
            .map_err(|error| LocalAgentError::Store(error.to_string()))?;
        if let Some(handle) = self.execution_sessions.read().get(execution_id).cloned() {
            self.cancel_execution(&handle)?;
        }
        Ok(true)
    }

    /// Delete one persisted session and its cascaded messages/executions.
    pub async fn delete_session(&self, session_id: &str) -> Result<u64, LocalAgentError> {
        validate_uuid(session_id, "session_id")?;
        let result = sqlx::query("DELETE FROM chat_sessions WHERE id = ?")
            .bind(session_id)
            .execute(&self.pool)
            .await
            .map_err(|error| LocalAgentError::Store(error.to_string()))?;
        let handle = SessionHandle(session_id.to_string());
        self.sessions.write().remove(&handle);
        self.snapshots.write().remove(&handle);
        self.execution_sessions
            .write()
            .retain(|_, owner| owner != &handle);
        Ok(result.rows_affected())
    }

    /// Delete persisted sessions older than a validated positive retention
    /// filter such as `30 days` or `12 hours`.
    pub async fn clear_history(&self, retention_filter: &str) -> Result<u64, LocalAgentError> {
        let cutoff = retention_cutoff(retention_filter)?;
        let session_ids = sqlx::query_scalar::<_, String>(
            "SELECT id FROM chat_sessions WHERE updated_at < ? ORDER BY id ASC",
        )
        .bind(&cutoff)
        .fetch_all(&self.pool)
        .await
        .map_err(|error| LocalAgentError::Store(error.to_string()))?;
        let result = sqlx::query("DELETE FROM chat_sessions WHERE updated_at < ?")
            .bind(&cutoff)
            .execute(&self.pool)
            .await
            .map_err(|error| LocalAgentError::Store(error.to_string()))?;
        for session_id in session_ids {
            let handle = SessionHandle(session_id);
            self.sessions.write().remove(&handle);
            self.snapshots.write().remove(&handle);
            self.execution_sessions
                .write()
                .retain(|_, owner| owner != &handle);
        }
        Ok(result.rows_affected())
    }

    /// Start another turn in an existing session and append its user message
    /// exactly once. A fresh cooperative cancellation token replaces the
    /// prior terminal turn token.
    pub fn begin_turn(
        &self,
        handle: &SessionHandle,
        user_message: &str,
    ) -> Result<CancelToken, LocalAgentError> {
        let user_message = validated_user_message(user_message)?;
        let mut sessions = self.sessions.write();
        let session = sessions
            .get_mut(handle)
            .ok_or(LocalAgentError::AgentRejected("session not found"))?;
        let token = session
            .start(&user_message)
            .map_err(|error| LocalAgentError::Store(error.to_string()))?;
        session
            .append_message(AgentMessage::user(user_message))
            .map_err(|error| LocalAgentError::Store(error.to_string()))?;
        Ok(token)
    }

    /// Clone the active turn's cancellation token for a local runtime adapter.
    pub fn cancel_token(&self, handle: &SessionHandle) -> Option<CancelToken> {
        self.sessions
            .read()
            .get(handle)
            .and_then(AgentSession::cancel_token)
    }

    /// Fetch the current per-turn snapshot for a session.
    pub fn snapshot(&self, handle: &SessionHandle) -> Option<TurnSnapshot> {
        self.snapshots.read().get(handle).cloned()
    }

    /// Parse one local LLM decision, validate it against the immutable turn
    /// snapshot, and hand the resulting action to a local scheduler.
    ///
    /// This is the exact T122 performance boundary: it starts after external
    /// LLM response time has ended and returns as soon as the next local action
    /// is accepted for scheduling. User Function, Workflow, Plugin, and Tool
    /// execution time is outside this method.
    pub async fn schedule_decision(
        &self,
        handle: &SessionHandle,
        decision_json: &str,
        scheduler: &dyn LocalAgentActionScheduler,
    ) -> Result<ScheduledAgentAction, LocalAgentError> {
        let action = self.validated_action(handle, decision_json)?;

        scheduler.schedule(action.clone()).await?;
        Ok(action)
    }

    fn validated_action(
        &self,
        handle: &SessionHandle,
        decision_json: &str,
    ) -> Result<ScheduledAgentAction, LocalAgentError> {
        if decision_json.is_empty() {
            return Err(LocalAgentError::AgentRejected("empty LLM decision"));
        }
        if decision_json.len() > 1024 * 1024 {
            return Err(LocalAgentError::AgentRejected("LLM decision exceeds 1MiB"));
        }
        let decision = serde_json::from_str::<AgentDecision>(decision_json)
            .map_err(|_| LocalAgentError::AgentRejected("invalid LLM decision"))?;
        let snapshot = self
            .snapshot(handle)
            .ok_or(LocalAgentError::AgentRejected("snapshot missing"))?;

        match decision {
            AgentDecision::Reply { content } => {
                if content.is_empty() {
                    return Err(LocalAgentError::AgentRejected("empty reply"));
                }
                Ok(ScheduledAgentAction::Reply { content })
            }
            AgentDecision::ToolCall { tool_id, input } => {
                if !snapshot.tool_ids.contains(&tool_id) {
                    return Err(LocalAgentError::AgentRejected("tool not in turn snapshot"));
                }
                Ok(ScheduledAgentAction::ToolCall { tool_id, input })
            }
            AgentDecision::RouteChild { identifier } => {
                if !snapshot
                    .direct_children
                    .iter()
                    .any(|child| child.identifier() == identifier)
                {
                    return Err(LocalAgentError::AgentRejected(
                        "child not in direct_children set",
                    ));
                }
                Ok(ScheduledAgentAction::RouteChild { identifier })
            }
        }
    }

    /// Run one bounded local decision loop to a single terminal reply.
    ///
    /// Each Tool decision is validated against the immutable turn snapshot and
    /// executed through [`PersistedToolExecutor`]. Tool output is appended to
    /// the local session and supplied to the next model decision. The loop is
    /// bounded by the feature-wide Agent depth limit of ten.
    pub async fn run_turn(
        &self,
        handle: &SessionHandle,
        model: &dyn LocalAgentDecisionModel,
        tool_executor: &PersistedToolExecutor,
    ) -> Result<LocalAgentTurnResult, LocalAgentError> {
        let cancel_token = self
            .cancel_token(handle)
            .ok_or(LocalAgentError::AgentRejected("turn not started"))?;
        let mut tool_observation = None;
        let mut tool_calls = 0usize;

        for _ in 0..10 {
            ensure_turn_not_cancelled(&cancel_token)?;
            self.set_session_state(handle, SessionState::AwaitingModel)?;
            let snapshot = self
                .snapshot(handle)
                .ok_or(LocalAgentError::AgentRejected("snapshot missing"))?;
            let messages = self.messages(handle)?;
            let decision = model
                .decide(snapshot.clone(), messages, tool_observation.take())
                .await?;
            ensure_turn_not_cancelled(&cancel_token)?;
            match self.validated_action(handle, &decision)? {
                ScheduledAgentAction::Reply { content } => {
                    self.append_message(handle, AgentMessage::assistant(content.clone()))?;
                    self.set_session_state(handle, SessionState::Idle)?;
                    return Ok(LocalAgentTurnResult {
                        reply: content,
                        tool_calls,
                    });
                }
                ScheduledAgentAction::ToolCall { tool_id, input } => {
                    self.set_session_state(handle, SessionState::RunningTool)?;
                    let output = tool_executor
                        .execute(
                            tool_id,
                            input,
                            ToolExecutionContext::new(snapshot.capability_names.clone()),
                        )
                        .await
                        .map_err(|error| LocalAgentError::ToolExecution(error.code()))?;
                    ensure_turn_not_cancelled(&cancel_token)?;
                    tool_calls += 1;
                    let observation = serde_json::json!({
                        "tool_id": tool_id,
                        "succeeded": true,
                        "output": output,
                    });
                    self.append_message(handle, AgentMessage::tool(observation.to_string()))?;
                    tool_observation = Some(observation);
                }
                ScheduledAgentAction::RouteChild { identifier } => {
                    self.route_to_child(handle, &identifier).await?;
                }
            }
        }

        Err(LocalAgentError::AgentRejected(
            "maximum Agent depth exceeded",
        ))
    }

    /// Reload the per-turn snapshot from the store.
    pub async fn reload_snapshot(
        &self,
        handle: &SessionHandle,
    ) -> Result<TurnSnapshot, LocalAgentError> {
        if !self.sessions.read().contains_key(handle) {
            return Err(LocalAgentError::AgentRejected("session not found"));
        }
        let current = self
            .snapshot(handle)
            .ok_or(LocalAgentError::AgentRejected("snapshot missing"))?;
        let fresh = TurnSnapshot::load(&self.store, current.agent.id()).await?;
        self.snapshots.write().insert(handle.clone(), fresh.clone());
        Ok(fresh)
    }

    /// Route the current turn to a direct child Agent.
    pub async fn route_to_child(
        &self,
        handle: &SessionHandle,
        child_identifier: &str,
    ) -> Result<TurnSnapshot, LocalAgentError> {
        let snapshot = self
            .snapshot(handle)
            .ok_or(LocalAgentError::AgentRejected("snapshot missing"))?;
        let child = snapshot
            .direct_children
            .iter()
            .find(|agent| agent.identifier() == child_identifier)
            .cloned()
            .ok_or(LocalAgentError::AgentRejected(
                "child not in direct_children set",
            ))?;
        let next = TurnSnapshot::load(&self.store, child.id()).await?;
        self.snapshots.write().insert(handle.clone(), next.clone());
        Ok(next)
    }

    /// Append a message to a session.
    pub fn append_message(
        &self,
        handle: &SessionHandle,
        message: AgentMessage,
    ) -> Result<(), LocalAgentError> {
        let mut sessions = self.sessions.write();
        let session = sessions
            .get_mut(handle)
            .ok_or(LocalAgentError::AgentRejected("session not found"))?;
        session
            .append_message(message)
            .map_err(|e| LocalAgentError::Store(e.to_string()))
    }

    /// Snapshot the current in-memory session messages.
    pub fn messages(&self, handle: &SessionHandle) -> Result<Vec<AgentMessage>, LocalAgentError> {
        self.sessions
            .read()
            .get(handle)
            .map(AgentSession::messages)
            .ok_or(LocalAgentError::AgentRejected("session not found"))
    }

    fn set_session_state(
        &self,
        handle: &SessionHandle,
        state: SessionState,
    ) -> Result<(), LocalAgentError> {
        let sessions = self.sessions.read();
        let session = sessions
            .get(handle)
            .ok_or(LocalAgentError::AgentRejected("session not found"))?;
        session.set_state(state);
        Ok(())
    }

    /// Terminate a session.
    pub fn end_session(&self, handle: &SessionHandle) -> Result<(), LocalAgentError> {
        let removed = self.sessions.write().remove(handle).is_some();
        self.snapshots.write().remove(handle);
        self.execution_sessions
            .write()
            .retain(|_, owner| owner != handle);
        if removed {
            Ok(())
        } else {
            Err(LocalAgentError::AgentRejected("session not found"))
        }
    }

    /// Current session state as a stable wire string.
    pub fn session_state(&self, handle: &SessionHandle) -> Option<String> {
        self.sessions.read().get(handle).map(|s| s.state())
    }

    /// Cooperative cancel for a long-running local turn. The active
    /// [`CancelToken`] is signalled, the session state transitions
    /// to `Terminated`, and the late-result guard ensures any
    /// in-flight adapter result is discarded. The session itself
    /// remains in the runtime for diagnostics; callers can
    /// [`Self::end_session`] to remove it.
    pub fn cancel_execution(&self, handle: &SessionHandle) -> Result<(), LocalAgentError> {
        let mut sessions = self.sessions.write();
        let session = sessions
            .get_mut(handle)
            .ok_or(LocalAgentError::AgentRejected("session not found"))?;
        session
            .cancel_in_flight()
            .map_err(|e| LocalAgentError::Store(e.to_string()))?;
        Ok(())
    }
}

fn validated_user_message(user_message: &str) -> Result<String, LocalAgentError> {
    let user_message = user_message.trim();
    if user_message.is_empty() {
        return Err(LocalAgentError::InvalidUserMessage("empty"));
    }
    if user_message.len() > 1024 * 1024 {
        return Err(LocalAgentError::InvalidUserMessage("exceeds 1MiB"));
    }
    Ok(user_message.to_string())
}

fn validate_uuid(value: &str, field: &'static str) -> Result<Uuid, LocalAgentError> {
    Uuid::parse_str(value).map_err(|_| LocalAgentError::InvalidInput {
        field,
        reason: "invalid_uuid",
    })
}

fn retention_cutoff(retention_filter: &str) -> Result<String, LocalAgentError> {
    let mut parts = retention_filter.split_whitespace();
    let value = parts.next().and_then(|part| part.parse::<i64>().ok());
    let unit = parts.next();
    if parts.next().is_some() || value.is_none() || unit.is_none() {
        return Err(LocalAgentError::InvalidInput {
            field: "retention_filter",
            reason: "invalid_format",
        });
    }
    let value = value.expect("checked above");
    if value <= 0 {
        return Err(LocalAgentError::InvalidInput {
            field: "retention_filter",
            reason: "out_of_range",
        });
    }
    let duration = match unit.expect("checked above") {
        "day" | "days" => ChronoDuration::days(value),
        "hour" | "hours" => ChronoDuration::hours(value),
        _ => {
            return Err(LocalAgentError::InvalidInput {
                field: "retention_filter",
                reason: "unsupported_unit",
            });
        }
    };
    Ok((Utc::now() - duration).to_rfc3339())
}

fn ensure_turn_not_cancelled(token: &CancelToken) -> Result<(), LocalAgentError> {
    if token.is_cancelled() {
        Err(LocalAgentError::AgentRejected(
            "local provider call cancelled",
        ))
    } else {
        Ok(())
    }
}
