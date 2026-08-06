//! Local agent session runtime (T115-T136 boundary).
//!
//! The types in this file are the minimum surface the T115 Red
//! tests require. Behaviour for full AgentLoop wiring, snapshot
//! persistence, cancel propagation, Tool dispatch and the
//! HiveWeb-failure-canary land in the T124 implementation pass.
//!
//! Critical boundary: the local agent runtime MUST never make an
//! HTTP request to HiveWeb. A Tool call to a remote tool must
//! surface as a local error, not silent retry / fallback.

#![warn(missing_docs)]

use std::{
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use serde_json::Value;
use thiserror::Error;

/// Lifecycle state of a single conversation session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionState {
    /// No work is in flight.
    Idle,
    /// The agent is waiting for the next model response.
    AwaitingModel,
    /// The agent is executing a tool call.
    RunningTool,
    /// The session is being rolled back to a snapshot.
    RollingBack,
    /// The session has terminated.
    Terminated,
}

impl SessionState {
    /// Stable wire string.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::AwaitingModel => "awaiting_model",
            Self::RunningTool => "running_tool",
            Self::RollingBack => "rolling_back",
            Self::Terminated => "terminated",
        }
    }
}

/// A single message in a conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentMessage {
    id: String,
    speaker: String,
    body: String,
}

impl AgentMessage {
    /// Build a user message with a body and a synthetic id.
    pub fn user(body: impl Into<String>) -> Self {
        Self {
            id: next_id("user"),
            speaker: "user".to_string(),
            body: body.into(),
        }
    }

    /// Build an assistant message.
    pub fn assistant(body: impl Into<String>) -> Self {
        Self {
            id: next_id("assistant"),
            speaker: "assistant".to_string(),
            body: body.into(),
        }
    }

    /// Stable message id within the session.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Speaker (user / assistant / tool).
    pub fn speaker(&self) -> &str {
        &self.speaker
    }

    /// Raw text body.
    pub fn body(&self) -> &str {
        &self.body
    }
}

/// One tool call made by the agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentToolCall {
    name: String,
    input: Value,
}

impl AgentToolCall {
    /// Construct a new tool call.
    pub fn new(name: impl Into<String>, input: Value) -> Self {
        Self {
            name: name.into(),
            input,
        }
    }

    /// Tool name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Input arguments.
    pub fn input(&self) -> &Value {
        &self.input
    }
}

/// A single execution record persisted to the session log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentExecution {
    pub id: String,
    pub session_id: String,
    pub state: SessionState,
}

/// Token used to cancel a long-running tool call.
#[derive(Debug, Clone, Default)]
pub struct CancelToken {
    cancelled: Arc<AtomicBool>,
}

impl CancelToken {
    /// Create a fresh, non-cancelled token.
    pub fn new() -> Self {
        Self::default()
    }

    /// Mark the token as cancelled.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    /// Whether the token is currently cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

/// A snapshot of a session for rollback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub id: String,
    pub session_id: String,
}

/// Errors returned by the session runtime.
#[derive(Debug, Error)]
pub enum AgentSessionError {
    /// The local agent refused to dispatch a remote / network call.
    #[error("local agent does not dispatch to HiveWeb; tool {0} must be local-only")]
    HiveWebForbidden(String),
    /// A tool call was attempted without a started session.
    #[error("agent session is not started")]
    NotStarted,
}

/// Local session runtime.
#[derive(Debug, Clone)]
pub struct AgentSession {
    database_path: std::path::PathBuf,
    state: Arc<Mutex<SessionState>>,
    messages: Arc<Mutex<Vec<AgentMessage>>>,
    executions: Arc<Mutex<Vec<AgentExecution>>>,
}

impl AgentSession {
    /// Construct a new session rooted at the given database path.
    pub fn new(database_path: &Path) -> Result<Self, AgentSessionError> {
        Ok(Self {
            database_path: database_path.to_path_buf(),
            state: Arc::new(Mutex::new(SessionState::Idle)),
            messages: Arc::new(Mutex::new(Vec::new())),
            executions: Arc::new(Mutex::new(Vec::new())),
        })
    }

    /// Database path backing this session.
    pub fn database_path(&self) -> &Path {
        &self.database_path
    }

    /// Current state, as a stable wire string.
    pub fn state(&self) -> String {
        self.state
            .lock()
            .expect("state poisoned")
            .as_str()
            .to_string()
    }

    /// Start a new turn. Returns a [`CancelToken`] that
    /// short-circuits the long-running tool call.
    pub fn start(&mut self, _input: &str) -> Result<CancelToken, AgentSessionError> {
        *self.state.lock().expect("state poisoned") = SessionState::AwaitingModel;
        Ok(CancelToken::new())
    }

    /// Invoke a Tool. The local runtime must reject any tool
    /// that would route to HiveWeb. The T115 Red test asserts
    /// this rejection is surfaced as an error (no silent retry).
    pub async fn invoke_tool(
        &self,
        call: AgentToolCall,
        _token: &CancelToken,
    ) -> Result<Value, AgentSessionError> {
        *self.state.lock().expect("state poisoned") = SessionState::RunningTool;
        Err(AgentSessionError::HiveWebForbidden(call.name))
    }

    /// Capture a snapshot of the current session state.
    pub fn snapshot(&self) -> Result<Snapshot, AgentSessionError> {
        Ok(Snapshot {
            id: next_id("snapshot"),
            session_id: next_id("session"),
        })
    }

    /// Roll the session back to the given snapshot. For the T115
    /// minimum surface this transitions the state to [`SessionState::Idle`].
    pub fn rollback_to(&mut self, _snapshot: Snapshot) -> Result<(), AgentSessionError> {
        *self.state.lock().expect("state poisoned") = SessionState::Idle;
        Ok(())
    }

    /// Append a message to the in-session log.
    pub fn append_message(&mut self, message: AgentMessage) -> Result<(), AgentSessionError> {
        self.messages
            .lock()
            .expect("messages poisoned")
            .push(message);
        Ok(())
    }

    /// Cancel any in-flight tool call and transition the session
    /// state to [`SessionState::Terminated`]. The cancel is the
    /// cooperative half; the composition's forced-termination
    /// watchdog is the half responsible for the 2-second wall-clock
    /// budget.
    pub fn cancel_in_flight(&mut self) -> Result<(), AgentSessionError> {
        *self.state.lock().expect("state poisoned") = SessionState::Terminated;
        Ok(())
    }

    /// Append an execution record.
    pub fn record_execution(&self, execution: AgentExecution) {
        self.executions
            .lock()
            .expect("executions poisoned")
            .push(execution);
    }

    /// Current messages in the session.
    pub fn messages(&self) -> Vec<AgentMessage> {
        self.messages.lock().expect("messages poisoned").clone()
    }
}

fn next_id(prefix: &str) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    format!("{prefix}-{n}")
}
