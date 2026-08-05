//! Perf bench: End-to-end chat SSE flow (T168 / SC-010)
//!
//! Target: 完整 SSE 对话流程（user msg → session create → orchestrator run_session →
//!         SSE token/done 事件消费），mock LLM provider 固定 800ms 响应，p95 ≤ 8s
//!
//! 当前实现策略（同 T140-T156 的 stub 模式）：
//!   - 可隔离部分：SSE event 构造/序列化 + mpsc channel 开销 → 真实基准
//!   - 不可隔离部分（需 DB + S3 + 真实 Extism）→ 结构化 stub + 详细 TODO
//!
//! 完整基准需要：
//!   1. MySQL 实例 + V001..V018 migrations applied
//!   2. S3/Rustfs endpoint
//!   3. 真实 WASM plugin 编译产物（smoke-plugin）
//!   4. Mock LLM provider（固定 800ms 延迟 + 可配置 tool_calls 响应）
//!
//! 运行: `cargo bench --bench chat_e2e`

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use providers::LLMProvider;
use serde_json::json;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Runtime;

// ============================================================================
// Part 1: SSE event construction + serialization (真实基准，无外部依赖)
// ============================================================================

fn bench_sse_event_construction(c: &mut Criterion) {
    let mut group = c.benchmark_group("sse_event_construction");
    group.warm_up_time(Duration::from_millis(500));
    group.measurement_time(Duration::from_secs(2));

    // token event — 最频繁（每 1-2 个 token 一次）
    group.bench_function("token_event", |b| {
        b.iter(|| {
            let payload = json!({ "text": "你好世界" });
            let data = payload.to_string();
            let event = format!("event: token\ndata: {data}\n\n");
            black_box(event);
        })
    });

    // done event — 每次对话一次
    group.bench_function("done_event", |b| {
        b.iter(|| {
            let payload = json!({ "elapsed_ms": 1200, "final_agent_id": null });
            let data = payload.to_string();
            let event = format!("event: done\ndata: {data}\n\n");
            black_box(event);
        })
    });

    // tool_call event — 中等频率
    group.bench_function("tool_call_event", |b| {
        b.iter(|| {
            let payload = json!({
                "tool_call_id": "call_abc123",
                "name": "invoke_function",
                "args": { "function_identifier": "json_parse", "function_input": "{\"raw\":\"{}\"}" }
            });
            let data = payload.to_string();
            let event = format!("event: tool_call\ndata: {data}\n\n");
            black_box(event);
        })
    });

    // tool_result event
    group.bench_function("tool_result_event", |b| {
        b.iter(|| {
            let payload = json!({
                "tool_call_id": "call_abc123",
                "result": { "parsed": true, "data": {} }
            });
            let data = payload.to_string();
            let event = format!("event: tool_result\ndata: {data}\n\n");
            black_box(event);
        })
    });

    // routed event — 路由时触发
    group.bench_function("routed_event", |b| {
        b.iter(|| {
            let payload = json!({ "agent_id": 42, "agent_identifier": "coding-expert" });
            let data = payload.to_string();
            let event = format!("event: routed\ndata: {data}\n\n");
            black_box(event);
        })
    });

    // fallback_used event
    group.bench_function("fallback_used_event", |b| {
        b.iter(|| {
            let payload =
                json!({ "from": "gpt-4o", "to": "gpt-4o-mini", "reason": "429 rate limited" });
            let data = payload.to_string();
            let event = format!("event: fallback_used\ndata: {data}\n\n");
            black_box(event);
        })
    });

    // error event
    group.bench_function("error_event", |b| {
        b.iter(|| {
            let payload = json!({ "code": 5006, "message": "已达最大 hop 5" });
            let data = payload.to_string();
            let event = format!("event: error\ndata: {data}\n\n");
            black_box(event);
        })
    });

    group.finish();
}

// ============================================================================
// Part 2: mpsc channel throughput (模拟 orchestrator → SSE 流)
// ============================================================================

fn bench_mpsc_channel_throughput(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("mpsc_channel_throughput");
    group.warm_up_time(Duration::from_millis(500));
    group.measurement_time(Duration::from_secs(2));

    // 测试不同 token 批量大小下的 channel 吞吐
    for batch_size in [10, 50, 100, 200] {
        group.throughput(Throughput::Elements(batch_size as u64));
        group.bench_with_input(
            BenchmarkId::new("token_batch", batch_size),
            &batch_size,
            |b, &batch_size| {
                b.to_async(&rt).iter(|| async move {
                    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();

                    // sender: 发 batch_size 个 token 事件
                    tokio::spawn(async move {
                        for i in 0..batch_size {
                            let payload = json!({ "text": format!("token_{i}") }).to_string();
                            let event = format!("event: token\ndata: {payload}\n\n");
                            if tx.send(event).is_err() {
                                break;
                            }
                        }
                    });

                    // receiver: 消费所有事件
                    let mut received = 0usize;
                    while let Some(_event) = rx.recv().await {
                        received += 1;
                        if received >= batch_size {
                            break;
                        }
                    }
                    black_box(received);
                });
            },
        );
    }

    group.finish();
}

// ============================================================================
// Part 3: Full session simulation with mock LLM (800ms fixed latency)
//         真实 mock — 不需要 DB/S3/Extism，只验证编排 + channel 开销
// ============================================================================

/// Mock LLM provider — 固定 800ms 延迟后返回文本响应（模拟一次 LLM round-trip）
struct MockLlmProvider {
    delay_ms: u64,
}

impl MockLlmProvider {
    fn new(delay_ms: u64) -> Self {
        Self { delay_ms }
    }
}

#[async_trait::async_trait]
impl providers::LLMProvider for MockLlmProvider {
    fn default_model(&self) -> String {
        "mock-bench-model".into()
    }

    async fn chat(&self, _req: providers::ChatRequest) -> providers::LLMResponse {
        tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;
        providers::LLMResponse {
            content: Some("这是一个模拟的助手回复。".into()),
            tool_calls: vec![],
            finish_reason: "stop".into(),
            usage: Default::default(),
            ..Default::default()
        }
    }
}

fn bench_full_session_mock(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("chat_e2e_session");
    group.warm_up_time(Duration::from_secs(2));
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(20); // 每次约 800ms+，20 个样本即可

    // 单次完整对话（1 hop, 无 tool call, mock LLM 800ms）
    group.bench_function("single_hop_no_toolcall", |b| {
        b.to_async(&rt).iter(|| async move {
            let provider = Arc::new(MockLlmProvider::new(800));

            // 模拟 orchestrator 发事件到 mpsc channel
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Result<String, Infallible>>();

            let provider_clone = Arc::clone(&provider);
            let tx_clone = tx.clone();
            tokio::spawn(async move {
                // 1. LLM call (800ms mock)
                let req = providers::ChatRequest {
                    messages: vec![json!({"role": "user", "content": "你好"})],
                    max_tokens: 2048,
                    temperature: 0.7,
                    ..Default::default()
                };
                let resp = provider_clone.chat(req).await;

                // 2. 发送 token 事件（模拟流式拆分）
                if let Some(content) = &resp.content {
                    for chunk in content.chars() {
                        let payload = json!({ "text": chunk }).to_string();
                        let event = format!("event: token\ndata: {payload}\n\n");
                        if tx_clone.send(Ok(event)).is_err() {
                            break;
                        }
                    }
                }

                // 3. 发送 done 事件
                let done = json!({ "elapsed_ms": 800, "final_agent_id": null }).to_string();
                let _ = tx_clone.send(Ok(format!("event: done\ndata: {done}\n\n")));
            });

            // 消费所有事件
            let mut events_received = 0usize;
            let mut got_done = false;
            while let Some(Ok(data)) = rx.recv().await {
                events_received += 1;
                if data.starts_with("event: done") {
                    got_done = true;
                    break;
                }
            }
            black_box((events_received, got_done));
        });
    });

    // 双 hop 场景（1 次 LLM → route → 1 次 LLM，总 mock 延迟 ~1600ms）
    group.bench_function("two_hop_with_routing", |b| {
        b.to_async(&rt).iter(|| async move {
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Result<String, Infallible>>();

            let tx_clone = tx.clone();
            tokio::spawn(async move {
                // Hop 1: LLM call (800ms) → 返回 route_to_subagent tool_call
                tokio::time::sleep(Duration::from_millis(800)).await;

                // routed event
                let routed =
                    json!({ "agent_id": 2, "agent_identifier": "coding-expert" }).to_string();
                let _ = tx_clone.send(Ok(format!("event: routed\ndata: {routed}\n\n")));

                // Hop 2: 子 agent LLM call (800ms)
                tokio::time::sleep(Duration::from_millis(800)).await;

                // token event
                let token = json!({ "text": "这是子Agent的回复" }).to_string();
                let _ = tx_clone.send(Ok(format!("event: token\ndata: {token}\n\n")));

                // done event
                let done = json!({ "elapsed_ms": 1600, "final_agent_id": 2 }).to_string();
                let _ = tx_clone.send(Ok(format!("event: done\ndata: {done}\n\n")));
            });

            let mut events_received = 0usize;
            let mut got_done = false;
            while let Some(Ok(data)) = rx.recv().await {
                events_received += 1;
                if data.starts_with("event: done") {
                    got_done = true;
                    break;
                }
            }
            black_box((events_received, got_done));
        });
    });

    group.finish();
}

// ============================================================================
// Part 4: Stub for full integration benchmark (needs DB/S3/Extism)
// ============================================================================

fn bench_chat_e2e_integration_stub(c: &mut Criterion) {
    let mut group = c.benchmark_group("chat_e2e_integration_stub");
    group.bench_function("stub", |b| b.iter(|| black_box(0)));
    group.finish();
}

criterion_group!(
    benches,
    bench_sse_event_construction,
    bench_mpsc_channel_throughput,
    bench_full_session_mock,
    bench_chat_e2e_integration_stub,
);
criterion_main!(benches);
