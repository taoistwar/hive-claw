//! T091 [P] [US10] Workflow DAG execution contract.
//
// 验证 LocalWorkflowNodeExecutor + WorkflowExecutor 的目标行为：
// - 拓扑顺序（topological order）
// - 并行层（parallel levels）正确收集
// - dangling_edge / cycle / unreachable_node 在 execute 内被校验拒绝
// - 节点失败 fail-fast 且零重试（zero retry）
// - 取消（cancel）在层级边界生效并跳过下游
// - 四类稳定节点（start / end / function / generate_answer）可执行
// - generate_answer 缺模型时给出诊断错误
// - UI 模型预设经注入 transport 解析后下发给 provider
// - input_mapping 数据流、完整 typed terminal report、timeout 和取消终态
//
// 本文件是 T094 reviewer 前的 Red 合同；缺失行为不得降格为“当前行为”。

mod support;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;
use support::TestWorkspace;

use hivegui::datasource::llm_provider_store::{
    LlmProviderInput, LlmProviderStore, LlmProviderTokenInput,
};
use hivegui::datasource::store::{Store, StoreOpenOptions};
use hivegui::datasource::workflow_store::{NodeType, WorkflowGraph, WorkflowNode, WorkflowStore};
use hivegui::runtime::execution::CancelHandle;
use hivegui::runtime::provider_resolver::{ProviderTransport, TransportOutcome, TransportRequest};
use hivegui::runtime::workflow_executor::{
    LocalWorkflowNodeExecutor, WorkflowExecutionError, WorkflowExecutor, WorkflowNodeExecutor,
    WorkflowNodeStatus, WorkflowRunStatus, WorkflowValidationError,
};

/// 记录每个节点是否被调用以及调用次数（用于零重试断言）。
#[derive(Clone, Default)]
struct RecordingExecutor {
    calls: Arc<Mutex<HashMap<String, usize>>>,
    delay_ms: u64,
}

impl WorkflowNodeExecutor for RecordingExecutor {
    fn execute(
        &self,
        node: WorkflowNode,
        _input: serde_json::Value,
        cancel: CancelHandle,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<serde_json::Value, String>> + Send>,
    > {
        let calls = self.calls.clone();
        // 仅被点名的节点（如 "slow"）延迟；其余节点即时返回，避免取消测试假阳性。
        let delay = if node.key() == "slow" {
            self.delay_ms
        } else {
            0
        };
        Box::pin(async move {
            if delay > 0 {
                tokio::time::sleep(Duration::from_millis(delay)).await;
            }
            if cancel.is_cancelled() {
                return Err("cancelled".to_string());
            }
            {
                let mut g = calls.lock().unwrap();
                *g.entry(node.key().to_string()).or_insert(0) += 1;
            }
            Ok(json!({ "echo": node.key() }))
        })
    }
}

/// 一个会失败并断言“只调用一次”的执行器（仅对指定节点失败）。
#[derive(Clone, Default)]
struct FailingExecutor {
    calls: Arc<Mutex<HashMap<String, usize>>>,
}

impl WorkflowNodeExecutor for FailingExecutor {
    fn execute(
        &self,
        node: WorkflowNode,
        _input: serde_json::Value,
        _cancel: CancelHandle,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<serde_json::Value, String>> + Send>,
    > {
        let calls = self.calls.clone();
        let should_fail = node.key() == "fails";
        Box::pin(async move {
            {
                let mut c = calls.lock().unwrap();
                *c.entry(node.key().to_string()).or_insert(0) += 1;
            }
            if should_fail {
                Err("boom".to_string())
            } else {
                Ok(json!({ "echo": node.key() }))
            }
        })
    }
}

/// 记录每个节点实际收到的输入，并让 producer 产生可映射的结构化输出。
#[derive(Clone, Default)]
struct InputRecordingExecutor {
    inputs: Arc<Mutex<HashMap<String, serde_json::Value>>>,
}

impl WorkflowNodeExecutor for InputRecordingExecutor {
    fn execute(
        &self,
        node: WorkflowNode,
        input: serde_json::Value,
        _cancel: CancelHandle,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<serde_json::Value, String>> + Send>,
    > {
        let inputs = self.inputs.clone();
        Box::pin(async move {
            inputs
                .lock()
                .unwrap()
                .insert(node.key().to_string(), input.clone());
            if node.key() == "producer" {
                Ok(json!({ "answer": "from-upstream" }))
            } else {
                Ok(input)
            }
        })
    }
}

/// 从调用开始即计数的慢节点执行器，用于证明超时路径零重试。
#[derive(Clone, Default)]
struct SlowRecordingExecutor {
    calls: Arc<Mutex<HashMap<String, usize>>>,
    delay_ms: u64,
}

impl WorkflowNodeExecutor for SlowRecordingExecutor {
    fn execute(
        &self,
        node: WorkflowNode,
        input: serde_json::Value,
        cancel: CancelHandle,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<serde_json::Value, String>> + Send>,
    > {
        let calls = self.calls.clone();
        let delay = if node.key() == "slow" {
            self.delay_ms
        } else {
            0
        };
        Box::pin(async move {
            {
                let mut calls = calls.lock().unwrap();
                *calls.entry(node.key().to_string()).or_insert(0) += 1;
            }
            if delay > 0 {
                tokio::time::sleep(Duration::from_millis(delay)).await;
            }
            if cancel.is_cancelled() {
                return Err("cancelled".to_string());
            }
            Ok(input)
        })
    }
}

/// 捕获所有发出请求的 mock provider transport（用于 UI 模型预设解析断言）。
#[derive(Clone, Default)]
struct RecordingTransport {
    requests: Arc<Mutex<Vec<TransportRequest>>>,
}

impl ProviderTransport for RecordingTransport {
    fn call(
        &self,
        request: TransportRequest,
    ) -> Result<TransportOutcome, hivegui::runtime::provider_resolver::TransportError> {
        {
            let mut g = self.requests.lock().unwrap();
            g.push(request);
        }
        Ok(TransportOutcome::Success {
            content: "mock-response".to_string(),
        })
    }
}

// ---------------------------------------------------------------------------
// 测试辅助
// ---------------------------------------------------------------------------

async fn open_store() -> (Store, TestWorkspace) {
    let ws = TestWorkspace::new().expect("temp workspace");
    let store = Store::open_local(StoreOpenOptions::new(
        ws.database_path().to_path_buf(),
        ws.plugin_root().to_path_buf(),
    ))
    .await
    .expect("open local store");
    (store, ws)
}

/// 向 store 注入一个使用环境变量 token 的 provider（供 generate_answer 解析）。
async fn seed_provider(pool: sqlx::SqlitePool, env_var: &str) {
    let llm = LlmProviderStore::new(pool, support::FIXTURE_DEVICE_KEY)
        .await
        .expect("llm provider store");
    let input = LlmProviderInput::new(
        "ui-provider",
        "openai",
        LlmProviderTokenInput::Env(env_var.to_string()),
        "https://api.openai.com/v1",
    )
    .expect("valid provider input");
    llm.create(input).await.expect("seed provider");
}

// ---------------------------------------------------------------------------
// 执行测试
// ---------------------------------------------------------------------------

#[tokio::test]
async fn topology_executes_in_dependency_order_and_collects_results() {
    let (store, _ws) = open_store().await;
    let pool = store.pool().clone();
    let ws_store = WorkflowStore::new(pool).expect("workflow store");

    let graph = WorkflowGraph::builder()
        .name("topo")
        .node(WorkflowNode::new("start", NodeType::Start))
        .node(WorkflowNode::new("a", NodeType::Function).with_function_id(1))
        .node(WorkflowNode::new("b", NodeType::Function).with_function_id(1))
        .node(WorkflowNode::new("c", NodeType::Function).with_function_id(1))
        .node(WorkflowNode::new("end", NodeType::End))
        .edge("start", "a")
        .edge("start", "b")
        .edge("a", "c")
        .edge("b", "c")
        .edge("c", "end")
        .build();

    let record = ws_store.create(graph).await.expect("create graph");
    let graph = ws_store
        .load_graph(record.id(), "topo")
        .await
        .expect("load graph");

    let executor = RecordingExecutor::default();
    let runner = WorkflowExecutor::new(executor.clone());
    let cancel = CancelHandle::new();
    let outcome = runner
        .execute(&graph, json!({ "q": 1 }), cancel)
        .await
        .expect("run");

    assert!(outcome.completed);
    assert_eq!(outcome.node_results.len(), 5);
    let order: Vec<String> = outcome
        .node_results
        .iter()
        .map(|r| r.node_key.clone())
        .collect();
    assert_eq!(order, vec!["start", "a", "b", "c", "end"]);

    let calls = executor.calls.lock().unwrap();
    for k in ["start", "a", "b", "c", "end"] {
        assert_eq!(*calls.get(k).unwrap_or(&0), 1, "node {k} should run once");
    }
}

#[tokio::test]
async fn parallel_levels_run_concurrently_and_collect_all() {
    let (store, _ws) = open_store().await;
    let pool = store.pool().clone();
    let ws_store = WorkflowStore::new(pool).expect("workflow store");

    let graph = WorkflowGraph::builder()
        .name("parallel")
        .node(WorkflowNode::new("start", NodeType::Start))
        .node(WorkflowNode::new("p1", NodeType::Function).with_function_id(1))
        .node(WorkflowNode::new("p2", NodeType::Function).with_function_id(1))
        .node(WorkflowNode::new("p3", NodeType::Function).with_function_id(1))
        .node(WorkflowNode::new("end", NodeType::End))
        .edge("start", "p1")
        .edge("start", "p2")
        .edge("start", "p3")
        .edge("p1", "end")
        .edge("p2", "end")
        .edge("p3", "end")
        .build();

    let record = ws_store.create(graph).await.expect("create graph");
    let graph = ws_store
        .load_graph(record.id(), "parallel")
        .await
        .expect("load graph");

    let executor = RecordingExecutor::default();
    let runner = WorkflowExecutor::new(executor.clone());
    let cancel = CancelHandle::new();
    let outcome = runner
        .execute(&graph, json!({}), cancel)
        .await
        .expect("run");

    assert!(outcome.completed);
    assert_eq!(outcome.node_results.len(), 5);
    let calls = executor.calls.lock().unwrap();
    for k in ["start", "p1", "p2", "p3", "end"] {
        assert_eq!(*calls.get(k).unwrap_or(&0), 1);
    }
}

#[tokio::test]
async fn validation_rejects_dangling_edge() {
    let (store, _ws) = open_store().await;
    let pool = store.pool().clone();
    let ws_store = WorkflowStore::new(pool).expect("workflow store");

    let graph = WorkflowGraph::builder()
        .name("dangling")
        .node(WorkflowNode::new("start", NodeType::Start))
        .node(WorkflowNode::new("end", NodeType::End))
        .edge("start", "missing")
        .build();

    let record = ws_store.create(graph).await.expect("create graph");
    let graph = ws_store
        .load_graph(record.id(), "dangling")
        .await
        .expect("load graph");

    let executor = RecordingExecutor::default();
    let runner = WorkflowExecutor::new(executor);
    let cancel = CancelHandle::new();
    let err = runner
        .execute(&graph, json!({}), cancel)
        .await
        .expect_err("dangling edge must be rejected");
    assert!(matches!(
        err,
        WorkflowExecutionError::Validation(WorkflowValidationError::DanglingEdge(_))
    ));
}

#[tokio::test]
async fn validation_rejects_cycle() {
    let (store, _ws) = open_store().await;
    let pool = store.pool().clone();
    let ws_store = WorkflowStore::new(pool).expect("workflow store");

    let graph = WorkflowGraph::builder()
        .name("cycle")
        .node(WorkflowNode::new("start", NodeType::Start))
        .node(WorkflowNode::new("a", NodeType::Function).with_function_id(1))
        .node(WorkflowNode::new("b", NodeType::Function).with_function_id(1))
        .node(WorkflowNode::new("end", NodeType::End))
        .edge("start", "a")
        .edge("a", "b")
        .edge("b", "a")
        .edge("b", "end")
        .build();

    let record = ws_store.create(graph).await.expect("create graph");
    let graph = ws_store
        .load_graph(record.id(), "cycle")
        .await
        .expect("load graph");

    let executor = RecordingExecutor::default();
    let runner = WorkflowExecutor::new(executor);
    let cancel = CancelHandle::new();
    let err = runner
        .execute(&graph, json!({}), cancel)
        .await
        .expect_err("cycle must be rejected");
    assert!(matches!(
        err,
        WorkflowExecutionError::Validation(WorkflowValidationError::Cycle(_))
    ));
}

#[tokio::test]
async fn validation_detects_unreachable_node() {
    let (store, _ws) = open_store().await;
    let pool = store.pool().clone();
    let ws_store = WorkflowStore::new(pool).expect("workflow store");

    let graph = WorkflowGraph::builder()
        .name("unreachable")
        .node(WorkflowNode::new("start", NodeType::Start))
        .node(WorkflowNode::new("a", NodeType::Function).with_function_id(1))
        .node(WorkflowNode::new("orphan", NodeType::Function).with_function_id(1))
        .node(WorkflowNode::new("end", NodeType::End))
        .edge("start", "a")
        .edge("a", "end")
        .build();

    let record = ws_store.create(graph).await.expect("create graph");
    let graph = ws_store
        .load_graph(record.id(), "unreachable")
        .await
        .expect("load graph");

    let executor = RecordingExecutor::default();
    let runner = WorkflowExecutor::new(executor);
    let cancel = CancelHandle::new();
    let err = runner
        .execute(&graph, json!({}), cancel)
        .await
        .expect_err("unreachable node must be rejected");
    assert!(matches!(
        err,
        WorkflowExecutionError::Validation(WorkflowValidationError::UnreachableNode(k)) if k == "orphan"
    ));
}

#[tokio::test]
async fn node_failure_is_fail_fast_without_retry() {
    let (store, _ws) = open_store().await;
    let pool = store.pool().clone();
    let ws_store = WorkflowStore::new(pool).expect("workflow store");

    let graph = WorkflowGraph::builder()
        .name("failfast")
        .node(WorkflowNode::new("start", NodeType::Start))
        .node(WorkflowNode::new("fails", NodeType::Function).with_function_id(1))
        .node(WorkflowNode::new("end", NodeType::End))
        .edge("start", "fails")
        .edge("fails", "end")
        .build();

    let record = ws_store.create(graph).await.expect("create graph");
    let graph = ws_store
        .load_graph(record.id(), "failfast")
        .await
        .expect("load graph");
    let failing = FailingExecutor::default();
    let runner = WorkflowExecutor::new(failing.clone());
    let cancel = CancelHandle::new();
    let err = runner
        .execute(&graph, json!({}), cancel)
        .await
        .expect_err("node failure must abort run");
    assert!(matches!(
        err,
        WorkflowExecutionError::NodeFailed(key, reason) if key == "fails" && reason == "boom"
    ));
    // 零重试：fails 仅执行一次，end 不应执行
    let calls = failing.calls.lock().unwrap();
    assert_eq!(*calls.get("fails").unwrap_or(&0), 1);
    assert_eq!(*calls.get("start").unwrap_or(&0), 1);
    assert_eq!(*calls.get("end").unwrap_or(&0), 0);
}

#[tokio::test]
async fn cancellation_stops_subsequent_levels() {
    let (store, _ws) = open_store().await;
    let pool = store.pool().clone();
    let ws_store = WorkflowStore::new(pool).expect("workflow store");

    let graph = WorkflowGraph::builder()
        .name("cancel")
        .node(WorkflowNode::new("start", NodeType::Start))
        .node(WorkflowNode::new("slow", NodeType::Function).with_function_id(1))
        .node(WorkflowNode::new("end", NodeType::End))
        .edge("start", "slow")
        .edge("slow", "end")
        .build();

    let record = ws_store.create(graph).await.expect("create graph");
    let graph = ws_store
        .load_graph(record.id(), "cancel")
        .await
        .expect("load graph");

    // slow 延迟 300ms，使其在取消信号后仍处于运行态。
    let executor = RecordingExecutor {
        delay_ms: 300,
        ..Default::default()
    };
    let runner = WorkflowExecutor::new(executor.clone());
    let cancel = CancelHandle::new();
    let cancel_clone = cancel.clone();
    let exec_fut = {
        let graph = graph.clone();
        let input = json!({});
        tokio::spawn(async move { runner.execute(&graph, input, cancel_clone).await })
    };
    // 50ms 后取消
    tokio::time::sleep(Duration::from_millis(50)).await;
    cancel.signal();
    let result = exec_fut.await.unwrap();

    match result {
        Err(WorkflowExecutionError::NodeFailed(key, reason)) => {
            assert_eq!(key, "slow");
            assert_eq!(reason, "cancelled");
        }
        other => panic!("expected slow cancelled, got {other:?}"),
    }
    // fail-fast：end 绝不应执行
    let calls = executor.calls.lock().unwrap();
    assert_eq!(
        *calls.get("end").unwrap_or(&0),
        0,
        "end must be skipped after cancel"
    );
}

#[tokio::test]
async fn supported_node_types_run_locally() {
    let (store, _ws) = open_store().await;
    let pool = store.pool().clone();
    let ws_store = WorkflowStore::new(pool).expect("workflow store");

    let graph = WorkflowGraph::builder()
        .name("nodetypes")
        .node(WorkflowNode::new("start", NodeType::Start))
        .node(
            WorkflowNode::new("gen", NodeType::GenerateAnswer).with_node_config(
                json!({ "model": "ui-selected-model", "prompt": "{{input.query}}" }),
            ),
        )
        .node(WorkflowNode::new("end", NodeType::End))
        .edge("start", "gen")
        .edge("gen", "end")
        .build();

    let record = ws_store.create(graph).await.expect("create graph");
    let graph = ws_store
        .load_graph(record.id(), "nodetypes")
        .await
        .expect("load graph");

    // 需要一个可解析的 provider，否则 generate_answer 在解析阶段报错。
    seed_provider(store.pool().clone(), "UI_TEST_TOKEN_GEN").await;
    unsafe {
        std::env::set_var("UI_TEST_TOKEN_GEN", "test-token");
    }

    let transport = Arc::new(RecordingTransport::default());
    let executor = LocalWorkflowNodeExecutor::new(
        store.pool().clone(),
        store.plugin_root().to_path_buf(),
        store.crypto().clone(),
    )
    .with_provider_transport(transport.clone());
    let runner = WorkflowExecutor::new(executor);
    let cancel = CancelHandle::new();
    let outcome = runner
        .execute(&graph, json!({ "query": "hi" }), cancel)
        .await
        .expect("run");

    assert!(outcome.completed);
    assert_eq!(outcome.node_results.len(), 3);
}

#[tokio::test]
async fn generate_answer_without_model_is_diagnostic() {
    let (store, _ws) = open_store().await;
    let pool = store.pool().clone();
    let ws_store = WorkflowStore::new(pool).expect("workflow store");

    let graph = WorkflowGraph::builder()
        .name("no-model")
        .node(WorkflowNode::new("start", NodeType::Start))
        .node(
            WorkflowNode::new("gen", NodeType::GenerateAnswer)
                .with_node_config(json!({ "prompt": "{{input.query}}" })),
        )
        .node(WorkflowNode::new("end", NodeType::End))
        .edge("start", "gen")
        .edge("gen", "end")
        .build();

    let record = ws_store.create(graph).await.expect("create graph");
    let graph = ws_store
        .load_graph(record.id(), "no-model")
        .await
        .expect("load graph");

    // 使用注入 transport 以避免在测试异步上下文里创建嵌套 tokio runtime
    // （默认 ReqwestProviderTransport 会在 call 路径创建 runtime）。模型诊断
    // 发生在 transport.call 之前，因此注入 transport 不影响该断言。
    let transport = Arc::new(RecordingTransport::default());
    let executor = LocalWorkflowNodeExecutor::new(
        store.pool().clone(),
        store.plugin_root().to_path_buf(),
        store.crypto().clone(),
    )
    .with_provider_transport(transport.clone());
    let runner = WorkflowExecutor::new(executor);
    let cancel = CancelHandle::new();
    let err = runner
        .execute(&graph, json!({ "query": "hi" }), cancel)
        .await
        .expect_err("missing model must be diagnostic error");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("model") || msg.contains("模型"),
        "unexpected error: {msg}"
    );
}

#[tokio::test]
async fn workflow_function_node_uses_the_callers_capability_snapshot() {
    let (store, _ws) = open_store().await;
    let function_id = sqlx::query_scalar::<_, i64>(
        "SELECT id FROM functions WHERE identifier = 'format_template'",
    )
    .fetch_one(store.pool())
    .await
    .expect("Builtin Function");
    sqlx::query("UPDATE functions SET required_capabilities = '[\"log.emit\"]' WHERE id = ?")
        .bind(function_id)
        .execute(store.pool())
        .await
        .expect("seed capability-bound Function");
    let node =
        WorkflowNode::new("capability-bound", NodeType::Function).with_function_id(function_id);
    let input = json!({"template":"hello {name}","vars":{"name":"local"}});

    let denied = LocalWorkflowNodeExecutor::new(
        store.pool().clone(),
        store.plugin_root().to_path_buf(),
        store.crypto().clone(),
    )
    .with_capabilities(Vec::new())
    .execute(node.clone(), input.clone(), CancelHandle::new())
    .await
    .expect_err("Workflow must not self-grant the Function capability");
    assert_eq!(denied, "capability_denied");

    let allowed = LocalWorkflowNodeExecutor::new(
        store.pool().clone(),
        store.plugin_root().to_path_buf(),
        store.crypto().clone(),
    )
    .with_capabilities(vec!["log.emit".to_string()])
    .execute(node, input, CancelHandle::new())
    .await
    .expect("the caller-granted snapshot reaches the Function");
    assert_eq!(allowed, json!("hello local"));
}

#[tokio::test]
async fn local_executor_resolves_ui_model_preset_through_injected_transport() {
    let (store, _ws) = open_store().await;
    let pool = store.pool().clone();
    let ws_store = WorkflowStore::new(pool.clone()).expect("workflow store");

    // 插入一个可被解析的 provider（key 与 node_config.model 一致）
    seed_provider(pool.clone(), "UI_TEST_TOKEN").await;
    unsafe {
        std::env::set_var("UI_TEST_TOKEN", "test-token");
    }

    let graph = WorkflowGraph::builder()
        .name("ui-model")
        .node(WorkflowNode::new("start", NodeType::Start))
        .node(
            WorkflowNode::new("gen", NodeType::GenerateAnswer).with_node_config(
                json!({ "model": "ui-selected-model", "prompt": "{{input.query}}" }),
            ),
        )
        .node(WorkflowNode::new("end", NodeType::End))
        .edge("start", "gen")
        .edge("gen", "end")
        .build();

    let record = ws_store.create(graph).await.expect("create graph");
    let graph = ws_store
        .load_graph(record.id(), "ui-model")
        .await
        .expect("load graph");

    let transport = Arc::new(RecordingTransport::default());
    let transport_capture = transport.requests.clone();
    let executor = LocalWorkflowNodeExecutor::new(
        pool,
        store.plugin_root().to_path_buf(),
        store.crypto().clone(),
    )
    .with_provider_transport(transport.clone());
    let runner = WorkflowExecutor::new(executor);
    let cancel = CancelHandle::new();
    let outcome = runner
        .execute(&graph, json!({ "query": "hello" }), cancel)
        .await
        .expect("expected ok via injected transport");

    assert!(outcome.completed);
    let reqs = transport_capture.lock().unwrap();
    assert_eq!(reqs.len(), 1, "exactly one provider call");
    assert_eq!(
        reqs[0].model(),
        "ui-selected-model",
        "UI model preset must reach provider"
    );
}

#[tokio::test]
async fn downstream_node_receives_mapped_upstream_output_instead_of_root_input() {
    let graph = WorkflowGraph::builder()
        .name("mapped-dataflow")
        .node(WorkflowNode::new("start", NodeType::Start))
        .node(WorkflowNode::new("producer", NodeType::Function).with_function_id(1))
        .node(
            WorkflowNode::new("consumer", NodeType::Function)
                .with_function_id(2)
                .with_node_config(json!({
                    "input_mapping": {
                        "question": {
                            "kind": "upstream",
                            "node_key": "producer",
                            "path": "answer"
                        }
                    }
                })),
        )
        .node(WorkflowNode::new("end", NodeType::End))
        .edge("start", "producer")
        .edge("producer", "consumer")
        .edge("consumer", "end")
        .build();

    let executor = InputRecordingExecutor::default();
    let captured_inputs = executor.inputs.clone();
    let runner = WorkflowExecutor::new(executor);
    runner
        .execute(
            &graph,
            json!({ "root_input": "must-not-reach-consumer" }),
            CancelHandle::new(),
        )
        .await
        .expect("mapped workflow executes");

    let inputs = captured_inputs.lock().unwrap();
    assert_eq!(
        inputs.get("consumer"),
        Some(&json!({ "question": "from-upstream" })),
        "consumer input must be derived from the producer output and its input_mapping"
    );
}

#[tokio::test]
async fn failed_parallel_layer_returns_a_complete_typed_terminal_report() {
    let graph = WorkflowGraph::builder()
        .name("typed-failure-report")
        .node(WorkflowNode::new("start", NodeType::Start))
        .node(WorkflowNode::new("fails", NodeType::Function).with_function_id(1))
        .node(WorkflowNode::new("sibling", NodeType::Function).with_function_id(2))
        .node(WorkflowNode::new("end", NodeType::End))
        .edge("start", "fails")
        .edge("start", "sibling")
        .edge("fails", "end")
        .edge("sibling", "end")
        .build();

    let executor = FailingExecutor::default();
    let calls = executor.calls.clone();
    let report = WorkflowExecutor::new(executor)
        .execute_report(&graph, json!({}), CancelHandle::new())
        .await
        .expect("node failure is a terminal report, not a discarded partial result");

    assert_eq!(report.status, WorkflowRunStatus::Failed);
    assert_eq!(report.failed_node.as_deref(), Some("fails"));
    assert_eq!(report.error_category.as_deref(), Some("node_failed"));
    assert!(report.side_effects_may_have_occurred);
    assert!(report.elapsed_ms < 60_000);

    let status = |key: &str| {
        report
            .node_results
            .iter()
            .find(|node| node.node_key == key)
            .map(|node| node.status)
    };
    assert_eq!(status("start"), Some(WorkflowNodeStatus::Completed));
    assert_eq!(status("sibling"), Some(WorkflowNodeStatus::Completed));
    assert_eq!(status("fails"), Some(WorkflowNodeStatus::Failed));
    assert_eq!(status("end"), Some(WorkflowNodeStatus::NotStarted));

    let calls = calls.lock().unwrap();
    assert_eq!(
        *calls.get("fails").unwrap_or(&0),
        1,
        "failure is never retried"
    );
    assert_eq!(
        *calls.get("end").unwrap_or(&0),
        0,
        "no later layer is scheduled"
    );
}

#[tokio::test]
async fn workflow_timeout_reports_timed_out_and_not_started_nodes_without_retry() {
    let graph = WorkflowGraph::builder()
        .name("typed-timeout-report")
        .node(WorkflowNode::new("start", NodeType::Start))
        .node(WorkflowNode::new("slow", NodeType::Function).with_function_id(1))
        .node(WorkflowNode::new("end", NodeType::End))
        .edge("start", "slow")
        .edge("slow", "end")
        .build();

    let executor = SlowRecordingExecutor {
        delay_ms: 250,
        ..Default::default()
    };
    let calls = executor.calls.clone();
    let report = WorkflowExecutor::new(executor)
        .execute_report_with_timeout(
            &graph,
            json!({}),
            CancelHandle::new(),
            Duration::from_millis(25),
        )
        .await
        .expect("timeout is a typed terminal report");

    assert_eq!(report.status, WorkflowRunStatus::Failed);
    assert_eq!(report.failed_node.as_deref(), Some("slow"));
    assert_eq!(report.error_category.as_deref(), Some("timeout"));
    let status = |key: &str| {
        report
            .node_results
            .iter()
            .find(|node| node.node_key == key)
            .map(|node| node.status)
    };
    assert_eq!(status("start"), Some(WorkflowNodeStatus::Completed));
    assert_eq!(status("slow"), Some(WorkflowNodeStatus::TimedOut));
    assert_eq!(status("end"), Some(WorkflowNodeStatus::NotStarted));

    let calls = calls.lock().unwrap();
    assert_eq!(
        *calls.get("slow").unwrap_or(&0),
        1,
        "timed-out node is never retried"
    );
    assert_eq!(
        *calls.get("end").unwrap_or(&0),
        0,
        "timeout stops later scheduling"
    );
}

#[tokio::test]
async fn cancellation_report_distinguishes_completed_interrupted_and_not_started_nodes() {
    let graph = WorkflowGraph::builder()
        .name("typed-cancellation-report")
        .node(WorkflowNode::new("start", NodeType::Start))
        .node(WorkflowNode::new("slow", NodeType::Function).with_function_id(1))
        .node(WorkflowNode::new("end", NodeType::End))
        .edge("start", "slow")
        .edge("slow", "end")
        .build();

    let executor = SlowRecordingExecutor {
        delay_ms: 250,
        ..Default::default()
    };
    let calls = executor.calls.clone();
    let runner = WorkflowExecutor::new(executor);
    let cancel = CancelHandle::new();
    let cancel_for_run = cancel.clone();
    let run = tokio::spawn(async move {
        runner
            .execute_report(&graph, json!({}), cancel_for_run)
            .await
            .expect("cancel is a typed terminal report")
    });
    tokio::time::sleep(Duration::from_millis(25)).await;
    cancel.signal();
    let report = run.await.expect("join workflow report");

    assert_eq!(report.status, WorkflowRunStatus::Cancelled);
    assert_eq!(report.error_category.as_deref(), Some("cancelled"));
    assert!(report.side_effects_may_have_occurred);
    let status = |key: &str| {
        report
            .node_results
            .iter()
            .find(|node| node.node_key == key)
            .map(|node| node.status)
    };
    assert_eq!(status("start"), Some(WorkflowNodeStatus::Completed));
    assert_eq!(status("slow"), Some(WorkflowNodeStatus::Cancelled));
    assert_eq!(status("end"), Some(WorkflowNodeStatus::NotStarted));

    let calls = calls.lock().unwrap();
    assert_eq!(*calls.get("slow").unwrap_or(&0), 1);
    assert_eq!(*calls.get("end").unwrap_or(&0), 0);
}
