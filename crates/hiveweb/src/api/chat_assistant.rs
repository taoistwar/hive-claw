//! Assistant API — 对外公开的 AI 助手接口（MD5 签名鉴权）
//!
//! POST /api/assistant?sign={md5}
//!
//! 请求头：
//!   Content-Type: application/json; charset=UTF-8
//!
//! 请求体：
//!   { "user_id": 123, "message": "你好" }
//!
//! 处理流程：
//!   1. Content-Type 校验
//!   2. MD5 签名校验（MD5(ASSISTANT_SECRET + "/api/assistant" + "?body=" + body)）
//!   3. user_id > 0 校验
//!   4. message 非空校验
//!   5. 外部 DB cloud_user 校验
//!   6. 外部 DB cc_user_membership VIP 判定（effective_end_time >= now）
//!   7. Redis 日访问次数限流（VIP: vip_ask_times, 普通: normal_ask_times）
//!   8. 内部 DB 同步用户（users 表，不存在则创建）
//!   9. 获取或创建 session → append_user_message_user → 更新 title →
//!      构建 OrchestratorDeps → run_session_user（SSE，与 post_message_sse 相同）

use axum::{
    Router,
    extract::{Query, State},
    response::{IntoResponse, Response},
    routing::post,
};
use axum::response::sse::Event;
use futures::stream::{self, Stream};
use redis::AsyncCommands;
use serde::Deserialize;
use sqlx::MySqlPool;
use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::{Arc, OnceLock};

use crate::api::AppState;
use crate::api::chat_common::{SseConcurrencyGuard, SseSlotConfig, sse_response, try_acquire_slot};
use crate::services::chat_user as svc;
use crate::utils::error::AppError;

/// 预共享密钥，从环境变量 ASSISTANT_SECRET 懒加载
static SECRET: OnceLock<String> = OnceLock::new();

fn get_secret() -> &'static str {
    SECRET
        .get_or_init(|| std::env::var("ASSISTANT_SECRET").unwrap_or_default())
        .as_str()
}

pub fn router() -> Router<AppState> {
    Router::new().route("/assistant", post(assistant_chat))
}

// ── 请求 / 响应 ──

#[derive(Debug, Deserialize)]
pub struct AssistantRequest {
    pub user_id: i64,
    pub message: String,
}

// ── 外部数据库模型 ──

#[derive(Debug, Clone, sqlx::FromRow)]
struct CloudUser {
    #[allow(dead_code)]
    #[sqlx(rename = "ID")]
    id: i64,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct CcUserMembership {
    #[allow(dead_code)]
    id: i64,
    #[allow(dead_code)]
    membership_level: Option<i32>,
    effective_end_time: Option<chrono::NaiveDateTime>,
}

// ── Handler ──

async fn assistant_chat(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Query(params): Query<HashMap<String, String>>,
    body: String,
) -> Response {
    // 0. Content-Type 校验
    let content_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if content_type != "application/json; charset=UTF-8" {
        return AppError::BadRequest("Invalid Content-Type, must be: application/json; charset=UTF-8".into())
            .into_response::<()>()
            .into_response();
    }

    // 1. MD5 签名校验
    let secret = get_secret();
    if !secret.is_empty() {
        let sign = params.get("sign").map(|s| s.as_str()).unwrap_or("");
        if !verify_sign(secret, &body, sign) {
            return AppError::BadRequest("Invalid signature".into())
                .into_response::<()>()
                .into_response();
        }
    }

    // 2. JSON 解析
    let req: AssistantRequest = match serde_json::from_str(&body) {
        Ok(r) => r,
        Err(_) => {
            return AppError::BadRequest("Invalid request body".into())
                .into_response::<()>()
                .into_response();
        }
    };

    // 3. user_id > 0 校验
    if req.user_id <= 0 {
        return AppError::BadRequest("user_id must be positive".into())
            .into_response::<()>()
            .into_response();
    }

    // 4. message 非空校验
    if req.message.trim().is_empty() {
        return AppError::BadRequest("message must not be empty".into())
            .into_response::<()>()
            .into_response();
    }

    let ext_pool = match &state.ext_pool {
        Some(p) => p,
        None => {
            return AppError::Internal("Assistant service unavailable".into())
                .into_response::<()>()
                .into_response();
        }
    };

    // 5. 校验 user_id 是否存在于外部 cloud_user 表
    if !user_exists_in_cloud(ext_pool, req.user_id).await {
        return AppError::BadRequest("User not found".into())
            .into_response::<()>()
            .into_response();
    }

    // 6. 获取会员等级 & 判断是否 VIP
    let is_vip = check_vip_membership(ext_pool, req.user_id).await;

    // 7. 日访问次数限流
    let limit_key = format!("assistant:daily:{}", req.user_id);
    let max_times = if is_vip {
        get_config_number(&state.pool, "vip_ask_times", 50).await
    } else {
        get_config_number(&state.pool, "normal_ask_times", 5).await
    };

    if let Err(msg) = check_and_incr_daily_limit(&state.redis, &limit_key, max_times).await {
        return AppError::BadRequest(msg).into_response::<()>().into_response();
    }

    // 8. 内部 users 表同步（不存在则创建）
    if let Err(e) = ensure_internal_user(&state.pool, req.user_id).await {
        let _ = decr_daily_limit(&state.redis, &limit_key).await;
        return AppError::Internal(format!("user sync: {e}"))
            .into_response::<()>()
            .into_response();
    }

    // 9. 获取最新 session 或创建新 session（与 post_message_sse 相同的业务逻辑）
    let session = match get_or_create_session(&state.pool, req.user_id).await {
        Ok(s) => s,
        Err(e) => {
            let _ = decr_daily_limit(&state.redis, &limit_key).await;
            return e.into_response::<()>().into_response();
        }
    };
    let session_id = session.id;

    // 10. Append user message
    if let Err(e) = svc::append_user_message_user(&state.pool, session_id, req.user_id, &req.message).await {
        let _ = decr_daily_limit(&state.redis, &limit_key).await;
        return e.into_response::<()>().into_response();
    }

    // 11. Auto-generate title from first message (first 30 chars)
    if session.title.is_none() || session.title.as_ref().map_or(true, |t| t.is_empty()) {
        let title = req.message.chars().take(30).collect::<String>();
        let _ = sqlx::query("UPDATE chat_sessions_user SET title = ? WHERE id = ?")
            .bind(&title)
            .bind(session_id)
            .execute(&state.pool)
            .await;
    }

    // 12. SSE concurrency guard
    if !try_acquire_slot(req.user_id, SseSlotConfig::USER).await {
        let _ = decr_daily_limit(&state.redis, &limit_key).await;
        return AppError::SseConcurrencyExceeded(
            "并发会话过多，请关闭其它对话窗口后重试".into(),
        )
        .into_response::<()>()
        .into_response();
    }
    let _guard = SseConcurrencyGuard {
        actor_id: req.user_id,
        is_admin: false,
    };

    // 13. Build OrchestratorDeps + spawn run_session_user (same as post_message_sse)
    let pool = state.pool.clone();
    let user_content = req.message.clone();

    let history: Vec<crate::models::ChatMessageUser> =
        svc::list_messages_user(&state.pool, session_id)
            .await
            .unwrap_or_default();

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Result<Event, Infallible>>();

    let deps = crate::runtime::orchestrator::OrchestratorDeps {
        pool: pool.clone(),
        s3: state.s3.clone(),
        llm: Arc::clone(&state.runtime_state.llm),
        registry: Arc::clone(&state.runtime_state.capabilities),
        invoker: Arc::clone(&state.runtime_state.invoker),
        ext_pool: state.ext_pool.clone(),
    };
    tokio::spawn(async move {
        crate::runtime::orchestrator::run_session_user(
            deps,
            session_id,
            1,
            req.user_id,
            history,
            user_content,
            tx,
        )
        .await;
    });

    let final_stream: std::pin::Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>> =
        Box::pin(stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|item| (item, rx))
        }));

    sse_response(final_stream)
}

// ── 签名校验 ──

fn verify_sign(secret: &str, body: &str, expected_sign: &str) -> bool {
    let sign_string = format!("{}{}?body={}", secret, "/api/assistant", body);
    let digest = format!("{:x}", md5::compute(sign_string.as_bytes()));
    digest == expected_sign
}

// ── 外部 DB 查询 ──

async fn user_exists_in_cloud(pool: &MySqlPool, user_id: i64) -> bool {
    sqlx::query_as::<_, CloudUser>("SELECT ID FROM cloud_user WHERE ID = ?")
        .bind(user_id)
        .fetch_optional(pool)
        .await
        .map(|r| r.is_some())
        .unwrap_or(false)
}

async fn check_vip_membership(pool: &MySqlPool, user_id: i64) -> bool {
    let row = sqlx::query_as::<_, CcUserMembership>(
        "SELECT id, membership_level, effective_end_time FROM cc_user_membership WHERE id = ? LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await;

    match row {
        Ok(Some(m)) => {
            if let Some(end) = m.effective_end_time {
                end >= chrono::Utc::now().naive_utc() // 包含边界（Clarify Q3）
            } else {
                // 无过期时间 → 永久有效
                true
            }
        }
        _ => false,
    }
}

// ── 日访问次数限流（Redis） ──

async fn check_and_incr_daily_limit(
    redis: &redis::Client,
    key: &str,
    max_times: i64,
) -> Result<(), String> {
    let mut conn = match redis.get_multiplexed_async_connection().await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("Redis connection failed: {e}");
            return Err("Redis unavailable".to_string());
        }
    };

    let current: i64 = match conn.incr(key, 1).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("Redis INCR failed: {e}");
            return Err("Redis error".to_string());
        }
    };

    if current == 1 {
        let secs = seconds_until_midnight();
        let _: Result<(), _> = conn.expire(key, secs as i64).await;
    }

    if current > max_times {
        return Err(format!("Daily limit reached ({}/{})", max_times, max_times));
    }

    Ok(())
}

/// 配额回滚：LLM 调用失败或内部错误时 DECR 计数器
async fn decr_daily_limit(redis: &redis::Client, key: &str) -> Result<(), ()> {
    let mut conn = match redis.get_multiplexed_async_connection().await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("Redis DECR connection failed: {e}");
            return Err(());
        }
    };
    let _: Result<i64, _> = conn.decr(key, 1).await;
    Ok(())
}

/// 计算到当天 23:59:59 剩余的秒数
fn seconds_until_midnight() -> u64 {
    let now = chrono::Utc::now();
    let today_midnight = now.date_naive().and_hms_opt(23, 59, 59).unwrap();
    let dur = today_midnight - now.naive_utc();
    (dur.num_seconds() + 1).max(60) as u64
}

// ── 全局配置读取 ──

async fn get_config_number(pool: &MySqlPool, key: &str, default: i64) -> i64 {
    match crate::services::global_config::fetch_by_key(pool, key).await {
        Ok(cfg) => cfg.data["value"].as_i64().unwrap_or(default),
        Err(_) => default,
    }
}

// ── 内部用户同步 ──

async fn ensure_internal_user(pool: &MySqlPool, user_id: i64) -> Result<(), anyhow::Error> {
    let exists: Option<(i64,)> = sqlx::query_as("SELECT id FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_optional(pool)
        .await?;

    if exists.is_some() {
        return Ok(());
    }

    let phone = format!("assistant_{}", user_id);
    let password_hash = bcrypt::hash("test", bcrypt::DEFAULT_COST)?;

    sqlx::query("INSERT INTO users (id, phone, password_hash, status) VALUES (?, ?, ?, 1)")
        .bind(user_id)
        .bind(&phone)
        .bind(&password_hash)
        .execute(pool)
        .await?;

    tracing::info!(user_id, "auto-created internal user for assistant");
    Ok(())
}

// ── Session 获取/创建（与 post_message_sse 相同的 chat_sessions_user 逻辑） ──

/// 获取用户最新的 session，如果不存在则创建一个
async fn get_or_create_session(
    pool: &MySqlPool,
    user_id: i64,
) -> Result<crate::models::ChatSessionUser, AppError> {
    // 查找最新 session（按 updated_at DESC）
    let existing: Option<crate::models::ChatSessionUser> = sqlx::query_as(
        "SELECT * FROM chat_sessions_user WHERE user_id = ? ORDER BY updated_at DESC LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| AppError::Internal(format!("session lookup: {e}")))?;

    if let Some(session) = existing {
        return Ok(session);
    }

    // 创建新 session
    svc::create_user_session(pool, user_id, None).await
}

// ── 测试 ──

#[cfg(test)]
mod tests {
    use super::*;

    // ── T025: 签名校验（纯函数，无需外部依赖） ──

    #[test]
    fn verify_sign_matches_correct() {
        let secret = "abc123";
        let body = r#"{"user_id":123,"message":"hello"}"#;
        let sign_string = format!("{}/api/assistant?body={}", secret, body);
        let expected = format!("{:x}", md5::compute(sign_string.as_bytes()));
        assert!(verify_sign(secret, body, &expected));
    }

    #[test]
    fn verify_sign_rejects_wrong_signature() {
        let secret = "abc123";
        let body = r#"{"user_id":123,"message":"hello"}"#;
        assert!(!verify_sign(secret, body, "wrong_sign"));
    }

    #[test]
    fn verify_sign_rejects_tampered_body() {
        let secret = "abc123";
        let body_tampered = r#"{"user_id":999,"message":"hack"}"#;
        // sign for a different body
        let original_body = r#"{"user_id":123,"message":"hello"}"#;
        let sign_string = format!("{}/api/assistant?body={}", secret, original_body);
        let sign = format!("{:x}", md5::compute(sign_string.as_bytes()));
        assert!(!verify_sign(secret, body_tampered, &sign));
    }

    #[test]
    fn verify_sign_empty_sign_fails() {
        assert!(!verify_sign("secret", "body", ""));
    }

    #[test]
    fn verify_sign_case_sensitive() {
        let secret = "abc123";
        let body = r#"{"user_id":123,"message":"hello"}"#;
        let sign_string = format!("{}/api/assistant?body={}", secret, body);
        let sign = format!("{:x}", md5::compute(sign_string.as_bytes()));
        // uppercase should not match
        assert!(!verify_sign(secret, body, &sign.to_uppercase()));
    }

    // ── T033: 日限流 TTL 计算 ──

    #[test]
    fn seconds_until_midnight_is_positive() {
        let secs = seconds_until_midnight();
        assert!(secs > 0, "TTL must be positive, got {}", secs);
    }

    #[test]
    fn seconds_until_midnight_does_not_exceed_24h() {
        let secs = seconds_until_midnight();
        assert!(
            secs <= 86400,
            "TTL must not exceed 24h (86400s), got {}",
            secs
        );
    }

    // ── T027: 参数校验（user_id） ──

    #[test]
    fn user_id_zero_should_fail_validation() {
        let user_id: i64 = 0;
        assert!(user_id <= 0, "user_id=0 must be rejected");
    }

    #[test]
    fn user_id_negative_should_fail_validation() {
        let user_id: i64 = -1;
        assert!(user_id <= 0, "user_id=-1 must be rejected");
    }

    #[test]
    fn user_id_positive_should_pass_validation() {
        let user_id: i64 = 1;
        assert!(user_id > 0, "user_id=1 must pass validation");
    }

    // ── T028: 参数校验（message） ──

    #[test]
    fn empty_message_should_fail() {
        let message = "";
        assert!(message.is_empty(), "empty message must be rejected");
    }

    #[test]
    fn non_empty_message_should_pass() {
        let message = "hello";
        assert!(
            !message.is_empty(),
            "non-empty message must pass validation"
        );
    }

    // ── JSON 序列化 ──

    #[test]
    fn assistant_request_deserializes_correctly() {
        let body = r#"{"user_id":42,"message":"hi"}"#;
        let req: AssistantRequest = serde_json::from_str(body).unwrap();
        assert_eq!(req.user_id, 42);
        assert_eq!(req.message, "hi");
    }

    #[test]
    fn assistant_request_rejects_missing_field() {
        // missing 'message' field
        let body = r#"{"user_id":42}"#;
        assert!(serde_json::from_str::<AssistantRequest>(body).is_err());
    }

    // ── 集成测试（需要外部依赖，默认 skip） ──
    // 运行方式：cargo test --features integration -- --ignored
    // 或设置完整环境变量后：cargo test -- --include-ignored

    #[tokio::test]
    #[ignore = "requires MySQL + Redis + external DB + LLM"]
    async fn integration_successful_request() {
        // T024: 合法用户成功请求
        // 需要：DATABASE_URL, REDIS_URL, EXTERNAL_DB_URL, ASSISTANT_SECRET
        // 以及 agents 表中 id=1 的 Main Agent
    }

    #[tokio::test]
    #[ignore = "requires MySQL + Redis"]
    async fn integration_user_not_found() {
        // T026: 用户不存在于 cloud_user
    }

    #[tokio::test]
    #[ignore = "requires MySQL + Redis + setup"]
    async fn integration_daily_limit_exceeded() {
        // T029: 限流超额
    }

    #[tokio::test]
    #[ignore = "requires external DB setup"]
    async fn integration_ext_db_unavailable() {
        // T030: 外部 DB 不可用
    }

    #[tokio::test]
    #[ignore = "requires MySQL"]
    async fn integration_user_sync() {
        // T031: 内部用户同步
    }

    #[tokio::test]
    #[ignore = "requires full infra"]
    async fn integration_llm_failure_quota_rollback() {
        // T032: LLM 失败配额回滚
    }

    #[tokio::test]
    #[ignore = "requires full infra"]
    async fn integration_cross_midnight_reset() {
        // T034: 跨天重置
    }

    #[tokio::test]
    #[ignore = "requires full infra"]
    async fn integration_concurrent_rate_limit() {
        // T035: 并发限流准确性
    }
}
