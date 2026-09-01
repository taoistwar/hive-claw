//! T102 [P] [US11] persisted Tool dispatch supplemental Red.
//!
//! This contract loads one persisted Tool, validates input and Capability
//! policy before any target starts, routes the XOR target locally, validates
//! output, and returns stable non-leaking errors.

mod support;

use std::sync::{Arc, Mutex};

use hivegui::{
    datasource::store::{Store, StoreOpenOptions},
    runtime::tool_adapter::{
        PersistedToolExecutor, ToolExecutionContext, ToolExecutionError, ToolTargetFuture,
        ToolTargetRunner,
    },
};
use serde_json::{Value, json};
use support::TestWorkspace;

const INPUT_SCHEMA: &str =
    r#"{"type":"object","properties":{"value":{"type":"string"}},"required":["value"]}"#;
const OUTPUT_SCHEMA: &str =
    r#"{"type":"object","properties":{"result":{"type":"string"}},"required":["result"]}"#;
const FIXTURE_TIME: &str = "2026-08-25T00:00:00Z";

#[derive(Debug, Clone, PartialEq)]
enum TargetCall {
    Function(i64, Value),
    Workflow(i64, Value),
}

#[derive(Clone)]
struct RecordingTargetRunner {
    calls: Arc<Mutex<Vec<TargetCall>>>,
    output: Value,
}

impl RecordingTargetRunner {
    fn returning(output: Value) -> Self {
        Self {
            calls: Arc::new(Mutex::new(Vec::new())),
            output,
        }
    }

    fn calls(&self) -> Vec<TargetCall> {
        self.calls.lock().expect("target call lock").clone()
    }
}

impl ToolTargetRunner for RecordingTargetRunner {
    fn execute_function(
        &self,
        function_id: i64,
        input: Value,
        _granted_capabilities: Vec<String>,
    ) -> ToolTargetFuture {
        self.calls
            .lock()
            .expect("target call lock")
            .push(TargetCall::Function(function_id, input));
        let output = self.output.clone();
        Box::pin(async move { Ok(output) })
    }

    fn execute_workflow(
        &self,
        workflow_id: i64,
        input: Value,
        _granted_capabilities: Vec<String>,
    ) -> ToolTargetFuture {
        self.calls
            .lock()
            .expect("target call lock")
            .push(TargetCall::Workflow(workflow_id, input));
        let output = self.output.clone();
        Box::pin(async move { Ok(output) })
    }
}

async fn open_store(workspace: &TestWorkspace) -> Store {
    Store::open_local(StoreOpenOptions::new(
        workspace.database_path(),
        workspace.plugin_root(),
    ))
    .await
    .expect("open canonical v4 Store")
}

async fn seed_function(store: &Store, identifier: &str, kind: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(concat!(
        "INSERT INTO functions (identifier, name, description, kind, input_schema, ",
        "output_schema, plugin_id, plugin_export, category_id, required_capabilities, ",
        "created_at, updated_at) ",
        "VALUES (?, ?, '', ?, ?, ?, NULL, NULL, NULL, NULL, ?, ?) RETURNING id"
    ))
    .bind(identifier)
    .bind(identifier)
    .bind(kind)
    .bind(INPUT_SCHEMA)
    .bind(OUTPUT_SCHEMA)
    .bind(FIXTURE_TIME)
    .bind(FIXTURE_TIME)
    .fetch_one(store.pool())
    .await
    .expect("seed Function target")
}

async fn seed_workflow(store: &Store, identifier: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(concat!(
        "INSERT INTO workflows (identifier, name, description, timeout_ms, input_schema, ",
        "start_description, output_schema, required_capabilities, created_at, updated_at) ",
        "VALUES (?, ?, '', 30000, ?, '', ?, NULL, ?, ?) RETURNING id"
    ))
    .bind(identifier)
    .bind(identifier)
    .bind(INPUT_SCHEMA)
    .bind(OUTPUT_SCHEMA)
    .bind(FIXTURE_TIME)
    .bind(FIXTURE_TIME)
    .fetch_one(store.pool())
    .await
    .expect("seed Workflow target")
}

async fn seed_tool(
    store: &Store,
    identifier: &str,
    kind: &str,
    function_id: Option<i64>,
    workflow_id: Option<i64>,
    required_capabilities: Option<&str>,
) -> i64 {
    sqlx::query_scalar::<_, i64>(concat!(
        "INSERT INTO tools (identifier, name, description, kind, source, is_always, ",
        "function_id, workflow_id, input_schema, output_schema, category_id, ",
        "required_capabilities, created_at, updated_at) ",
        "VALUES (?, ?, '', ?, 'workspace', 0, ?, ?, ?, ?, NULL, ?, ?, ?) RETURNING id"
    ))
    .bind(identifier)
    .bind(identifier)
    .bind(kind)
    .bind(function_id)
    .bind(workflow_id)
    .bind(INPUT_SCHEMA)
    .bind(OUTPUT_SCHEMA)
    .bind(required_capabilities)
    .bind(FIXTURE_TIME)
    .bind(FIXTURE_TIME)
    .fetch_one(store.pool())
    .await
    .expect("seed persisted Tool")
}

fn context(capabilities: &[&str]) -> ToolExecutionContext {
    ToolExecutionContext::new(capabilities.iter().map(|value| value.to_string()).collect())
}

#[tokio::test(flavor = "current_thread")]
async fn input_schema_rejection_happens_before_any_target_starts() {
    let workspace = TestWorkspace::new().expect("workspace");
    let store = open_store(&workspace).await;
    let function_id = seed_function(&store, "dispatch_input_function", "builtin").await;
    let tool_id = seed_tool(
        &store,
        "dispatch_input_tool",
        "function-wrap",
        Some(function_id),
        None,
        None,
    )
    .await;
    let runner = Arc::new(RecordingTargetRunner::returning(json!({"result":"unused"})));
    let executor = PersistedToolExecutor::new(store.pool().clone(), runner.clone());

    let error = executor
        .execute(tool_id, json!({"value": 7}), context(&[]))
        .await
        .expect_err("wrong input type must fail");
    assert_eq!(error.code(), "input_schema_mismatch");
    assert!(runner.calls().is_empty(), "schema failure started a target");
}

#[tokio::test(flavor = "current_thread")]
async fn capability_denial_happens_before_any_target_starts() {
    let workspace = TestWorkspace::new().expect("workspace");
    let store = open_store(&workspace).await;
    let function_id = seed_function(&store, "dispatch_capability_function", "builtin").await;
    let tool_id = seed_tool(
        &store,
        "dispatch_capability_tool",
        "function-wrap",
        Some(function_id),
        None,
        Some(r#"["log.emit"]"#),
    )
    .await;
    let runner = Arc::new(RecordingTargetRunner::returning(json!({"result":"unused"})));
    let executor = PersistedToolExecutor::new(store.pool().clone(), runner.clone());

    let error = executor
        .execute(tool_id, json!({"value":"ok"}), context(&[]))
        .await
        .expect_err("missing Capability must fail");
    assert_eq!(error.code(), "capability_denied");
    assert!(
        runner.calls().is_empty(),
        "Capability denial started a target"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn function_and_workflow_xor_targets_route_once_and_locally() {
    let workspace = TestWorkspace::new().expect("workspace");
    let store = open_store(&workspace).await;
    let function_id = seed_function(&store, "dispatch_function", "builtin").await;
    let workflow_id = seed_workflow(&store, "dispatch_workflow").await;
    let function_tool = seed_tool(
        &store,
        "dispatch_function_tool",
        "function-wrap",
        Some(function_id),
        None,
        None,
    )
    .await;
    let workflow_tool = seed_tool(
        &store,
        "dispatch_workflow_tool",
        "workflow-wrap",
        None,
        Some(workflow_id),
        None,
    )
    .await;
    let runner = Arc::new(RecordingTargetRunner::returning(json!({"result":"ok"})));
    let executor = PersistedToolExecutor::new(store.pool().clone(), runner.clone());

    assert_eq!(
        executor
            .execute(function_tool, json!({"value":"function"}), context(&[]))
            .await
            .expect("function dispatch"),
        json!({"result":"ok"})
    );
    assert_eq!(
        executor
            .execute(workflow_tool, json!({"value":"workflow"}), context(&[]))
            .await
            .expect("workflow dispatch"),
        json!({"result":"ok"})
    );
    assert_eq!(
        runner.calls(),
        vec![
            TargetCall::Function(function_id, json!({"value":"function"})),
            TargetCall::Workflow(workflow_id, json!({"value":"workflow"})),
        ]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn placeholder_function_fails_before_capability_or_target_lookup() {
    let workspace = TestWorkspace::new().expect("workspace");
    let store = open_store(&workspace).await;
    let function_id = seed_function(&store, "dispatch_placeholder", "placeholder").await;
    let tool_id = seed_tool(
        &store,
        "dispatch_placeholder_tool",
        "function-wrap",
        Some(function_id),
        None,
        Some(r#"["log.emit"]"#),
    )
    .await;
    let runner = Arc::new(RecordingTargetRunner::returning(json!({"result":"unused"})));
    let executor = PersistedToolExecutor::new(store.pool().clone(), runner.clone());

    let error = executor
        .execute(tool_id, json!({"value":"ok"}), context(&[]))
        .await
        .expect_err("Placeholder Function must never execute");
    assert_eq!(error.code(), "function_not_executable");
    assert!(
        runner.calls().is_empty(),
        "Placeholder reached the target runner"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn output_schema_failure_is_stable_and_does_not_leak_backend_text() {
    let workspace = TestWorkspace::new().expect("workspace");
    let store = open_store(&workspace).await;
    let function_id = seed_function(&store, "dispatch_output_function", "builtin").await;
    let tool_id = seed_tool(
        &store,
        "dispatch_output_tool",
        "function-wrap",
        Some(function_id),
        None,
        None,
    )
    .await;
    let runner = Arc::new(RecordingTargetRunner::returning(json!({"result":7})));
    let executor = PersistedToolExecutor::new(store.pool().clone(), runner.clone());

    let error: ToolExecutionError = executor
        .execute(tool_id, json!({"value":"ok"}), context(&[]))
        .await
        .expect_err("wrong output type must fail");
    assert_eq!(error.code(), "output_schema_mismatch");
    let public = error.to_string().to_ascii_lowercase();
    for forbidden in ["sqlx", "sqlite", "select ", "insert ", "database"] {
        assert!(
            !public.contains(forbidden),
            "public Tool execution error leaked backend detail: {public}"
        );
    }
    assert_eq!(
        runner.calls(),
        vec![TargetCall::Function(function_id, json!({"value":"ok"}))]
    );
}
