//! Integration & contract tests: Agent Hook 配置管理 (008-agent-hook-config)
//!
//! Covers: T008-T011, T013-T014, T016-T017, T052, T054, T056-T058.
//!
//! Contract tests (REST API): T008-T011, T013-T014, T016, T052, T056
//! Runtime tests: T017, T054, T057-T058

mod common;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

#[derive(Clone, Default)]
struct TraceWriter(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

struct TraceWriterGuard(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for TraceWriterGuard {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("trace buffer poisoned").extend(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for TraceWriter {
    type Writer = TraceWriterGuard;

    fn make_writer(&'a self) -> Self::Writer {
        TraceWriterGuard(std::sync::Arc::clone(&self.0))
    }
}

// ── Agent seed helper (RAII cleanup on drop) ──

struct SeededAgent {
    pub id: i64,
    pool: sqlx::MySqlPool,
}

impl SeededAgent {
    async fn new(pool: &sqlx::MySqlPool, prefix: &str) -> anyhow::Result<Self> {
        use std::time::{SystemTime, UNIX_EPOCH};
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.subsec_nanos() % 100_000_000)
            .unwrap_or(0);
        let identifier = format!("{prefix}-{}", suffix);
        let name = format!("Test {prefix}");

        let result = sqlx::query(
            r#"INSERT INTO agents (identifier, name, description, system_prompt, depth)
               VALUES (?, ?, ?, ?, 0)"#,
        )
        .bind(&identifier)
        .bind(&name)
        .bind(format!("Test agent: {prefix}"))
        .bind("You are a helpful test assistant.")
        .execute(pool)
        .await?;
        Ok(Self {
            id: result.last_insert_id() as i64,
            pool: pool.clone(),
        })
    }
}

impl Drop for SeededAgent {
    fn drop(&mut self) {
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let pool = self.pool.clone();
            let id = self.id;
            handle.spawn(async move {
                // FK CASCADE cleans up the Agent's Hook configuration.
                let _ = sqlx::query("DELETE FROM agents WHERE id = ?")
                    .bind(id)
                    .execute(&pool)
                    .await;
            });
        }
    }
}

// ── HTTP helpers ──

async fn put_json_auth(
    app: &axum::Router,
    path: &str,
    token: &str,
    payload: Value,
) -> anyhow::Result<(StatusCode, Value)> {
    let req = Request::builder()
        .method("PUT")
        .uri(path)
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {}", token))
        .body(Body::from(payload.to_string()))?;
    let resp = app.clone().oneshot(req).await?;
    let status = resp.status();
    let bytes = resp.into_body().collect().await?.to_bytes();
    let body: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)
            .unwrap_or(Value::String(String::from_utf8_lossy(&bytes).into_owned()))
    };
    Ok((status, body))
}

// ── Test data ──

fn make_hook_payload(trigger: &str, action_type: &str, params: Value) -> Value {
    json!({
        "name": format!("test-{trigger}"),
        "trigger_point": trigger,
        "action_type": action_type,
        "action_params": params,
    })
}

fn make_webhook_payload(url: &str) -> Value {
    make_hook_payload(
        "after_agent_end",
        "http_webhook",
        json!({
            "webhook_url": url,
            "headers": {},
        }),
    )
}

fn make_function_payload(function_id: i64) -> Value {
    make_hook_payload(
        "before_agent_start",
        "call_function",
        json!({
            "function_id": function_id,
            "args": {},
        }),
    )
}

// ═══════════════════════════════════════════════════════════════════
// US1 — Hook CRUD (T008-T011, T013)
// ═══════════════════════════════════════════════════════════════════

/// T008 — POST /agents/:id/hooks 创建 Hook → 200 + Hook 对象含 id/created_at
#[tokio::test]
async fn t008_create_hook_returns_200_and_hook_object() -> anyhow::Result<()> {
    let app = common::test_app().await?;
    let pool = common::test_pool().await?;
    let admin = common::seed_admin(&pool, 3, 1, "test-pass-123").await?; // Super
    let agent = SeededAgent::new(&pool, "t008").await?;

    let (status, body) = common::post_json_auth(
        &app,
        &format!("/api/agents/{}/hooks", agent.id),
        &admin.token()?,
        make_hook_payload(
            "before_agent_start",
            "http_webhook",
            json!({
                "webhook_url": "https://example.com/hook",
                "headers": {},
            }),
        ),
    )
    .await?;

    assert_eq!(
        status,
        StatusCode::OK,
        "create hook must return 200, got {status}: {body}"
    );
    assert_eq!(body["code"], 0, "must have code=0");
    let data = &body["data"];
    assert!(data["id"].as_i64().is_some(), "must include id");
    assert_eq!(data["name"], "test-before_agent_start");
    assert_eq!(data["trigger_point"], "before_agent_start");
    assert_eq!(data["action_type"], "http_webhook");
    assert!(
        data["created_at"].as_str().is_some(),
        "must include created_at"
    );
    assert!(
        data["enabled"].as_bool().unwrap_or(false),
        "default enabled=true"
    );
    Ok(())
}

/// T009 — GET /agents/:id/hooks 列出 Hook → 返回数组 + 按 trigger_point + sort_order 排序
#[tokio::test]
async fn t009_list_hooks_returns_sorted_array() -> anyhow::Result<()> {
    let app = common::test_app().await?;
    let pool = common::test_pool().await?;
    let admin = common::seed_admin(&pool, 3, 1, "test-pass-123").await?;
    let agent = SeededAgent::new(&pool, "t009").await?;

    // Create hooks with different trigger points and sort orders
    let base_path = format!("/api/agents/{}/hooks", agent.id);
    let token = admin.token()?;

    // after_agent_end with sort_order=0
    let mut payload = make_hook_payload(
        "after_agent_end",
        "http_webhook",
        json!({"webhook_url": "https://example.com/a", "headers": {}}),
    );
    payload["sort_order"] = json!(0);
    common::post_json_auth(&app, &base_path, &token, payload).await?;

    // before_tool_call with sort_order=0
    let mut payload = make_hook_payload(
        "before_tool_call",
        "http_webhook",
        json!({"webhook_url": "https://example.com/b", "headers": {}}),
    );
    payload["sort_order"] = json!(0);
    common::post_json_auth(&app, &base_path, &token, payload).await?;

    // after_agent_end with sort_order=1
    let mut payload = make_hook_payload(
        "after_agent_end",
        "http_webhook",
        json!({"webhook_url": "https://example.com/c", "headers": {}}),
    );
    payload["sort_order"] = json!(1);
    common::post_json_auth(&app, &base_path, &token, payload).await?;

    let (status, body) = common::get(&app, &base_path, Some(&token)).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["code"], 0);

    let items = body["data"].as_array().expect("must be array");
    assert_eq!(items.len(), 3);
    // Must be sorted: after_agent_end[0], after_agent_end[1], before_tool_call[0]
    assert_eq!(items[0]["trigger_point"], "after_agent_end");
    assert_eq!(items[0]["sort_order"], 0);
    assert_eq!(items[1]["trigger_point"], "after_agent_end");
    assert_eq!(items[1]["sort_order"], 1);
    assert_eq!(items[2]["trigger_point"], "before_tool_call");
    assert_eq!(items[2]["sort_order"], 0);
    Ok(())
}

/// T010 — PUT /agents/:id/hooks/:hook_id 更新 Hook → 乐观锁 (4094)
#[tokio::test]
async fn t010_update_hook_optimistic_lock_conflict() -> anyhow::Result<()> {
    let app = common::test_app().await?;
    let pool = common::test_pool().await?;
    let admin = common::seed_admin(&pool, 3, 1, "test-pass-123").await?;
    let agent = SeededAgent::new(&pool, "t010").await?;
    let token = admin.token()?;
    let base_path = format!("/api/agents/{}/hooks", agent.id);

    // Create a hook
    let (_, create_body) = common::post_json_auth(
        &app,
        &base_path,
        &token,
        make_webhook_payload("https://example.com/hook"),
    )
    .await?;
    let hook_id = create_body["data"]["id"].as_i64().unwrap();
    let _updated_at = create_body["data"]["updated_at"].as_str().unwrap();

    // Update with wrong updated_at → 4094 optimistic lock conflict
    let (status, body) = put_json_auth(
        &app,
        &format!("/api/agents/{}/hooks/{}", agent.id, hook_id),
        &token,
        json!({
            "name": "updated-name",
            "updated_at": "2020-01-01T00:00:00Z",
        }),
    )
    .await?;

    assert!(
        status.is_client_error(),
        "stale updated_at must produce error, got {status}: {body}"
    );
    assert_eq!(
        body["code"], 4094,
        "must return 4094 optimistic lock, got {body}"
    );
    Ok(())
}

/// T011 — DELETE /agents/:id/hooks/:hook_id → 200 + 再次 GET 不含该 Hook
#[tokio::test]
async fn t011_delete_hook_and_verify_removed() -> anyhow::Result<()> {
    let app = common::test_app().await?;
    let pool = common::test_pool().await?;
    let admin = common::seed_admin(&pool, 3, 1, "test-pass-123").await?;
    let agent = SeededAgent::new(&pool, "t011").await?;
    let token = admin.token()?;
    let base_path = format!("/api/agents/{}/hooks", agent.id);

    // Create
    let (_, create_body) = common::post_json_auth(
        &app,
        &base_path,
        &token,
        make_webhook_payload("https://example.com/hook"),
    )
    .await?;
    let hook_id = create_body["data"]["id"].as_i64().unwrap();

    // Delete
    let (status, _) = common::delete_auth(
        &app,
        &format!("/api/agents/{}/hooks/{}", agent.id, hook_id),
        &token,
    )
    .await?;
    assert_eq!(status, StatusCode::OK);

    // Verify removed from list
    let (_, list_body) = common::get(&app, &base_path, Some(&token)).await?;
    let items = list_body["data"].as_array().unwrap();
    let found = items.iter().any(|h| h["id"].as_i64() == Some(hook_id));
    assert!(!found, "deleted hook must not appear in list");
    Ok(())
}

/// T013 — 同一触发点配置 6 个 Hook → 返回 6001
#[tokio::test]
async fn t013_trigger_limit_exceeded_returns_6001() -> anyhow::Result<()> {
    let app = common::test_app().await?;
    let pool = common::test_pool().await?;
    let admin = common::seed_admin(&pool, 3, 1, "test-pass-123").await?;
    let agent = SeededAgent::new(&pool, "t013").await?;
    let token = admin.token()?;
    let base_path = format!("/api/agents/{}/hooks", agent.id);

    // Create 5 hooks on same trigger point (should succeed)
    for i in 0..5 {
        let (status, _) = common::post_json_auth(
            &app, &base_path, &token,
            json!({
                "name": format!("hook-{i}"),
                "trigger_point": "on_agent_error",
                "action_type": "http_webhook",
                "action_params": {"webhook_url": format!("https://example.com/hook{i}"), "headers": {}},
                "sort_order": i,
            }),
        ).await?;
        assert_eq!(
            status,
            StatusCode::OK,
            "hook {i} must create OK, got {status}"
        );
    }

    // 6th must fail with 6001
    let (status, body) = common::post_json_auth(
        &app,
        &base_path,
        &token,
        json!({
            "name": "hook-6",
            "trigger_point": "on_agent_error",
            "action_type": "http_webhook",
            "action_params": {"webhook_url": "https://example.com/hook6", "headers": {}},
            "sort_order": 5,
        }),
    )
    .await?;

    assert!(status.is_client_error(), "6th hook must fail, got {status}");
    assert_eq!(
        body["code"], 6001,
        "must return 6001 trigger limit exceeded, got {body}"
    );
    Ok(())
}

// ═══════════════════════════════════════════════════════════════════
// US2 — HTTP Webhook (T014, T015, T052)
// ═══════════════════════════════════════════════════════════════════

/// T014 — http:// URL → 6003 (FR-007: only https:// allowed)
#[tokio::test]
async fn t014_non_https_webhook_url_returns_6003() -> anyhow::Result<()> {
    let app = common::test_app().await?;
    let pool = common::test_pool().await?;
    let admin = common::seed_admin(&pool, 3, 1, "test-pass-123").await?;
    let agent = SeededAgent::new(&pool, "t014").await?;

    let (status, body) = common::post_json_auth(
        &app,
        &format!("/api/agents/{}/hooks", agent.id),
        &admin.token()?,
        make_webhook_payload("http://example.com/hook"),
    )
    .await?;

    assert!(
        status.is_client_error(),
        "non-HTTPS URL must fail, got {status}"
    );
    assert_eq!(
        body["code"], 6003,
        "must return 6003 invalid webhook URL, got {body}"
    );
    Ok(())
}

/// T052 — Header injection in action_params.headers → 6003 (FR-007b)
#[tokio::test]
async fn t052_header_injection_rejected_with_6003() -> anyhow::Result<()> {
    let app = common::test_app().await?;
    let pool = common::test_pool().await?;
    let admin = common::seed_admin(&pool, 3, 1, "test-pass-123").await?;
    let agent = SeededAgent::new(&pool, "t052").await?;

    // Header name with carriage return
    let (status, body) = common::post_json_auth(
        &app,
        &format!("/api/agents/{}/hooks", agent.id),
        &admin.token()?,
        json!({
            "name": "injection-test",
            "trigger_point": "after_agent_end",
            "action_type": "http_webhook",
            "action_params": {
                "webhook_url": "https://example.com/hook",
                "headers": {
                    "X-Custom\r\nEvil": "value"
                },
            },
        }),
    )
    .await?;

    assert!(
        status.is_client_error(),
        "header injection must fail, got {status}"
    );
    assert_eq!(
        body["code"], 6003,
        "must return 6003 for header injection, got {body}"
    );

    // Header value with newline
    let (status2, body2) = common::post_json_auth(
        &app,
        &format!("/api/agents/{}/hooks", agent.id),
        &admin.token()?,
        json!({
            "name": "injection-test-2",
            "trigger_point": "after_agent_end",
            "action_type": "http_webhook",
            "action_params": {
                "webhook_url": "https://example.com/hook",
                "headers": {
                    "X-Ok": "val\nBad"
                },
            },
        }),
    )
    .await?;

    assert!(
        status2.is_client_error(),
        "header value injection must fail, got {status2}"
    );
    assert_eq!(
        body2["code"], 6003,
        "must return 6003 for header value injection"
    );
    Ok(())
}

// ═══════════════════════════════════════════════════════════════════
// US3 — Function/Workflow 调用 (T016, T017)
// ═══════════════════════════════════════════════════════════════════

/// T016 — 不存在的 function_id → 6002 (FR-006)
#[tokio::test]
async fn t016_nonexistent_function_id_returns_6002() -> anyhow::Result<()> {
    let app = common::test_app().await?;
    let pool = common::test_pool().await?;
    let admin = common::seed_admin(&pool, 3, 1, "test-pass-123").await?;
    let agent = SeededAgent::new(&pool, "t016").await?;

    let (status, body) = common::post_json_auth(
        &app,
        &format!("/api/agents/{}/hooks", agent.id),
        &admin.token()?,
        make_function_payload(99999999), // non-existent ID
    )
    .await?;

    assert!(
        status.is_client_error(),
        "nonexistent function_id must fail, got {status}"
    );
    assert_eq!(
        body["code"], 6002,
        "must return 6002 invalid reference, got {body}"
    );
    Ok(())
}

/// T017 — after_agent_end Hook (call_workflow) → workflow 被执行
#[tokio::test]
#[ignore = "Requires valid Workflow configured in the system"]
async fn t017_workflow_hook_executes_on_after_agent_end() -> anyhow::Result<()> {
    // This test requires a valid Workflow in the database.
    // Platform admin must create a Workflow before running this test.
    // Steps:
    // 1. Create hook (call_workflow) on agent 1 → trigger chat
    // 2. Verify the Workflow side effect; Hook observability is tracing-only.
    eprintln!("T017: skipped — requires pre-existing Workflow");
    Ok(())
}

// ═══════════════════════════════════════════════════════════════════
// Performance benchmarks (T054, T055)
// ═══════════════════════════════════════════════════════════════════

/// T054 — Hook 调度开销 benchmark: `run_hooks()` 空钩子列表 p95 ≤ 50ms (SC-003).
///
/// Runs `run_hooks()` 1000 times against an empty hook map and reports p50/p95/p99.
#[tokio::test]
async fn t054_hook_scheduling_overhead_benchmark() -> anyhow::Result<()> {
    use hiveweb::runtime::capability::CapabilityRegistry;
    use hiveweb::runtime::execution_context::RuntimeExecutionContext;
    use hiveweb::runtime::hook::{HookContext, HookDeps, run_hooks};
    use hiveweb::runtime::invoker::Invoker;
    use hiveweb::runtime::llm::LlmRegistry;
    use hiveweb::runtime::pool::{InstancePool, PoolConfig};
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::Instant;

    let pool = common::test_pool().await?;
    let pool = Arc::new(pool);

    // Build minimal HookDeps (never accessed when hooks list is empty)
    let s3_client = hiveweb::storage::s3::create_client().await?;
    let instance_pool = InstancePool::new(PoolConfig::from_env());
    let agent_ctx = Arc::new(agent::context::AgentContext::new(
        "bench-ctx".to_string(),
        agent::context::UserInput {
            raw_text: String::new(),
            session_id: None,
            message_id: None,
            timestamp: chrono::Utc::now(),
            metadata: std::collections::HashMap::new(),
        },
        agent::context::ContextConfig::default(),
    ));
    let deps = HookDeps {
        s3: Some(s3_client),
        llm: Arc::new(LlmRegistry::new()),
        registry: Arc::new(CapabilityRegistry::new()),
        invoker: Arc::new(Invoker::new(instance_pool)),
        ext_pool: None,
        redis: None,
        execution_context: RuntimeExecutionContext::best_effort(None, None),
        agent_ctx,
    };

    let ctx = HookContext {
        agent_id: 1,
        identifier: "bench".into(),
        session_id: 1,
        actor_id: 1,
        request_id: "req-bench".into(),
        trigger_point: "before_agent_start".into(),
        message: String::new(),
        channel: String::new(),
        client_type: String::new(),
        client_version: String::new(),
    };
    let empty_hooks: HashMap<String, Vec<hiveweb::models::agent_hook::AgentHook>> = HashMap::new();

    const ITERATIONS: usize = 1000;
    let mut durations = Vec::with_capacity(ITERATIONS);

    for _ in 0..ITERATIONS {
        let start = Instant::now();
        let _ = run_hooks(
            pool.clone(),
            &empty_hooks,
            "before_agent_start",
            &ctx,
            &deps,
        )
        .await;
        durations.push(start.elapsed().as_micros() as f64 / 1000.0); // ms
    }

    durations.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let p50 = durations[ITERATIONS / 2];
    let p95 = durations[(ITERATIONS as f64 * 0.95) as usize];
    let p99 = durations[(ITERATIONS as f64 * 0.99) as usize];
    let avg = durations.iter().sum::<f64>() / ITERATIONS as f64;

    eprintln!(
        "T054 Hook scheduling benchmark ({} iterations): avg={:.3}ms, p50={:.3}ms, p95={:.3}ms, p99={:.3}ms",
        ITERATIONS, avg, p50, p95, p99
    );

    assert!(
        p95 <= 50.0,
        "SC-003 FAIL: Hook scheduling p95 {:.3}ms exceeds 50ms budget",
        p95
    );
    Ok(())
}

// ═══════════════════════════════════════════════════════════════════
// Hook execution observability without database persistence
// ═══════════════════════════════════════════════════════════════════

/// Hook execution history is no longer exposed through an HTTP API.
#[tokio::test]
async fn t056_execution_history_endpoint_is_removed() -> anyhow::Result<()> {
    let app = common::test_app().await?;
    let pool = common::test_pool().await?;
    let admin = common::seed_admin(&pool, 3, 1, "test-pass-123").await?;

    let (status, _) = common::get(
        &app,
        "/api/agents/1/hooks/executions?page=1&page_size=20",
        Some(&admin.token()?),
    )
    .await?;

    assert!(
        matches!(
            status,
            StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED
        ),
        "removed history endpoint must not accept GET requests, got {status}"
    );
    Ok(())
}

/// Skipped and failed Hook executions must be observable only through tracing
/// and must not append rows to `hook_executions`.
#[tokio::test(flavor = "current_thread")]
async fn t057_hook_execution_does_not_persist_history() -> anyhow::Result<()> {
    use hiveweb::runtime::capability::CapabilityRegistry;
    use hiveweb::runtime::execution_context::RuntimeExecutionContext;
    use hiveweb::runtime::hook::{HookContext, HookDeps, run_hooks};
    use hiveweb::runtime::invoker::Invoker;
    use hiveweb::runtime::llm::LlmRegistry;
    use hiveweb::runtime::pool::{InstancePool, PoolConfig};
    use std::collections::HashMap;
    use std::sync::Arc;

    let pool = Arc::new(common::test_pool().await?);
    let request_id = format!("req-t057-{}", uuid::Uuid::new_v4());
    let now = chrono::Utc::now();
    let skipped_hook = hiveweb::models::agent_hook::AgentHook {
        id: -57,
        agent_id: 1,
        name: "trace-only-hook".into(),
        description: None,
        trigger_point: "before_agent_start".into(),
        action_type: "http_webhook".into(),
        action_params: json!({}),
        enabled: false,
        sort_order: 0,
        blocking_mode: false,
        timeout_ms: 1_000,
        created_at: now,
        updated_at: now,
    };
    let failed_hook = hiveweb::models::agent_hook::AgentHook {
        id: -58,
        agent_id: 1,
        name: "trace-only-failed-hook".into(),
        description: None,
        trigger_point: "before_agent_start".into(),
        action_type: "http_webhook".into(),
        action_params: json!({}),
        enabled: true,
        sort_order: 1,
        blocking_mode: false,
        timeout_ms: 1_000,
        created_at: now,
        updated_at: now,
    };
    let hooks = HashMap::from([(
        "before_agent_start".to_string(),
        vec![skipped_hook, failed_hook],
    )]);

    let agent_ctx = Arc::new(agent::context::AgentContext::new(
        "trace-only-test".to_string(),
        agent::context::UserInput {
            raw_text: String::new(),
            session_id: Some("57".into()),
            message_id: None,
            timestamp: now,
            metadata: HashMap::new(),
        },
        agent::context::ContextConfig::default(),
    ));
    let deps = HookDeps {
        s3: Some(hiveweb::storage::s3::create_client().await?),
        llm: Arc::new(LlmRegistry::new()),
        registry: Arc::new(CapabilityRegistry::new()),
        invoker: Arc::new(Invoker::new(InstancePool::new(PoolConfig::from_env()))),
        ext_pool: None,
        redis: None,
        execution_context: RuntimeExecutionContext::best_effort(Some(request_id.clone()), Some(57)),
        agent_ctx,
    };
    let ctx = HookContext {
        agent_id: 1,
        identifier: "trace-only-agent".into(),
        session_id: 57,
        actor_id: 1,
        request_id: request_id.clone(),
        trigger_point: "before_agent_start".into(),
        message: "must-not-be-persisted".into(),
        channel: "test".into(),
        client_type: "test".into(),
        client_version: "test".into(),
    };

    let trace_writer = TraceWriter::default();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_target(false)
        .with_writer(trace_writer.clone())
        .finish();
    let _trace_guard = tracing::subscriber::set_default(subscriber);

    run_hooks(Arc::clone(&pool), &hooks, "before_agent_start", &ctx, &deps)
        .await
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;

    let trace_output = String::from_utf8(
        trace_writer
            .0
            .lock()
            .expect("trace buffer poisoned")
            .clone(),
    )?;
    assert!(trace_output.contains("hook_execution"));
    assert!(trace_output.contains("webhook_action_failed"));
    assert!(trace_output.contains(&request_id));
    assert!(!trace_output.contains("must-not-be-persisted"));

    let persisted: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM hook_executions WHERE request_id = ?")
            .bind(&request_id)
            .fetch_one(pool.as_ref())
            .await?;
    assert_eq!(persisted, 0, "Hook execution must not be persisted");

    Ok(())
}

/// All Hook runtime paths, including asynchronous Webhook retries, must remain
/// free of writes to the legacy execution-history table.
#[test]
fn t058_hook_runtime_has_no_execution_history_insert() {
    let source = include_str!("../src/runtime/hook.rs");
    assert!(
        !source.contains("INSERT INTO hook_executions"),
        "Hook runtime must use tracing instead of execution-history INSERTs"
    );
    assert!(
        source.contains("error_kind = %error_kind"),
        "Hook tracing must emit only a bounded error category"
    );
    assert!(
        !source.contains("Some(&e.to_string())"),
        "Hook tracing must not emit arbitrary downstream error text"
    );
}
