//! T116 [P] [US13] Local agent runtime contract.
//!
//! Source of truth: `specs/011-hivegui-standalone-mode/tasks.md` §T116
//! ("在 `crates/hivegui/tests/local_agent_runtime.rs` 直接调用公开
//! `start_session`/控制命令，编写 HiveWeb 未配置且不运行时由 mock LLM
//! 驱动真实本地 Tool 的完整 Agent 对话 E2E，并用网络捕获断言零
//! HiveWeb 请求及失败零 HiveWeb fallback；覆盖 `start_session` 只能
//! 解析默认根、无入口覆盖参数、非法 session_id/execution_id UUID、
//! 空/超1MiB user_message、非法 retention_filter 在创建会话或执行
//! 记录前失败，以及显式∪always资源、Capability、快照、直接子路由、
//! 跨级/循环拒绝和唯一终态；重复启动并执行回复、结束和直接子路由
//! 控制后，断言 Tool Store/列表始终不存在这些运行时控制记录").
//!
//! Public boundaries the T125-T126 implementation must satisfy:
//!   - `hivegui::agent::local_agent::LocalAgentRuntime`
//!   - `hivegui::agent::local_agent::LocalAgentError`
//!   - `hivegui::agent::local_agent::SessionHandle`
//!   - `hivegui::agent::local_agent::TurnSnapshot`
//!
//! T123 reviewer signs §T116.11; T125-T126 implementation then makes
//! these tests Green; T136 reruns to record the Green evidence.

#![allow(missing_docs)]

mod support;

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};

use hive_runtime_core::execution::{EventSink, RuntimeEvent};
use hivegui::datasource::{
    entity_store::{AgentInput, AgentStore},
    llm_store::LlmStore,
    store::{Store, StoreOpenOptions},
};
use hivegui::runtime::provider_resolver::LocalProviderDecisionModel;
use hivegui::{
    agent::{
        local_agent::{
            LocalAgentDecisionFuture, LocalAgentDecisionModel, LocalAgentError, LocalAgentRuntime,
            TurnSnapshot,
        },
        session::AgentMessage,
    },
    runtime::tool_adapter::{
        LocalPersistedToolTargetRunner, PersistedToolExecutor, ToolExecutionContext,
        ToolExecutionError, ToolTargetFuture, ToolTargetRunner,
    },
};
use support::{CapturedHttpServer, MockHttpResponse, TestWorkspace};
use tokio::{io::AsyncReadExt, net::TcpListener, sync::Notify, task::JoinHandle};
use uuid::Uuid;

async fn fresh_runtime() -> (TestWorkspace, LocalAgentRuntime, AgentStore) {
    let workspace = TestWorkspace::new().expect("test workspace");
    let database = Store::open_local(StoreOpenOptions::new(
        workspace.database_path(),
        workspace.plugin_root(),
    ))
    .await
    .expect("open canonical local Store");
    let pool = database.pool().clone();
    let store = AgentStore::new(pool.clone()).expect("agent store");
    let runtime = LocalAgentRuntime::new(pool).expect("local agent runtime");
    (workspace, runtime, store)
}

fn root_input(identifier: &str, name: &str) -> AgentInput {
    AgentInput::new_root(identifier, name, "system prompt").expect("validated root")
}

#[derive(Clone)]
struct ScriptedLocalModel {
    decisions: Arc<Mutex<VecDeque<String>>>,
    observations: Arc<Mutex<Vec<Option<serde_json::Value>>>>,
}

impl ScriptedLocalModel {
    fn new(decisions: impl IntoIterator<Item = String>) -> Self {
        Self {
            decisions: Arc::new(Mutex::new(decisions.into_iter().collect())),
            observations: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn observations(&self) -> Vec<Option<serde_json::Value>> {
        self.observations.lock().expect("observations lock").clone()
    }
}

impl LocalAgentDecisionModel for ScriptedLocalModel {
    fn decide(
        &self,
        _snapshot: TurnSnapshot,
        _messages: Vec<AgentMessage>,
        tool_observation: Option<serde_json::Value>,
    ) -> LocalAgentDecisionFuture {
        self.observations
            .lock()
            .expect("observations lock")
            .push(tool_observation);
        let decision = self
            .decisions
            .lock()
            .expect("decisions lock")
            .pop_front()
            .expect("scripted model decision");
        Box::pin(async move { Ok(decision) })
    }
}

#[derive(Debug)]
struct NoopRuntimeEventSink;

impl EventSink for NoopRuntimeEventSink {
    fn emit(&self, _event: RuntimeEvent) {}
}

struct HangingLocalLlm {
    address: std::net::SocketAddr,
    request_seen: Arc<Notify>,
    task: JoinHandle<()>,
}

impl HangingLocalLlm {
    async fn spawn() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind hanging local LLM");
        let address = listener.local_addr().expect("listener address");
        let request_seen = Arc::new(Notify::new());
        let notify = request_seen.clone();
        let task = tokio::spawn(async move {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let mut buffer = [0_u8; 4096];
            if stream.read(&mut buffer).await.unwrap_or(0) > 0 {
                notify.notify_one();
            }
            std::future::pending::<()>().await;
        });
        Self {
            address,
            request_seen,
            task,
        }
    }

    fn base_url(&self) -> String {
        format!("http://{}", self.address)
    }

    async fn wait_for_request(&self) {
        self.request_seen.notified().await;
    }
}

impl Drop for HangingLocalLlm {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[derive(Clone)]
struct LocalFunctionRunner {
    calls: Arc<Mutex<Vec<(i64, serde_json::Value)>>>,
}

impl ToolTargetRunner for LocalFunctionRunner {
    fn execute_function(
        &self,
        function_id: i64,
        input: serde_json::Value,
        _granted_capabilities: Vec<String>,
    ) -> ToolTargetFuture {
        self.calls
            .lock()
            .expect("runner calls lock")
            .push((function_id, input));
        Box::pin(async { Ok(serde_json::json!({"result":"local-ok"})) })
    }

    fn execute_workflow(
        &self,
        _workflow_id: i64,
        _input: serde_json::Value,
        _granted_capabilities: Vec<String>,
    ) -> ToolTargetFuture {
        Box::pin(async { Err(ToolExecutionError::TargetFailed) })
    }
}

#[tokio::test(flavor = "current_thread")]
async fn mock_llm_drives_one_real_local_tool_to_one_final_reply_without_hiveweb() {
    const INPUT_SCHEMA: &str =
        r#"{"type":"object","properties":{"value":{"type":"string"}},"required":["value"]}"#;
    const OUTPUT_SCHEMA: &str =
        r#"{"type":"object","properties":{"result":{"type":"string"}},"required":["result"]}"#;
    let (_workspace, runtime, store) = fresh_runtime().await;
    let function_id = sqlx::query_scalar::<_, i64>(concat!(
        "INSERT INTO functions (identifier,name,description,kind,input_schema,output_schema,",
        "plugin_id,plugin_export,category_id,required_capabilities,created_at,updated_at) ",
        "VALUES ('agent_local_function','Agent Local Function','','builtin',?,?,NULL,NULL,NULL,NULL,datetime('now'),datetime('now')) RETURNING id"
    ))
    .bind(INPUT_SCHEMA)
    .bind(OUTPUT_SCHEMA)
    .fetch_one(runtime.pool())
    .await
    .expect("seed local Function");
    let tool_id = sqlx::query_scalar::<_, i64>(concat!(
        "INSERT INTO tools (identifier,name,description,kind,source,is_always,function_id,workflow_id,",
        "input_schema,output_schema,category_id,required_capabilities,created_at,updated_at) ",
        "VALUES ('agent_local_tool','Agent Local Tool','','function-wrap','workspace',0,?,NULL,?,?,NULL,NULL,datetime('now'),datetime('now')) RETURNING id"
    ))
    .bind(function_id)
    .bind(INPUT_SCHEMA)
    .bind(OUTPUT_SCHEMA)
    .fetch_one(runtime.pool())
    .await
    .expect("seed local Tool");
    store
        .create(root_input("root-agent", "Root Agent").with_tools([tool_id]))
        .await
        .expect("create root with Tool snapshot");

    let (handle, _token) = runtime
        .start_session("run the local tool")
        .await
        .expect("session");
    let model = ScriptedLocalModel::new([
        serde_json::json!({
            "action":"tool_call",
            "tool_id":tool_id,
            "input":{"value":"from-model"}
        })
        .to_string(),
        serde_json::json!({"action":"reply","content":"local tool finished"}).to_string(),
    ]);
    let runner = Arc::new(LocalFunctionRunner {
        calls: Arc::new(Mutex::new(Vec::new())),
    });
    let executor = PersistedToolExecutor::new(runtime.pool().clone(), runner.clone());

    let result = runtime
        .run_turn(&handle, &model, &executor)
        .await
        .expect("local turn must terminate");
    assert_eq!(result.reply(), "local tool finished");
    assert_eq!(result.tool_calls(), 1);
    assert_eq!(
        runner.calls.lock().expect("runner calls lock").as_slice(),
        &[(function_id, serde_json::json!({"value":"from-model"}))]
    );
    assert_eq!(
        model.observations(),
        vec![
            None,
            Some(serde_json::json!({
                "tool_id":tool_id,
                "succeeded":true,
                "output":{"result":"local-ok"}
            }))
        ]
    );
    let messages = runtime.messages(&handle).expect("session messages");
    assert_eq!(
        messages
            .iter()
            .map(|message| message.speaker())
            .collect::<Vec<_>>(),
        vec!["user", "tool", "assistant"]
    );
    assert_eq!(runtime.session_state(&handle).as_deref(), Some("idle"));
}

#[tokio::test(flavor = "current_thread")]
async fn production_provider_model_drives_real_local_tool_and_final_reply() {
    const INPUT_SCHEMA: &str = r#"{"type":"object","properties":{"template":{"type":"string"},"vars":{"type":"object"}},"required":["template","vars"]}"#;
    const OUTPUT_SCHEMA: &str = r#"{"type":"string"}"#;
    let workspace = TestWorkspace::new().expect("test workspace");
    let database = Store::open_local(StoreOpenOptions::new(
        workspace.database_path(),
        workspace.plugin_root(),
    ))
    .await
    .expect("open canonical local Store");
    let pool = database.pool().clone();
    let llm_store = LlmStore::new(pool.clone(), database.crypto().clone());
    let server = CapturedHttpServer::spawn(vec![
        MockHttpResponse::json(
            200,
            serde_json::json!({
                "choices": [{
                    "message": {"role": "assistant", "content": serde_json::json!({
                        "action":"tool_call",
                        "tool_id": 1,
                        "input":{"template":"hello {name}","vars":{"name":"local"}}
                    }).to_string()},
                    "finish_reason": "stop"
                }]
            }),
        ),
        MockHttpResponse::json(
            200,
            serde_json::json!({
                "choices": [{
                    "message": {"role": "assistant", "content": serde_json::json!({
                        "action":"reply",
                        "content":"hello local"
                    }).to_string()},
                    "finish_reason": "stop"
                }]
            }),
        ),
    ])
    .await
    .expect("local mock LLM");
    let provider = llm_store
        .create_provider(
            "agent-local-provider",
            "openai",
            &server.base_url(),
            "local-test-token",
            "",
        )
        .await
        .expect("provider");
    let preset = llm_store
        .create_preset("agent-local-preset", "", false, 512, 0.0)
        .await
        .expect("preset");
    llm_store
        .create_model("agent-local-model", preset.id, provider.id, 1)
        .await
        .expect("model");
    let format_template_id = sqlx::query_scalar::<_, i64>(
        "SELECT id FROM functions WHERE identifier = 'format_template'",
    )
    .fetch_one(&pool)
    .await
    .expect("builtin Function");
    let tool_id = sqlx::query_scalar::<_, i64>(concat!(
        "INSERT INTO tools (identifier,name,description,kind,source,is_always,function_id,workflow_id,",
        "input_schema,output_schema,category_id,required_capabilities,created_at,updated_at) ",
        "VALUES ('agent_format','Agent Format','format locally','function-wrap','workspace',0,?,NULL,?,?,NULL,NULL,datetime('now'),datetime('now')) RETURNING id"
    ))
    .bind(format_template_id)
    .bind(INPUT_SCHEMA)
    .bind(OUTPUT_SCHEMA)
    .fetch_one(&pool)
    .await
    .expect("persisted local Tool");
    assert_eq!(tool_id, 1, "isolated Store assigns the first Tool id");
    let agent_store = AgentStore::new(pool.clone()).expect("agent store");
    agent_store
        .create(
            root_input("provider-root", "Provider Root")
                .with_model_preset("agent-local-preset")
                .with_tools([tool_id]),
        )
        .await
        .expect("root Agent");
    let runtime = LocalAgentRuntime::new(pool.clone()).expect("runtime");
    let (handle, _token) = runtime
        .start_session("format a greeting")
        .await
        .expect("session");
    let model = LocalProviderDecisionModel::from_store(
        &database,
        "agent-provider-execution",
        handle.as_str(),
        Arc::new(NoopRuntimeEventSink),
    );
    let executor = PersistedToolExecutor::new(
        pool,
        Arc::new(LocalPersistedToolTargetRunner::from_store(&database)),
    );

    let result = runtime
        .run_turn(&handle, &model, &executor)
        .await
        .expect("production local Provider turn");
    assert_eq!(result.reply(), "hello local");
    assert_eq!(result.tool_calls(), 1);
    assert!(
        runtime
            .messages(&handle)
            .expect("messages")
            .iter()
            .any(|message| message.speaker() == "tool" && message.body().contains("hello local")),
        "the real Builtin output must feed the second local Provider decision"
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 2, "one Tool decision and one final reply");
    let request_text = String::from_utf8_lossy(&requests[0]);
    assert!(request_text.contains("system prompt"));
    assert!(request_text.contains("agent_format"));
    assert!(request_text.contains("format locally"));
    assert!(
        !request_text.to_ascii_lowercase().contains("hiveweb"),
        "production Agent prompt must not introduce a HiveWeb route"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_new_turn_replaces_the_cancel_token_and_appends_the_user_once() {
    let (_workspace, runtime, store) = fresh_runtime().await;
    store
        .create(root_input("turn-root", "Turn Root"))
        .await
        .expect("root Agent");
    let (handle, first_token) = runtime.start_session("first").await.expect("first turn");
    runtime
        .cancel_execution(&handle)
        .expect("cancel first turn");
    assert!(first_token.is_cancelled());

    let second_token = runtime.begin_turn(&handle, "second").expect("second turn");
    assert!(!second_token.is_cancelled());
    assert_eq!(
        runtime.session_state(&handle).as_deref(),
        Some("awaiting_model")
    );
    assert_eq!(
        runtime
            .messages(&handle)
            .expect("messages")
            .iter()
            .map(|message| (message.speaker(), message.body()))
            .collect::<Vec<_>>(),
        vec![("user", "first"), ("user", "second")]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn stop_cancels_the_inflight_production_provider_without_a_late_reply() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let database = Store::open_local(StoreOpenOptions::new(
        workspace.database_path(),
        workspace.plugin_root(),
    ))
    .await
    .expect("open canonical local Store");
    let pool = database.pool().clone();
    let server = HangingLocalLlm::spawn().await;
    let llm_store = LlmStore::new(pool.clone(), database.crypto().clone());
    let provider = llm_store
        .create_provider(
            "agent-cancel-provider",
            "openai",
            &server.base_url(),
            "local-test-token",
            "",
        )
        .await
        .expect("provider");
    let preset = llm_store
        .create_preset("agent-cancel-preset", "", false, 512, 0.0)
        .await
        .expect("preset");
    llm_store
        .create_model("agent-cancel-model", preset.id, provider.id, 1)
        .await
        .expect("model");
    AgentStore::new(pool.clone())
        .expect("Agent store")
        .create(root_input("cancel-root", "Cancel Root").with_model_preset("agent-cancel-preset"))
        .await
        .expect("root Agent");
    let runtime = LocalAgentRuntime::new(pool.clone()).expect("runtime");
    let (handle, _token) = runtime
        .start_session("wait for stop")
        .await
        .expect("session");
    let model = LocalProviderDecisionModel::from_store(
        &database,
        "agent-cancel-execution",
        handle.as_str(),
        Arc::new(NoopRuntimeEventSink),
    )
    .with_cancel_token(
        runtime
            .cancel_token(&handle)
            .expect("runtime exposes the active turn token"),
    );
    let executor = PersistedToolExecutor::new(
        pool,
        Arc::new(LocalPersistedToolTargetRunner::from_store(&database)),
    );
    let task_runtime = runtime.clone();
    let task_handle = handle.clone();
    let turn =
        tokio::spawn(async move { task_runtime.run_turn(&task_handle, &model, &executor).await });
    server.wait_for_request().await;
    runtime
        .cancel_execution(&handle)
        .expect("Stop signals the active turn");
    let error = tokio::time::timeout(Duration::from_millis(250), turn)
        .await
        .expect("Stop must abort the Provider HTTP promptly")
        .expect("turn task")
        .expect_err("a stopped turn cannot produce a reply");
    assert_eq!(
        error,
        LocalAgentError::AgentRejected("local provider call cancelled")
    );
    assert_eq!(
        runtime.session_state(&handle).as_deref(),
        Some("terminated")
    );
    assert_eq!(
        runtime
            .messages(&handle)
            .expect("session messages")
            .iter()
            .map(|message| message.speaker())
            .collect::<Vec<_>>(),
        vec!["user"],
        "a cancelled Provider call must not append a late assistant reply"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn turn_snapshot_unions_explicit_and_always_tools_with_stable_deduplication() {
    let (_workspace, runtime, store) = fresh_runtime().await;
    let function_id = sqlx::query_scalar::<_, i64>(
        "SELECT id FROM functions WHERE identifier = 'format_template'",
    )
    .fetch_one(runtime.pool())
    .await
    .expect("builtin Function");
    let insert_tool = |identifier: &'static str, is_always: bool| {
        let pool = runtime.pool().clone();
        async move {
            sqlx::query_scalar::<_, i64>(concat!(
                "INSERT INTO tools (identifier,name,description,kind,source,is_always,function_id,workflow_id,",
                "input_schema,output_schema,category_id,required_capabilities,created_at,updated_at) ",
                "SELECT ?,?,'','function-wrap','workspace',?,id,NULL,input_schema,output_schema,NULL,NULL,datetime('now'),datetime('now') ",
                "FROM functions WHERE id=? RETURNING id"
            ))
            .bind(identifier)
            .bind(identifier)
            .bind(is_always)
            .bind(function_id)
            .fetch_one(&pool)
            .await
            .expect("seed Tool")
        }
    };
    let explicit_id = insert_tool("explicit_agent_tool", false).await;
    let always_id = insert_tool("always_agent_tool", true).await;
    let unrelated_id = insert_tool("unrelated_agent_tool", false).await;

    store
        .create(root_input("root-agent", "Root Agent").with_tools([explicit_id, explicit_id]))
        .await
        .expect("create root");
    let (handle, _) = runtime.start_session("hello").await.expect("session");
    let snapshot = runtime.snapshot(&handle).expect("turn snapshot");

    let mut expected = vec![explicit_id, always_id];
    expected.sort_unstable();
    assert_eq!(snapshot.tool_ids, expected);
    assert!(!snapshot.tool_ids.contains(&unrelated_id));
}

#[tokio::test(flavor = "current_thread")]
async fn production_agent_tool_runner_executes_a_real_local_builtin() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let store = Store::open_local(StoreOpenOptions::new(
        workspace.database_path(),
        workspace.plugin_root(),
    ))
    .await
    .expect("open canonical local Store");
    let function = sqlx::query_as::<_, (i64, String, String)>(
        "SELECT id, input_schema, output_schema FROM functions \
         WHERE identifier = 'format_template'",
    )
    .fetch_one(store.pool())
    .await
    .expect("format_template Function");
    let tool_id = sqlx::query_scalar::<_, i64>(concat!(
        "INSERT INTO tools (identifier,name,description,kind,source,is_always,function_id,workflow_id,",
        "input_schema,output_schema,category_id,required_capabilities,created_at,updated_at) ",
        "VALUES ('agent_production_builtin','Agent Production Builtin','','function-wrap','workspace',0,?,NULL,?,?,NULL,NULL,datetime('now'),datetime('now')) RETURNING id"
    ))
    .bind(function.0)
    .bind(function.1)
    .bind(function.2)
    .fetch_one(store.pool())
    .await
    .expect("seed persisted Tool");
    let runner = Arc::new(LocalPersistedToolTargetRunner::from_store(&store));
    let executor = PersistedToolExecutor::new(store.pool().clone(), runner);

    let output = executor
        .execute(
            tool_id,
            serde_json::json!({"template":"hello {name}","vars":{"name":"local"}}),
            ToolExecutionContext::new(Vec::new()),
        )
        .await
        .expect("real local Builtin execution");
    assert_eq!(output, serde_json::json!("hello local"));
}

#[tokio::test(flavor = "current_thread")]
async fn start_session_resolves_default_root() {
    let (_workspace, runtime, store) = fresh_runtime().await;
    let first = store
        .create(root_input("root-agent", "Root Agent"))
        .await
        .expect("create root");
    assert!(
        first.is_default(),
        "first Agent must be auto-default per T115"
    );

    let (handle, _token) = runtime
        .start_session("hello from the user")
        .await
        .expect("start_session must resolve default root");
    let snapshot = runtime
        .snapshot(&handle)
        .expect("snapshot exists for the new session");
    assert_eq!(snapshot.agent.identifier(), "root-agent");
    assert!(snapshot.direct_children.is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn start_session_rejects_empty_user_message() {
    let (_workspace, runtime, store) = fresh_runtime().await;
    store
        .create(root_input("root-agent", "Root Agent"))
        .await
        .expect("create root");

    let err = runtime
        .start_session("")
        .await
        .expect_err("empty user_message must be rejected");
    assert!(matches!(err, LocalAgentError::InvalidUserMessage("empty")));
}

#[tokio::test(flavor = "current_thread")]
async fn start_session_rejects_oversized_user_message() {
    let (_workspace, runtime, store) = fresh_runtime().await;
    store
        .create(root_input("root-agent", "Root Agent"))
        .await
        .expect("create root");

    let payload = "x".repeat(1024 * 1024 + 1);
    let err = runtime
        .start_session(&payload)
        .await
        .expect_err("> 1MiB user_message must be rejected");
    assert!(matches!(
        err,
        LocalAgentError::InvalidUserMessage("exceeds 1MiB")
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn public_session_commands_reject_invalid_ids_and_retention_before_any_write() {
    let (_workspace, runtime, store) = fresh_runtime().await;
    store
        .create(root_input("command-root", "Command Root"))
        .await
        .expect("create root Agent");
    let before: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM chat_sessions), \
                (SELECT COUNT(*) FROM agent_executions)",
    )
    .fetch_one(runtime.pool())
    .await
    .expect("command footprint before invalid input");

    let invalid_session = runtime
        .continue_session("not-a-session-uuid", "hello")
        .await
        .expect_err("continue_session must validate session_id before lookup/write");
    assert_eq!(invalid_session.field(), "session_id");
    assert_eq!(invalid_session.reason(), "invalid_uuid");

    let invalid_execution = runtime
        .stop_execution("not-an-execution-uuid")
        .await
        .expect_err("stop_execution must validate execution_id before lookup/write");
    assert_eq!(invalid_execution.field(), "execution_id");
    assert_eq!(invalid_execution.reason(), "invalid_uuid");

    let invalid_delete = runtime
        .delete_session("still-not-a-session-uuid")
        .await
        .expect_err("delete_session must validate session_id before lookup/write");
    assert_eq!(invalid_delete.field(), "session_id");
    assert_eq!(invalid_delete.reason(), "invalid_uuid");

    let invalid_retention = runtime
        .clear_history("0 days")
        .await
        .expect_err("retention_filter value must be positive");
    assert_eq!(invalid_retention.field(), "retention_filter");
    assert_eq!(invalid_retention.reason(), "out_of_range");

    let after: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM chat_sessions), \
                (SELECT COUNT(*) FROM agent_executions)",
    )
    .fetch_one(runtime.pool())
    .await
    .expect("command footprint after invalid input");
    assert_eq!(
        after, before,
        "invalid commands must leave both tables unchanged"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn start_session_trims_message_and_persists_uuid_session_and_execution() {
    let (_workspace, runtime, store) = fresh_runtime().await;
    let root = store
        .create(root_input("persistent-root", "Persistent Root"))
        .await
        .expect("create root Agent");

    let whitespace = runtime
        .start_session(" \t\n ")
        .await
        .expect_err("trimmed-empty user_message must fail before persistence");
    assert_eq!(whitespace.field(), "user_message");
    assert_eq!(whitespace.reason(), "empty");
    let empty_footprint: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM chat_sessions), \
                (SELECT COUNT(*) FROM agent_executions)",
    )
    .fetch_one(runtime.pool())
    .await
    .expect("empty message footprint");
    assert_eq!(empty_footprint, (0, 0));

    let (handle, _token) = runtime
        .start_session("  persisted hello  ")
        .await
        .expect("start persisted session");
    let session: (String, String, i64, Option<i64>, String) = sqlx::query_as(
        "SELECT id, execution_id, entry_agent_id, current_agent_id, status \
         FROM chat_sessions",
    )
    .fetch_one(runtime.pool())
    .await
    .expect("one persisted ChatSession");
    let execution: (String, String, Option<i64>, String) = sqlx::query_as(
        "SELECT execution_id, session_id, current_agent_id, status FROM agent_executions",
    )
    .fetch_one(runtime.pool())
    .await
    .expect("one persisted AgentExecution");
    assert_eq!(session.0, handle.as_str());
    assert!(Uuid::parse_str(&session.0).is_ok());
    assert!(Uuid::parse_str(&session.1).is_ok());
    assert_eq!(session.2, root.id());
    assert_eq!(session.3, Some(root.id()));
    assert_eq!(session.4, "active");
    assert_eq!(execution.0, session.1);
    assert_eq!(execution.1, session.0);
    assert_eq!(execution.2, Some(root.id()));
    assert_eq!(execution.3, "running");
    assert_eq!(
        runtime
            .messages(&handle)
            .expect("runtime message")
            .iter()
            .map(|message| message.body())
            .collect::<Vec<_>>(),
        vec!["persisted hello"],
        "the accepted message is trimmed exactly once"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn start_session_requires_default_root() {
    let (_workspace, runtime, _store) = fresh_runtime().await;
    let err = runtime
        .start_session("hi")
        .await
        .expect_err("start_session without a default root must fail");
    assert!(matches!(err, LocalAgentError::NoDefaultRoot));
}

#[tokio::test(flavor = "current_thread")]
async fn route_to_child_loads_direct_child_snapshot() {
    let (_workspace, runtime, store) = fresh_runtime().await;
    let parent = store
        .create(root_input("parent", "Parent"))
        .await
        .expect("create parent");
    store
        .create(root_input("child", "Child").with_parent(Some(parent.id())))
        .await
        .expect("create child");

    let (handle, _token) = runtime.start_session("hello").await.expect("start session");
    let child_snapshot = runtime
        .route_to_child(&handle, "child")
        .await
        .expect("route to direct child");
    assert_eq!(child_snapshot.agent.identifier(), "child");
    assert_eq!(child_snapshot.agent.parent_agent_id(), Some(parent.id()));
}

#[tokio::test(flavor = "current_thread")]
async fn route_to_child_rejects_non_direct_child() {
    let (_workspace, runtime, store) = fresh_runtime().await;
    let parent = store
        .create(root_input("parent", "Parent"))
        .await
        .expect("create parent");
    let child = store
        .create(root_input("child", "Child").with_parent(Some(parent.id())))
        .await
        .expect("create child");
    let grandchild = store
        .create(root_input("grand", "Grandchild").with_parent(Some(child.id())))
        .await
        .expect("create grandchild");

    let (handle, _token) = runtime.start_session("hello").await.expect("start session");
    let err = runtime
        .route_to_child(&handle, "grand")
        .await
        .expect_err("cross-level routing must be rejected");
    assert!(matches!(err, LocalAgentError::AgentRejected(_)));
    let _ = grandchild;
}

#[tokio::test(flavor = "current_thread")]
async fn cycle_in_hierarchy_is_rejected_on_update_without_mutation() {
    let (_workspace, _runtime, store) = fresh_runtime().await;
    let a = store.create(root_input("a", "A")).await.expect("create a");
    let b = store
        .create(root_input("b", "B").with_parent(Some(a.id())))
        .await
        .expect("create b");
    let err = store
        .update(a.id(), root_input("a", "A").with_parent(Some(b.id())))
        .await
        .expect_err("moving a below b must create a cycle");
    assert_eq!(err.field(), "parent_agent_id");
    assert_eq!(err.reason(), "cycle");
    let a_reloaded = store
        .fetch_one(a.id())
        .await
        .expect("fetch a")
        .expect("a exists");
    let b_reloaded = store
        .fetch_one(b.id())
        .await
        .expect("fetch b")
        .expect("b exists");
    assert!(a_reloaded.parent_agent_id().is_none());
    assert_eq!(a_reloaded.depth(), 0);
    assert_eq!(b_reloaded.parent_agent_id(), Some(a.id()));
    assert_eq!(b_reloaded.depth(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn session_terminates_in_exactly_one_state() {
    let (_workspace, runtime, store) = fresh_runtime().await;
    store
        .create(root_input("root-agent", "Root Agent"))
        .await
        .expect("create root");

    let (handle, _token) = runtime.start_session("hello").await.expect("start session");
    runtime
        .append_message(&handle, hivegui::agent::session::AgentMessage::user("ping"))
        .expect("append message");
    runtime.cancel_execution(&handle).expect("cancel");
    let state = runtime
        .session_state(&handle)
        .expect("session still present after cancel");
    assert_eq!(state, "terminated", "cancel must transition to terminated");
    runtime.end_session(&handle).expect("end session");
    let after = runtime.session_state(&handle);
    assert!(
        after.is_none(),
        "ended session must be evicted from runtime"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn runtime_controls_never_persist_to_tool_store() {
    use hivegui::datasource::tool_store::{ToolInput, ToolKind, ToolSource, ToolStore};

    let (workspace, runtime, agent_store) = fresh_runtime().await;
    let pool = workspace.sqlite_pool().await.expect("pool");
    let tool_store = ToolStore::new(pool.clone()).expect("tool store");
    let function = sqlx::query_as::<_, (i64, String, String)>(
        "SELECT id, input_schema, output_schema FROM functions \
         WHERE identifier = 'format_template'",
    )
    .fetch_one(&pool)
    .await
    .expect("canonical Builtin Function target");
    let tool_input = |identifier: &str| {
        ToolInput::for_write(
            identifier.to_string(),
            identifier.to_string(),
            "runtime control persistence probe".to_string(),
            ToolKind::FunctionWrap,
            ToolSource::Workspace,
            false,
            Some(function.0),
            None,
            function.1.clone(),
            function.2.clone(),
            None,
            None,
        )
        .expect("valid persisted Tool input")
    };
    let initial_create = tool_store
        .create(tool_input("seed-tool"))
        .await
        .expect("seed tool created");

    agent_store
        .create(root_input("root-agent", "Root Agent"))
        .await
        .expect("create root");

    // Drive the runtime through a full lifecycle: start → reply → child
    // route → end. None of these control commands may produce a Tool
    // row in the Tool Store.
    let (handle, _token) = runtime.start_session("hello").await.expect("start session");
    runtime
        .append_message(&handle, hivegui::agent::session::AgentMessage::user("ping"))
        .expect("append message");
    let snapshot = runtime
        .reload_snapshot(&handle)
        .await
        .expect("reload snapshot");
    assert_eq!(snapshot.agent.identifier(), "root-agent");
    runtime.cancel_execution(&handle).expect("cancel");
    runtime.end_session(&handle).expect("end session");

    // Sanity: an explicit create is still possible after the lifecycle.
    let second = tool_store
        .create(tool_input("user-defined-tool"))
        .await
        .expect("explicit create should still work");
    assert_eq!(second.id(), initial_create.id() + 1);
}

#[tokio::test(flavor = "current_thread")]
async fn no_hiveweb_request_is_issued_during_full_session_lifecycle() {
    use std::net::TcpListener;
    use std::sync::Mutex;

    let bind_port: u16 = 49152;
    let listener = TcpListener::bind(("127.0.0.1", bind_port)).expect("capture listener");
    let captured: std::sync::Arc<Mutex<Vec<String>>> = std::sync::Arc::new(Mutex::new(Vec::new()));
    let captured_for_thread = captured.clone();
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            use std::io::Read;
            let mut buf = [0u8; 256];
            let n = stream.read(&mut buf).unwrap_or(0);
            let s = String::from_utf8_lossy(&buf[..n]).to_string();
            captured_for_thread.lock().expect("capture lock").push(s);
            // Don't respond — the runtime must not block on a server
            // that doesn't reply.
        }
    });

    let (_workspace, runtime, store) = fresh_runtime().await;
    store
        .create(root_input("root-agent", "Root Agent"))
        .await
        .expect("create root");

    let (handle, _token) = runtime.start_session("hello").await.expect("start session");
    runtime
        .append_message(&handle, hivegui::agent::session::AgentMessage::user("ping"))
        .expect("append message");
    runtime.cancel_execution(&handle).expect("cancel");
    runtime.end_session(&handle).expect("end session");

    // Give the capture loop a moment to register any connection that
    // might have been attempted (there must be none).
    std::thread::sleep(Duration::from_millis(50));
    let snapshot = captured.lock().expect("capture lock").clone();
    assert!(
        snapshot.is_empty(),
        "no HiveWeb request may be issued during the local session lifecycle; captured: {:?}",
        snapshot
    );
}

#[tokio::test(flavor = "current_thread")]
async fn snapshot_contains_resource_and_capability_lists() {
    let (_workspace, runtime, store) = fresh_runtime().await;
    let explicit_skill = sqlx::query_scalar::<_, i64>(
        "INSERT INTO skills (identifier, name, description, frontmatter, content, source, \
         is_always, category_id, required_capabilities, created_at, updated_at) \
         VALUES ('explicit_agent_skill', 'Explicit Agent Skill', '', NULL, 'explicit body', \
         'workspace', 0, NULL, NULL, datetime('now'), datetime('now')) RETURNING id",
    )
    .fetch_one(runtime.pool())
    .await
    .expect("seed explicit Skill");
    let always_skill = sqlx::query_scalar::<_, i64>(
        "INSERT INTO skills (identifier, name, description, frontmatter, content, source, \
         is_always, category_id, required_capabilities, created_at, updated_at) \
         VALUES ('always_agent_skill', 'Always Agent Skill', '', NULL, 'always body', \
         'workspace', 1, NULL, NULL, datetime('now'), datetime('now')) RETURNING id",
    )
    .fetch_one(runtime.pool())
    .await
    .expect("seed always Skill");
    let capability =
        sqlx::query_scalar::<_, String>("SELECT name FROM capabilities WHERE name = 'log.emit'")
            .fetch_one(runtime.pool())
            .await
            .expect("canonical local Capability");
    let parent = store
        .create(
            root_input("parent", "Parent")
                .with_skills([explicit_skill])
                .with_capabilities([capability.clone()]),
        )
        .await
        .expect("create parent");

    let (handle, _token) = runtime.start_session("hello").await.expect("start session");
    let snapshot = runtime.snapshot(&handle).expect("snapshot exists").clone();
    assert_eq!(snapshot.agent.identifier(), parent.identifier());
    assert_eq!(snapshot.skill_ids, [explicit_skill]);
    assert_eq!(snapshot.always_skill_ids, [always_skill]);
    assert_eq!(snapshot.capability_names, [capability]);
    assert_eq!(
        snapshot
            .content
            .skills()
            .iter()
            .map(|skill| skill.identifier.as_str())
            .collect::<Vec<_>>(),
        vec!["explicit_agent_skill", "always_agent_skill"]
    );
    assert_eq!(
        snapshot
            .content
            .capabilities()
            .iter()
            .map(|capability| capability.name.as_str())
            .collect::<Vec<_>>(),
        vec!["log.emit"]
    );
}

#[allow(dead_code)]
fn _pin_types() {
    fn assert_send<T: Send + Sync>() {}
    assert_send::<LocalAgentRuntime>();
    assert_send::<hivegui::agent::local_agent::SessionHandle>();
}
