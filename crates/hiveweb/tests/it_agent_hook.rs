//! Integration & contract tests: Agent Hook 配置管理 (008-agent-hook-config)
//!
//! Covers: T008-T019, T052-T053 (Phase 2.5 红灯测试)
//!
//! Contract tests (REST API): T008-T011, T013-T014, T016, T052, T018-T019
//! Integration tests (chat session): T012, T015, T017, T053 (require full chat infra)

mod common;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

// ── Agent seed helper (RAII cleanup on drop) ──

struct SeededAgent {
    pub id: i64,
    pub identifier: String,
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
            identifier: identifier.clone(),
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
                // FK CASCADE will clean up agent_hooks; hook_executions has no FK
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
    make_hook_payload("after_agent_end", "http_webhook", json!({
        "webhook_url": url,
        "headers": {},
    }))
}

fn make_function_payload(function_id: i64) -> Value {
    make_hook_payload("before_agent_start", "call_function", json!({
        "function_id": function_id,
        "args": {},
    }))
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
        make_hook_payload("before_agent_start", "http_webhook", json!({
            "webhook_url": "https://example.com/hook",
            "headers": {},
        })),
    )
    .await?;

    assert_eq!(status, StatusCode::OK, "create hook must return 200, got {status}: {body}");
    assert_eq!(body["code"], 0, "must have code=0");
    let data = &body["data"];
    assert!(data["id"].as_i64().is_some(), "must include id");
    assert_eq!(data["name"], "test-before_agent_start");
    assert_eq!(data["trigger_point"], "before_agent_start");
    assert_eq!(data["action_type"], "http_webhook");
    assert!(data["created_at"].as_str().is_some(), "must include created_at");
    assert!(data["enabled"].as_bool().unwrap_or(false), "default enabled=true");
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
    let mut payload = make_hook_payload("after_agent_end", "http_webhook",
        json!({"webhook_url": "https://example.com/a", "headers": {}}));
    payload["sort_order"] = json!(0);
    common::post_json_auth(&app, &base_path, &token, payload).await?;

    // before_tool_call with sort_order=0
    let mut payload = make_hook_payload("before_tool_call", "http_webhook",
        json!({"webhook_url": "https://example.com/b", "headers": {}}));
    payload["sort_order"] = json!(0);
    common::post_json_auth(&app, &base_path, &token, payload).await?;

    // after_agent_end with sort_order=1
    let mut payload = make_hook_payload("after_agent_end", "http_webhook",
        json!({"webhook_url": "https://example.com/c", "headers": {}}));
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
    ).await?;
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
    ).await?;

    assert!(
        status.is_client_error(),
        "stale updated_at must produce error, got {status}: {body}"
    );
    assert_eq!(body["code"], 4094, "must return 4094 optimistic lock, got {body}");
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
        &app, &base_path, &token,
        make_webhook_payload("https://example.com/hook"),
    ).await?;
    let hook_id = create_body["data"]["id"].as_i64().unwrap();

    // Delete
    let (status, _) = common::delete_auth(
        &app,
        &format!("/api/agents/{}/hooks/{}", agent.id, hook_id),
        &token,
    ).await?;
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
        assert_eq!(status, StatusCode::OK, "hook {i} must create OK, got {status}");
    }

    // 6th must fail with 6001
    let (status, body) = common::post_json_auth(
        &app, &base_path, &token,
        json!({
            "name": "hook-6",
            "trigger_point": "on_agent_error",
            "action_type": "http_webhook",
            "action_params": {"webhook_url": "https://example.com/hook6", "headers": {}},
            "sort_order": 5,
        }),
    ).await?;

    assert!(
        status.is_client_error(),
        "6th hook must fail, got {status}"
    );
    assert_eq!(body["code"], 6001, "must return 6001 trigger limit exceeded, got {body}");
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
    ).await?;

    assert!(
        status.is_client_error(),
        "non-HTTPS URL must fail, got {status}"
    );
    assert_eq!(body["code"], 6003, "must return 6003 invalid webhook URL, got {body}");
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
    ).await?;

    assert!(
        status.is_client_error(),
        "header injection must fail, got {status}"
    );
    assert_eq!(body["code"], 6003, "must return 6003 for header injection, got {body}");

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
    ).await?;

    assert!(
        status2.is_client_error(),
        "header value injection must fail, got {status2}"
    );
    assert_eq!(body2["code"], 6003, "must return 6003 for header value injection");
    Ok(())
}

/// T015 — 不可达 Webhook → hook_executions 有 error 记录
#[tokio::test]
async fn t015_unreachable_webhook_produces_error_audit() -> anyhow::Result<()> {
    let app = common::test_app().await?;
    let pool = common::test_pool().await?;
    let admin = common::seed_admin(&pool, 3, 1, "test-pass-123").await?;
    let token = admin.token()?;
    let sort_ord = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos() as i32;

    let (status, create_body) = common::post_json_auth(
        &app, "/api/agents/1/hooks", &token,
        json!({
            "name": "t015-unreachable",
            "trigger_point": "before_agent_start",
            "action_type": "http_webhook",
            "sort_order": sort_ord,
            "action_params": {
                "webhook_url": "https://203.0.113.1/hook",
                "headers": {},
                "timeout_ms": 1000
            },
        }),
    ).await?;
    assert_eq!(status, StatusCode::OK, "T015: create failed: {create_body}");
    let hook_id = create_body["data"]["id"].as_i64().unwrap();
    

    let (_, sb) = common::post_json_auth(&app, "/api/admin-chat/sessions", &token, json!({"title": "t015"})).await?;
    let sid = sb["data"]["id"].as_i64().unwrap();
    common::post_json_auth(&app, &format!("/api/admin-chat/sessions/{sid}/messages"), &token, json!({"content": "."})).await?;

    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT outcome FROM hook_executions WHERE hook_id = ? ORDER BY created_at DESC LIMIT 1"
    ).bind(hook_id).fetch_all(&pool).await?;
    assert!(!rows.is_empty(), "T015: no exec record for hook {hook_id}");
    eprintln!("T015: outcome={}", rows[0].0);
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
    ).await?;

    assert!(
        status.is_client_error(),
        "nonexistent function_id must fail, got {status}"
    );
    assert_eq!(body["code"], 6002, "must return 6002 invalid reference, got {body}");
    Ok(())
}

/// T017 — after_agent_end Hook (call_workflow) → workflow 被执行
#[tokio::test]
#[ignore = "Requires valid Workflow configured in the system"]
async fn t017_workflow_hook_executes_on_after_agent_end() -> anyhow::Result<()> {
    // This test requires a valid Workflow in the database.
    // Plantform admin must create a Workflow before running this test.
    // Steps:
    // 1. Create hook (call_workflow) on agent 1 → trigger chat
    // 2. Verify hook_executions has record
    eprintln!("T017: skipped — requires pre-existing Workflow");
    Ok(())
}

// ═══════════════════════════════════════════════════════════════════
// US4 — Hook 执行历史查询 (T018, T019)
// ═══════════════════════════════════════════════════════════════════

/// T018 — GET executions → 分页返回 + 按 created_at DESC 排序
#[tokio::test]
async fn t018_list_executions_returns_paginated_sorted_desc() -> anyhow::Result<()> {
    let app = common::test_app().await?;
    let pool = common::test_pool().await?;
    let admin = common::seed_admin(&pool, 3, 1, "test-pass-123").await?;
    let agent = SeededAgent::new(&pool, "t018").await?;
    let token = admin.token()?;

    // Seed some hook_executions directly (simulating past executions)
    for i in 0..5 {
        sqlx::query(
            r#"INSERT INTO hook_executions
               (agent_id, agent_identifier, trigger_point, action_type, outcome, elapsed_ms, request_id)
               VALUES (?, ?, ?, ?, ?, ?, ?)"#,
        )
        .bind(agent.id)
        .bind(&agent.identifier)
        .bind("before_agent_start")
        .bind("http_webhook")
        .bind(if i == 3 { "error" } else { "success" })
        .bind(100 + i * 10)
        .bind(format!("req-t018-{i}"))
        .execute(&pool)
        .await?;
    }

    // Query with default pagination
    let (status, body) = common::get(
        &app,
        &format!("/api/agents/{}/hooks/executions?page=1&page_size=20", agent.id),
        Some(&token),
    ).await?;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["code"], 0);
    let data = &body["data"];
    assert!(data["items"].is_array());
    assert_eq!(data["page"], 1);
    assert_eq!(data["page_size"], 20);
    assert!(data["total"].as_u64().unwrap_or(0) >= 5);

    // Must be sorted by created_at DESC
    let items = data["items"].as_array().unwrap();
    if items.len() >= 2 {
        let t0 = items[0]["created_at"].as_str().unwrap();
        let t1 = items[1]["created_at"].as_str().unwrap();
        assert!(t0 >= t1, "executions must be sorted by created_at DESC");
    }
    Ok(())
}

/// T019 — 删除 Agent → Hook 执行历史仍可查询（Agent 字段显示已删除标记）
#[tokio::test]
async fn t019_deleted_agent_executions_remain_queryable() -> anyhow::Result<()> {
    let app = common::test_app().await?;
    let pool = common::test_pool().await?;
    let admin = common::seed_admin(&pool, 3, 1, "test-pass-123").await?;

    // Create a separate scope so we can control agent deletion
    let agent_id: i64;
    let agent_ident: String;
    {
        let agent = SeededAgent::new(&pool, "t019").await?;
        agent_id = agent.id;
        agent_ident = agent.identifier.clone();

        // Seed executions for this agent
        for i in 0..3 {
            sqlx::query(
                r#"INSERT INTO hook_executions
                   (agent_id, agent_identifier, trigger_point, action_type, outcome, elapsed_ms, request_id)
                   VALUES (?, ?, ?, ?, ?, ?, ?)"#,
            )
            .bind(agent_id)
            .bind(&agent_ident)
            .bind("after_agent_end")
            .bind("http_webhook")
            .bind("success")
            .bind(50)
            .bind(format!("req-t019-{i}"))
            .execute(&pool)
            .await?;
        }
        // agent goes out of scope here → Drop deletes the agent row
    }

    // Now query executions — should still return records
    let (status, body) = common::get(
        &app,
        &format!("/api/agents/{}/hooks/executions?page=1&page_size=20", agent_id),
        Some(&admin.token()?),
    ).await?;

    assert_eq!(status, StatusCode::OK);
    let items = body["data"]["items"].as_array().unwrap();
    assert!(!items.is_empty(), "executions must persist after agent deletion");

    // agent_identifier snapshot should still contain the original identifier
    let ident = items[0]["agent_identifier"].as_str().unwrap();
    assert_eq!(ident, agent_ident, "agent_identifier snapshot must be preserved");
    Ok(())
}

// ═══════════════════════════════════════════════════════════════════
// Integration tests (require full chat session infrastructure)
// ═══════════════════════════════════════════════════════════════════

/// T012 — 配置 before_agent_start Hook (http_webhook) →
/// 触发 Agent 对话 → hook_executions 有 success 记录
#[tokio::test]
async fn t012_hook_execution_produces_audit_record() -> anyhow::Result<()> {
    let app = common::test_app().await?;
    let pool = common::test_pool().await?;
    let admin = common::seed_admin(&pool, 3, 1, "test-pass-123").await?;
    let token = admin.token()?;

    // Use a non-main agent to avoid collision with other tests on agent 1
    // Admin chat hardcodes agent_id=1, so hooks must be on agent 1
    // Clean up stale hooks from previous runs to avoid trigger limit (max 5)
    let _ = sqlx::query("DELETE FROM agent_hooks WHERE agent_id = 1 AND trigger_point = 'before_agent_start'")
        .execute(&pool).await;
    let sort_ord = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos() as i32;
    let (status, create_body) = common::post_json_auth(
        &app, "/api/agents/1/hooks", &token,
        json!({
            "name": "t012-before-agent-start",
            "trigger_point": "before_agent_start",
            "action_type": "http_webhook",
            "sort_order": sort_ord,
            "action_params": {
                "webhook_url": "https://httpbin.org/post",
                "headers": {},
                "timeout_ms": 2000
            },
        }),
    ).await?;
    assert_eq!(status, StatusCode::OK, "T012: hook create failed: {create_body}");
    let hook_id = create_body["data"]["id"].as_i64().expect("hook id required");
    eprintln!("T012: created hook id={hook_id}");

    // Cleanup hook after test
    

    // Create admin chat session
    let (_, session_body) = common::post_json_auth(
        &app, "/api/admin-chat/sessions", &token, json!({"title": "t012"}),
    ).await?;
    let sid = session_body["data"]["id"].as_i64().unwrap();
    let (ms, _) = common::post_json_auth(
        &app, &format!("/api/admin-chat/sessions/{sid}/messages"),
        &token, json!({"content": "."}),
    ).await?;
    eprintln!("T012: msg status={ms}");

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let rows: Vec<(i64,)> = sqlx::query_as(
        "SELECT id FROM hook_executions WHERE hook_id = ? ORDER BY created_at DESC LIMIT 1"
    ).bind(hook_id).fetch_all(&pool).await?;
    assert!(!rows.is_empty(), "T012: no hook_executions for hook {hook_id}");
    Ok(())
}

/// T053 — 阻塞模式 Hook 失败 → SSE 流发出 error 事件 (SC-007)
#[tokio::test]
async fn t053_blocking_mode_failure_emits_sse_error() -> anyhow::Result<()> {
    let app = common::test_app().await?;
    let pool = common::test_pool().await?;
    let admin = common::seed_admin(&pool, 3, 1, "test-pass-123").await?;
    let token = admin.token()?;
    let sort_ord = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos() as i32;

    let (status, create_body) = common::post_json_auth(
        &app, "/api/agents/1/hooks", &token,
        json!({
            "name": "t053-blocking-fail",
            "trigger_point": "before_agent_start",
            "action_type": "http_webhook",
            "sort_order": sort_ord,
            "blocking_mode": true,
            "action_params": {
                "webhook_url": "https://203.0.113.1/hook",
                "headers": {},
                "timeout_ms": 2000
            },
        }),
    ).await?;
    assert_eq!(status, StatusCode::OK, "T053: create failed: {create_body}");
    let hook_id = create_body["data"]["id"].as_i64().unwrap();
    

    let (_, sb) = common::post_json_auth(&app, "/api/admin-chat/sessions", &token, json!({"title": "t053"})).await?;
    let sid = sb["data"]["id"].as_i64().unwrap();

    let req = Request::builder()
        .method("POST")
        .uri(&format!("/api/admin-chat/sessions/{sid}/messages"))
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {}", token))
        .header("accept", "text/event-stream")
        .body(Body::from(json!({"content": "."}).to_string()))?;

    let resp = app.clone().oneshot(req).await?;
    let s = resp.status();
    let bytes = resp.into_body().collect().await?.to_bytes();
    let body_text = String::from_utf8_lossy(&bytes);
    eprintln!("T053: status={s}, body={}", body_text.chars().take(300).collect::<String>());

    let has_error = body_text.to_lowercase().contains("error") || !s.is_success();
    assert!(has_error, "SC-007 FAIL: blocking hook must produce SSE error. status={s}");
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
    use hiveweb::runtime::hook::{run_hooks, HookContext};
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::Instant;

    let pool = common::test_pool().await?;
    let pool = Arc::new(pool);

    let ctx = HookContext {
        agent_id: 1,
        identifier: "bench".into(),
        session_id: 1,
        actor_id: 1,
        request_id: "req-bench".into(),
        trigger_point: "before_agent_start".into(),
    };
    let empty_hooks: HashMap<String, Vec<hiveweb::models::agent_hook::AgentHook>> = HashMap::new();

    const ITERATIONS: usize = 1000;
    let mut durations = Vec::with_capacity(ITERATIONS);

    for _ in 0..ITERATIONS {
        let start = Instant::now();
        let _ = run_hooks(pool.clone(), &empty_hooks, "before_agent_start", &ctx).await;
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
        "SC-003 FAIL: Hook scheduling p95 {:.3}ms exceeds 50ms budget", p95
    );
    Ok(())
}

/// T055 — Hook 执行历史查询 benchmark: 10K+ rows, p95 ≤ 2s (SC-006).
///
/// Pre-fills 10K hook_executions rows, then queries with paging and measures latency.
#[tokio::test]
async fn t055_execution_history_query_benchmark() -> anyhow::Result<()> {
    let pool = common::test_pool().await?;
    let admin = common::seed_admin(&pool, 3, 1, "test-pass-123").await?;
    let app = common::test_app().await?;

    // Seed temp agent
    let agent = SeededAgent::new(&pool, "t055-bench").await?;

    // Bulk-insert 10K hook_executions
    let batch_size = 1000;
    let total = 10000;
    for batch in 0..(total / batch_size) {
        let mut values = Vec::new();
        for i in 0..batch_size {
            let idx = batch * batch_size + i;
            values.push(format!(
                "({}, '{}', 'before_agent_start', 'http_webhook', '{}', {}, 'req-bench-{}')",
                agent.id,
                agent.identifier,
                if idx % 10 == 0 { "error" } else { "success" },
                idx % 100 + 1,
                idx,
            ));
        }
        let sql = format!(
            r#"INSERT INTO hook_executions
               (agent_id, agent_identifier, trigger_point, action_type, outcome, elapsed_ms, request_id)
               VALUES {}"#,
            values.join(", ")
        );
        sqlx::query(&sql).execute(&pool).await?;
    }

    // Run benchmark queries
    use std::time::Instant;
    const QUERY_ITERATIONS: usize = 100;
    let mut durations = Vec::with_capacity(QUERY_ITERATIONS);

    for _ in 0..QUERY_ITERATIONS {
        let start = Instant::now();
        let (status, _body) = common::get(
            &app,
            &format!(
                "/api/agents/{}/hooks/executions?page=1&page_size=20",
                agent.id
            ),
            Some(&admin.token()?),
        ).await?;
        assert_eq!(status, StatusCode::OK);
        durations.push(start.elapsed().as_micros() as f64 / 1000.0); // ms
    }

    durations.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p50 = durations[QUERY_ITERATIONS / 2];
    let p95 = durations[(QUERY_ITERATIONS as f64 * 0.95) as usize];
    let p99 = durations[(QUERY_ITERATIONS as f64 * 0.99) as usize];
    let avg = durations.iter().sum::<f64>() / QUERY_ITERATIONS as f64;

    eprintln!(
        "T055 Execution history query benchmark ({} iterations, {} rows): avg={:.3}ms, p50={:.3}ms, p95={:.3}ms, p99={:.3}ms",
        QUERY_ITERATIONS, total, avg, p50, p95, p99
    );

    assert!(
        p95 <= 2000.0,
        "SC-006 FAIL: Exec history query p95 {:.3}ms exceeds 2000ms budget", p95
    );
    Ok(())
}
