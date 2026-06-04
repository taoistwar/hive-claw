//! Messages API — 获取用户历史消息列表（MD5 签名鉴权）
//!
//! POST /api/messages?sign={md5}
//!
//! 请求 Body (JSON)：
//!   user_id  - 用户 ID，必须 > 0
//!   date     - 最后一条记录时间（YYYY-MM-DD HH:MM:SS），返回该时间之前的最近 10 条
//!
//! URL 查询参数：
//!   sign     - MD5 签名：MD5(ASSISTANT_SECRET + canonical_json_body)
//!
//! 处理流程：
//!   1. user_id > 0 校验
//!   2. date 格式校验（YYYY-MM-DD HH:MM:SS）
//!   3. MD5 签名校验（对完整 JSON body 计算 MD5）
//!   4. 查询 chat_messages_user 表，返回最近 10 条

use axum::{
    Router,
    extract::{Query, State},
    response::{IntoResponse, Response},
    routing::post,
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

use crate::api::chat_common;
use crate::api::AppState;
use crate::models::ChatMessageUser;
use crate::utils::error::AppError;

/// 预共享密钥，从环境变量 ASSISTANT_SECRET 懒加载
static SECRET: OnceLock<String> = OnceLock::new();

fn get_secret() -> &'static str {
    SECRET
        .get_or_init(|| std::env::var("ASSISTANT_SECRET").unwrap_or_default())
        .as_str()
}

pub fn router() -> Router<AppState> {
    Router::new().route("/messages", post(list_messages))
}

// ── 请求 / 响应 ──

#[derive(Debug, Deserialize)]
pub struct MessagesBody {
    pub user_id: i64,
    /// 最后一条聊天记录的时间（YYYY-MM-DD HH:MM:SS），返回该时间往前的最近 10 条
    pub date: String,
}

#[derive(Debug, Deserialize)]
pub struct SignQuery {
    /// MD5 签名（对 JSON body 的 canonical 序列化结果计算）
    pub sign: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct MessagesResponse {
    pub messages: Vec<ChatMessageUser>,
}

// ── Handler ──

async fn list_messages(
    State(state): State<AppState>,
    Query(sign_query): Query<SignQuery>,
    body: String,
) -> Response {
    // 3. MD5 签名校验（对原始 JSON body 字符串计算，与客户端 JSON.stringify 一致）
    let secret = get_secret();
    if !secret.is_empty() {
        let sign = sign_query.sign.as_deref().unwrap_or("");
        if !chat_common::verify_sign(secret, "/api/messages", body.trim(), sign) {
            return AppError::BadRequest("Invalid signature".into())
                .into_response::<()>()
                .into_response();
        }
    }

    let params: MessagesBody = match serde_json::from_str(&body) {
        Ok(p) => p,
        Err(_) => {
            return AppError::BadRequest("Invalid request body".into())
                .into_response::<()>()
                .into_response();
        }
    };

    // 1. user_id > 0 校验
    if params.user_id <= 0 {
        return AppError::BadRequest("user_id must be positive".into())
            .into_response::<()>()
            .into_response();
    }

    // 2. date 格式校验（YYYY-MM-DD HH:MM:SS）
    let date_str = params.date.trim();
    if date_str.is_empty() {
        return AppError::BadRequest("date is required (format: YYYY-MM-DD HH:MM:SS)".into())
            .into_response::<()>()
            .into_response();
    }

    let cutoff = match chrono::NaiveDateTime::parse_from_str(date_str, "%Y-%m-%d %H:%M:%S") {
        Ok(dt) => dt,
        Err(_) => {
            return AppError::BadRequest(
                "date must be in YYYY-MM-DD HH:MM:SS format".into(),
            )
            .into_response::<()>()
            .into_response();
        }
    };

    // 4. 查询消息：user_id 匹配，且 created_at 在截止日期之前，取最近 10 条
    let messages: Vec<ChatMessageUser> = match sqlx::query_as(
        "SELECT id, session_id, user_id, role, content, elapsed_ms, created_at \
         FROM chat_messages_user \
         WHERE user_id = ? AND created_at < ? \
         ORDER BY created_at DESC \
         LIMIT 10",
    )
    .bind(params.user_id)
    .bind(cutoff)
    .fetch_all(&state.pool)
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!(error = %e, user_id = params.user_id, "failed to query messages");
            return AppError::Internal("Failed to query messages".into())
                .into_response::<()>()
                .into_response();
        }
    };

    axum::Json(MessagesResponse { messages }).into_response()
}

// ── 测试 ──

#[cfg(test)]
mod tests {
    use crate::api::chat_common;

    #[test]
    fn verify_sign_matches_correct() {
        let secret = "abc123";
        let body = r#"{"user_id":42,"date":"2026-06-03 15:27:31"}"#;
        let sign_str = format!("{}/api/messages?body={}", secret, body);
        let expected = format!("{:x}", md5::compute(sign_str.as_bytes()));
        assert!(chat_common::verify_sign(secret, "/api/messages", body, &expected));
    }

    #[test]
    fn verify_sign_rejects_wrong_signature() {
        let body = r#"{"user_id":42,"date":"2026-06-03 15:27:31"}"#;
        assert!(!chat_common::verify_sign("abc123", "/api/messages", body, "wrong"));
    }

    #[test]
    fn verify_sign_rejects_tampered_params() {
        let secret = "abc123";
        let body = r#"{"user_id":42,"date":"2026-06-03 15:27:31"}"#;
        let sign_str = format!("{}/api/messages?body={}", secret, body);
        let sign = format!("{:x}", md5::compute(sign_str.as_bytes()));
        assert!(!chat_common::verify_sign(secret, "/api/messages", r#"{"user_id":99,"date":"2026-06-03 15:27:31"}"#, &sign));
    }

    #[test]
    fn verify_sign_empty_sign_fails() {
        assert!(!chat_common::verify_sign("secret", "/api/messages", "{}", ""));
    }

    #[test]
    fn messages_response_serializes_correctly() {
        use super::MessagesResponse;
        use serde_json;
        let resp = MessagesResponse { messages: vec![] };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"messages\""));
        assert!(json.contains("[]"));
    }
}
