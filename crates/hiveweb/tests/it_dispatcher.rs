//! Dispatcher 安全门集成测试 (T046..T050 / US4)
//!
//! 直接调 `runtime::capability::dispatch`，绕过 Extism，验证 4 个安全门：
//!   1. 未知 capability → 4045
//!   2. 已知但 Agent 未授权 → 4030
//!   3. 已知且授权 → ok=true
//!   4. 非法 envelope JSON → 4001（T080 §3 调和：envelope 解析失败统一映射为
//!      共享 ABI `invalid_args`，而非 API 层 `BAD_REQUEST` 4000）

mod common;

use std::sync::Arc;

use hiveweb::runtime::RuntimeExecutionContext;
use hiveweb::runtime::capabilities::rate_limit::CapabilityRateLimits;
use hiveweb::runtime::capability::{
    self, CapabilityRegistry, DispatchCtx, DispatcherDeps, NETWORK_HTTP, TIME_NOW,
};

#[tokio::test]
async fn t046_unknown_capability_returns_4045() -> anyhow::Result<()> {
    let pool = common::test_pool().await?;
    let s3 = hiveweb::storage::s3::create_client().await?;
    let deps = DispatcherDeps {
        pool,
        s3: Some(s3),
        registry: Arc::new(CapabilityRegistry::new()),
        llm: Arc::new(hiveweb::runtime::LlmRegistry::new()),
        rate_limits: Arc::new(CapabilityRateLimits::new()),
    };
    let ctx = DispatchCtx {
        execution_context: RuntimeExecutionContext::best_effort(None, None),
        agent_id: 1, // main agent
        plugin_id: 9999,
        function_id: None,
        permissions: vec![],
    };
    let envelope = r#"{"capability":"no.such.cap","args":{}}"#;
    let resp = capability::dispatch(&deps, &ctx, envelope).await;
    let v: serde_json::Value = serde_json::from_str(&resp)?;
    assert_eq!(v["ok"], false);
    assert_eq!(
        v["code"], 4045,
        "unknown capability must return 4045; got {resp}"
    );
    Ok(())
}

#[tokio::test]
async fn t047_known_but_not_granted_returns_4030() -> anyhow::Result<()> {
    let pool = common::test_pool().await?;
    let s3 = hiveweb::storage::s3::create_client().await?;
    let deps = DispatcherDeps {
        pool: pool.clone(),
        s3: Some(s3),
        registry: Arc::new(CapabilityRegistry::new()),
        llm: Arc::new(hiveweb::runtime::LlmRegistry::new()),
        rate_limits: Arc::new(CapabilityRateLimits::new()),
    };

    // 准备：确保 main agent (id=1) 没有 network.http 授权
    sqlx::query("DELETE FROM agent_permissions WHERE agent_id = 1 AND capability = ?")
        .bind(NETWORK_HTTP)
        .execute(&pool)
        .await?;

    let ctx = DispatchCtx {
        execution_context: RuntimeExecutionContext::best_effort(None, None),
        agent_id: 1,
        plugin_id: 9999,
        function_id: None,
        permissions: vec![],
    };
    let envelope = format!(r#"{{"capability":"{NETWORK_HTTP}","args":{{}}}}"#);
    let resp = capability::dispatch(&deps, &ctx, &envelope).await;
    let v: serde_json::Value = serde_json::from_str(&resp)?;
    assert_eq!(v["ok"], false);
    assert_eq!(
        v["code"], 4030,
        "ungranted capability must return 4030; got {resp}"
    );
    Ok(())
}

#[tokio::test]
async fn t048_granted_time_now_returns_ok_with_data() -> anyhow::Result<()> {
    let pool = common::test_pool().await?;
    let s3 = hiveweb::storage::s3::create_client().await?;
    let deps = DispatcherDeps {
        pool: pool.clone(),
        s3: Some(s3),
        registry: Arc::new(CapabilityRegistry::new()),
        llm: Arc::new(hiveweb::runtime::LlmRegistry::new()),
        rate_limits: Arc::new(CapabilityRateLimits::new()),
    };

    // 准备：给 main agent (id=1) 临时授权 time.now
    sqlx::query("INSERT IGNORE INTO agent_permissions (agent_id, capability) VALUES (1, ?)")
        .bind(TIME_NOW)
        .execute(&pool)
        .await?;

    let ctx = DispatchCtx {
        execution_context: RuntimeExecutionContext::best_effort(None, None),
        agent_id: 1,
        plugin_id: 9999,
        function_id: None,
        permissions: vec![],
    };
    let envelope = format!(r#"{{"capability":"{TIME_NOW}","args":{{}}}}"#);
    let resp = capability::dispatch(&deps, &ctx, &envelope).await;
    let v: serde_json::Value = serde_json::from_str(&resp)?;
    assert_eq!(v["ok"], true, "granted time.now should succeed; got {resp}");
    assert!(
        v["data"]["unix_ms"].is_number(),
        "data.unix_ms missing: {resp}"
    );
    assert!(
        v["data"]["rfc3339"].is_string(),
        "data.rfc3339 missing: {resp}"
    );

    // 清理（恢复原状）
    sqlx::query("DELETE FROM agent_permissions WHERE agent_id = 1 AND capability = ?")
        .bind(TIME_NOW)
        .execute(&pool)
        .await?;
    Ok(())
}

#[tokio::test]
async fn t049_bad_envelope_returns_4001() -> anyhow::Result<()> {
    let pool = common::test_pool().await?;
    let s3 = hiveweb::storage::s3::create_client().await?;
    let deps = DispatcherDeps {
        pool,
        s3: Some(s3),
        registry: Arc::new(CapabilityRegistry::new()),
        llm: Arc::new(hiveweb::runtime::LlmRegistry::new()),
        rate_limits: Arc::new(CapabilityRateLimits::new()),
    };
    let ctx = DispatchCtx {
        execution_context: RuntimeExecutionContext::best_effort(None, None),
        agent_id: 1,
        plugin_id: 9999,
        function_id: None,
        permissions: vec![],
    };
    let resp = capability::dispatch(&deps, &ctx, "{not-json}").await;
    let v: serde_json::Value = serde_json::from_str(&resp)?;
    assert_eq!(v["ok"], false);
    // Envelope 解析失败统一映射为共享 ABI 的 `invalid_args` (4001)，而非
    // API 层 `BAD_REQUEST` (4000)（T080 §3 错误码调和）。
    assert_eq!(v["code"], 4001, "bad envelope must return 4001; got {resp}");
    Ok(())
}
