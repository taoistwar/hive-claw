//! T126 local tool-calling adapter.
//!
//! The local agent runtime uses this adapter to dispatch
//! Function/Workflow/Plugin tool calls. The adapter never routes to
//! a remote or network target; a request that would require such a
//! route is rejected with [`ToolAdapterError::RemoteForbidden`].

#![warn(missing_docs)]

use std::{collections::BTreeSet, future::Future, path::PathBuf, pin::Pin, sync::Arc};

use hive_json_schema::CompiledJsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{FromRow, Pool, Sqlite};
use thiserror::Error;
use uuid::Uuid;

use crate::datasource::function_store::FunctionStore;
use crate::datasource::workflow_store::WorkflowStore;
use crate::datasource::{Crypto, Store};
use crate::runtime::execution::CancelHandle;
use crate::runtime::execution::{FailureCategory, LocalExecutionOutcome};
use crate::runtime::workflow_executor::{LocalWorkflowNodeExecutor, WorkflowExecutor};

/// Future returned by one local persisted-Tool target runner.
pub type ToolTargetFuture =
    Pin<Box<dyn Future<Output = Result<Value, ToolExecutionError>> + Send + 'static>>;

/// Injected local Function/Workflow execution boundary.
pub trait ToolTargetRunner: Send + Sync {
    /// Execute one Function target locally.
    fn execute_function(
        &self,
        function_id: i64,
        input: Value,
        granted_capabilities: Vec<String>,
    ) -> ToolTargetFuture;
    /// Execute one Workflow target locally.
    fn execute_workflow(
        &self,
        workflow_id: i64,
        input: Value,
        granted_capabilities: Vec<String>,
    ) -> ToolTargetFuture;
}

/// Production local target runner used by the Agent Tool loop.
///
/// Builtin and Custom Functions execute through the same managed
/// [`crate::runtime::function_test_executor::FunctionTestExecutor`] boundary
/// as the Function UI. Workflows load their persisted graph and execute every
/// node through [`LocalWorkflowNodeExecutor`]. Neither branch contains a
/// HiveWeb client or remote fallback.
#[derive(Clone)]
pub struct LocalPersistedToolTargetRunner {
    pool: Pool<Sqlite>,
    plugin_root: PathBuf,
    crypto: Crypto,
}

impl LocalPersistedToolTargetRunner {
    /// Bind the runner to one opened desktop-local Store.
    pub fn from_store(store: &Store) -> Self {
        Self {
            pool: store.pool().clone(),
            plugin_root: store.plugin_root().to_path_buf(),
            crypto: store.crypto().clone(),
        }
    }
}

impl ToolTargetRunner for LocalPersistedToolTargetRunner {
    fn execute_function(
        &self,
        function_id: i64,
        input: Value,
        granted_capabilities: Vec<String>,
    ) -> ToolTargetFuture {
        let pool = self.pool.clone();
        let plugin_root = self.plugin_root.clone();
        Box::pin(async move {
            let function = FunctionStore::new(pool.clone())
                .map_err(|_| ToolExecutionError::TargetFailed)?
                .get(function_id)
                .await
                .map_err(|_| ToolExecutionError::TargetFailed)?
                .ok_or(ToolExecutionError::TargetFailed)?
                .into_legacy_entity();
            let rendered = crate::runtime::function_test_executor::FunctionTestExecutor::new(
                plugin_root,
                pool,
            )
            .execute_with_capabilities(&function, input, granted_capabilities)
            .await
            .map_err(|_| ToolExecutionError::TargetFailed)?;
            serde_json::from_str(&rendered).map_err(|_| ToolExecutionError::TargetFailed)
        })
    }

    fn execute_workflow(
        &self,
        workflow_id: i64,
        input: Value,
        granted_capabilities: Vec<String>,
    ) -> ToolTargetFuture {
        let pool = self.pool.clone();
        let plugin_root = self.plugin_root.clone();
        let crypto = self.crypto.clone();
        Box::pin(async move {
            let name = sqlx::query_scalar::<_, String>("SELECT name FROM workflows WHERE id = ?")
                .bind(workflow_id)
                .fetch_optional(&pool)
                .await
                .map_err(|_| ToolExecutionError::TargetFailed)?
                .ok_or(ToolExecutionError::TargetFailed)?;
            let graph = WorkflowStore::new(pool.clone())
                .map_err(|_| ToolExecutionError::TargetFailed)?
                .load_graph(workflow_id, name)
                .await
                .map_err(|_| ToolExecutionError::TargetFailed)?;
            let executor = WorkflowExecutor::new(
                LocalWorkflowNodeExecutor::new(pool, plugin_root, crypto)
                    .with_capabilities(granted_capabilities),
            );
            let outcome = executor
                .execute(&graph, input, CancelHandle::new())
                .await
                .map_err(|_| ToolExecutionError::TargetFailed)?;
            outcome
                .node_results
                .last()
                .map(|result| result.output.clone())
                .ok_or(ToolExecutionError::TargetFailed)
        })
    }
}

/// Immutable Capability snapshot for one persisted Tool invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolExecutionContext {
    granted_capabilities: BTreeSet<String>,
}

impl ToolExecutionContext {
    /// Construct a snapshot. Capability ordering does not grant extra access;
    /// membership is evaluated against the persisted declaration.
    pub fn new(granted_capabilities: Vec<String>) -> Self {
        Self {
            granted_capabilities: granted_capabilities.into_iter().collect(),
        }
    }

    fn grants(&self, capability: &str) -> bool {
        self.granted_capabilities.contains(capability)
    }

    fn granted(&self) -> Vec<String> {
        self.granted_capabilities.iter().cloned().collect()
    }
}

/// Stable, non-leaking persisted Tool execution failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ToolExecutionError {
    /// Persisted Tool id does not exist.
    #[error("not_found")]
    NotFound,
    /// A Function target is schema-only and cannot execute.
    #[error("function_not_executable")]
    FunctionNotExecutable,
    /// Input violates the persisted input JSON Schema.
    #[error("input_schema_mismatch")]
    InputSchemaMismatch,
    /// Execution snapshot lacks a required Capability.
    #[error("capability_denied")]
    CapabilityDenied,
    /// Target output violates the persisted output JSON Schema.
    #[error("output_schema_mismatch")]
    OutputSchemaMismatch,
    /// Persisted kind, target XOR, schema, or capability metadata is invalid.
    #[error("invalid_persisted_tool")]
    InvalidPersistedTool,
    /// The local target failed without exposing backend text.
    #[error("target_failed")]
    TargetFailed,
}

impl ToolExecutionError {
    /// Stable public error code.
    pub const fn code(self) -> &'static str {
        match self {
            Self::NotFound => "not_found",
            Self::FunctionNotExecutable => "function_not_executable",
            Self::InputSchemaMismatch => "input_schema_mismatch",
            Self::CapabilityDenied => "capability_denied",
            Self::OutputSchemaMismatch => "output_schema_mismatch",
            Self::InvalidPersistedTool => "invalid_persisted_tool",
            Self::TargetFailed => "target_failed",
        }
    }
}

#[derive(Debug, FromRow)]
struct PersistedToolRow {
    kind: String,
    function_id: Option<i64>,
    workflow_id: Option<i64>,
    input_schema: String,
    output_schema: String,
    required_capabilities: Option<String>,
}

/// Production persisted Tool executor. The executor loads the canonical local
/// row, validates policy and schemas, then invokes exactly one injected local
/// Function or Workflow target. No remote fallback exists.
#[derive(Clone)]
pub struct PersistedToolExecutor {
    pool: Pool<Sqlite>,
    runner: Arc<dyn ToolTargetRunner>,
}

impl PersistedToolExecutor {
    /// Construct the local executor over the canonical Store pool.
    pub fn new(pool: Pool<Sqlite>, runner: Arc<dyn ToolTargetRunner>) -> Self {
        Self { pool, runner }
    }

    /// Execute one persisted Tool through the strict validation sequence.
    pub async fn execute(
        &self,
        tool_id: i64,
        input: Value,
        context: ToolExecutionContext,
    ) -> Result<Value, ToolExecutionError> {
        if tool_id <= 0 {
            return Err(ToolExecutionError::NotFound);
        }
        let tool = sqlx::query_as::<_, PersistedToolRow>(
            "SELECT kind, function_id, workflow_id, input_schema, output_schema, \
                    required_capabilities FROM tools WHERE id = ?",
        )
        .bind(tool_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| ToolExecutionError::InvalidPersistedTool)?
        .ok_or(ToolExecutionError::NotFound)?;

        let target = match (tool.kind.as_str(), tool.function_id, tool.workflow_id) {
            ("function-wrap", Some(function_id), None) => {
                let kind =
                    sqlx::query_scalar::<_, String>("SELECT kind FROM functions WHERE id = ?")
                        .bind(function_id)
                        .fetch_optional(&self.pool)
                        .await
                        .map_err(|_| ToolExecutionError::InvalidPersistedTool)?
                        .ok_or(ToolExecutionError::InvalidPersistedTool)?;
                if kind == "placeholder" {
                    return Err(ToolExecutionError::FunctionNotExecutable);
                }
                if !matches!(kind.as_str(), "builtin" | "custom") {
                    return Err(ToolExecutionError::InvalidPersistedTool);
                }
                PersistedTarget::Function(function_id)
            }
            ("workflow-wrap", None, Some(workflow_id)) => PersistedTarget::Workflow(workflow_id),
            _ => return Err(ToolExecutionError::InvalidPersistedTool),
        };

        validate_json_schema(
            &tool.input_schema,
            &input,
            ToolExecutionError::InputSchemaMismatch,
        )?;
        let required = tool
            .required_capabilities
            .as_deref()
            .map(serde_json::from_str::<Vec<String>>)
            .transpose()
            .map_err(|_| ToolExecutionError::InvalidPersistedTool)?
            .unwrap_or_default();
        let mut declared = BTreeSet::new();
        for capability in required {
            if capability.trim() != capability
                || capability.is_empty()
                || !declared.insert(capability.clone())
            {
                return Err(ToolExecutionError::InvalidPersistedTool);
            }
            if !context.grants(&capability) {
                return Err(ToolExecutionError::CapabilityDenied);
            }
        }

        let granted_capabilities = context.granted();
        let output = match target {
            PersistedTarget::Function(function_id) => {
                self.runner
                    .execute_function(function_id, input, granted_capabilities)
                    .await
            }
            PersistedTarget::Workflow(workflow_id) => {
                self.runner
                    .execute_workflow(workflow_id, input, granted_capabilities)
                    .await
            }
        }
        .map_err(|_| ToolExecutionError::TargetFailed)?;
        validate_json_schema(
            &tool.output_schema,
            &output,
            ToolExecutionError::OutputSchemaMismatch,
        )?;
        Ok(output)
    }
}

enum PersistedTarget {
    Function(i64),
    Workflow(i64),
}

fn validate_json_schema(
    schema: &str,
    value: &Value,
    mismatch: ToolExecutionError,
) -> Result<(), ToolExecutionError> {
    let schema = serde_json::from_str::<Value>(schema)
        .map_err(|_| ToolExecutionError::InvalidPersistedTool)?;
    let compiled = CompiledJsonSchema::compile(&schema)
        .map_err(|_| ToolExecutionError::InvalidPersistedTool)?;
    compiled.validate(value).map_err(|_| mismatch)
}

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
