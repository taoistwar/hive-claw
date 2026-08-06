//! T126 local tool-calling adapter.
//!
//! The local agent runtime uses this adapter to dispatch
//! Function/Workflow/Plugin tool calls. The adapter never routes to
//! a remote or network target; a request that would require such a
//! route is rejected with [`ToolAdapterError::RemoteForbidden`].

#![warn(missing_docs)]

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use uuid::Uuid;

use crate::runtime::execution::{FailureCategory, LocalExecutionOutcome};

/// Stable tool kind label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    /// Built-in pure computation (format_template, json_parse, etc.).
    Builtin,
    /// User-defined extism plugin function.
    Function,
    /// Workflow dispatch.
    Workflow,
    /// Internal HiveGUI tool that does not route outside the local
    /// runtime.
    Local,
}

impl ToolKind {
    /// Canonical wire string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
            Self::Function => "function",
            Self::Workflow => "workflow",
            Self::Local => "local",
        }
    }
}

/// One tool call request produced by the LLM/agent loop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallRequest {
    /// Stable call id (UUID v4) so the result can be paired with
    /// the request.
    pub call_id: String,
    /// Tool identifier.
    pub identifier: String,
    /// Tool kind.
    pub kind: ToolKind,
    /// Tool arguments.
    pub input: Value,
}

impl ToolCallRequest {
    /// Build a new request with a generated call id.
    pub fn new(identifier: impl Into<String>, kind: ToolKind, input: Value) -> Self {
        Self {
            call_id: Uuid::new_v4().to_string(),
            identifier: identifier.into(),
            kind,
            input,
        }
    }
}

/// One tool call result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallResult {
    /// Same call id as the request.
    pub call_id: String,
    /// Whether the call succeeded.
    pub succeeded: bool,
    /// Result content (for success) or error reason (for failure).
    pub output: Value,
}

impl ToolCallResult {
    /// Build a successful result.
    pub fn success(call_id: impl Into<String>, output: Value) -> Self {
        Self {
            call_id: call_id.into(),
            succeeded: true,
            output,
        }
    }

    /// Build a failed result.
    pub fn failure(call_id: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            call_id: call_id.into(),
            succeeded: false,
            output: Value::String(reason.into()),
        }
    }
}

/// Errors surfaced by the local tool adapter.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum ToolAdapterError {
    /// A tool call routed to a remote / network target. The local
    /// adapter never allows this.
    #[error("tool {0} is not local; remote calls are forbidden")]
    RemoteForbidden(String),
    /// The requested tool does not exist in the local registry.
    #[error("tool {0} not found")]
    NotFound(String),
    /// The tool returned a non-terminal outcome.
    #[error("tool {0} produced a non-terminal outcome")]
    NonTerminal(String),
    /// The tool call was cancelled.
    #[error("tool {0} was cancelled")]
    Cancelled(String),
    /// The tool adapter is at capacity.
    #[error("tool adapter is at capacity")]
    AtCapacity,
    /// Underlying backend failure.
    #[error("tool backend error: {0}")]
    Backend(String),
}

/// Trait abstraction for tool dispatch. The T126 test injection
/// uses this trait to drive the local loop without invoking any
/// real Builtin/Plugin/Workflow runtime.
pub trait LocalToolDispatcher: Send + Sync {
    /// Dispatch a single tool call. The future must resolve to a
    /// terminal [`ToolCallResult`].
    fn dispatch(
        &self,
        request: ToolCallRequest,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<ToolCallResult, ToolAdapterError>> + Send>,
    >;
}

/// Default in-memory dispatcher. The T126 wiring layer registers
/// the real Builtin/Plugin/Workflow dispatchers; this stub returns
/// a successful empty result so the loop terminates.
#[derive(Debug, Default)]
pub struct InMemoryToolDispatcher {
    inner: parking_lot::Mutex<Vec<ToolCallResult>>,
}

impl InMemoryToolDispatcher {
    /// Build a new empty dispatcher.
    pub fn new() -> Self {
        Self::default()
    }

    /// Snapshot the dispatched calls.
    pub fn calls(&self) -> Vec<ToolCallResult> {
        self.inner.lock().clone()
    }
}

impl LocalToolDispatcher for InMemoryToolDispatcher {
    fn dispatch(
        &self,
        request: ToolCallRequest,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<ToolCallResult, ToolAdapterError>> + Send>,
    > {
        let result = ToolCallResult::success(request.call_id.clone(), Value::Null);
        self.inner.lock().push(result.clone());
        Box::pin(async move { Ok(result) })
    }
}

/// Local tool adapter. The adapter holds a [`LocalToolDispatcher`]
/// and rejects any request that would route outside the local
/// runtime.
#[derive(Clone)]
pub struct LocalToolAdapter {
    dispatcher: Arc<dyn LocalToolDispatcher>,
}

impl LocalToolAdapter {
    /// Build a new adapter against the given dispatcher.
    pub fn new(dispatcher: Arc<dyn LocalToolDispatcher>) -> Self {
        Self { dispatcher }
    }

    /// Build the default adapter (in-memory, no-op).
    pub fn default_in_memory() -> Self {
        Self::new(Arc::new(InMemoryToolDispatcher::new()))
    }

    /// Dispatch a single tool call. The adapter refuses any call
    /// whose identifier starts with `http`, `https`, `remote`,
    /// `remote-host.` or `tcp:` (the prefix heuristic is documented
    /// in the T126 review notes).
    pub async fn dispatch(
        &self,
        request: ToolCallRequest,
    ) -> Result<ToolCallResult, ToolAdapterError> {
        let forbidden = ["http://", "https://", "remote://", "remote-host.", "tcp:"];
        if forbidden
            .iter()
            .any(|prefix| request.identifier.starts_with(prefix))
        {
            return Err(ToolAdapterError::RemoteForbidden(request.identifier));
        }
        self.dispatcher.dispatch(request).await
    }
}

/// Convert a [`ToolAdapterError`] to the public execution
/// [`FailureCategory`] for the activity log.
pub fn tool_error_to_failure(error: &ToolAdapterError) -> FailureCategory {
    match error {
        ToolAdapterError::RemoteForbidden(_) => FailureCategory::Auth,
        ToolAdapterError::NotFound(_) => FailureCategory::NotFound,
        ToolAdapterError::NonTerminal(_) => FailureCategory::Internal,
        ToolAdapterError::Cancelled(_) => FailureCategory::Internal,
        ToolAdapterError::AtCapacity => FailureCategory::Internal,
        ToolAdapterError::Backend(_) => FailureCategory::Internal,
    }
}

/// Convert a [`LocalExecutionOutcome`] to a [`ToolCallResult`].
pub fn outcome_to_result(call_id: &str, outcome: LocalExecutionOutcome) -> ToolCallResult {
    match outcome {
        LocalExecutionOutcome::Completed => {
            ToolCallResult::success(call_id.to_string(), Value::Null)
        }
        LocalExecutionOutcome::Failed(category) => {
            ToolCallResult::failure(call_id.to_string(), format!("{:?}", category))
        }
        LocalExecutionOutcome::Cancelled => {
            ToolCallResult::failure(call_id.to_string(), "cancelled")
        }
    }
}
