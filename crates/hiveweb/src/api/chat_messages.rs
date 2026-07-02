//! Messages API — 获取用户历史消息列表（MD5 签名鉴权）
//!
//! POST /api/messages?sign={md5}
//!
//! 请求 Body (JSON)：
//!   user_id     - 用户 ID，必须 > 0
//!   date        - 最后一条记录时间（YYYY-MM-DD HH:MM:SS），返回该时间之前的最近 10 条
//!   channel     - 渠道标识（可选，用于过滤已下架游戏）
//!   client_type - 客户端平台（可选，用于过滤已下架游戏）
//!
//! URL 查询参数：
//!   sign     - MD5 签名：MD5(ASSISTANT_SECRET + canonical_json_body)
//!
//! 处理流程：
//!   1. user_id > 0 校验
//!   2. date 格式校验（YYYY-MM-DD HH:MM:SS）
//!   3. MD5 签名校验（对完整 JSON body 计算 MD5）
//!   4. 查询 chat_messages_user 表，返回最近 10 条
//!   5. 过滤 extensions 中已下架的游戏卡片

use axum::{
    Json, Router,
    extract::{Query, State},
    response::{IntoResponse, Response},
    routing::post,
};
use serde::{Deserialize, Serialize};

use crate::api::AppState;
use crate::api::chat_common;
use crate::models::ChatMessageUser;
use crate::services::chat_user as svc;
use crate::utils::error::AppError;

pub fn router() -> Router<AppState> {
    Router::new().route("/messages", post(list_messages))
}

// ── 请求 / 响应 ──

#[derive(Debug, Deserialize)]
pub struct MessagesBody {
    pub user_id: i64,
    /// 最后一条聊天记录的时间（YYYY-MM-DD HH:MM:SS），返回该时间往前的最近 10 条
    pub date: String,
    /// 渠道标识（如 `app`、`web`、`api`）
    #[serde(default)]
    pub channel: Option<String>,
    /// 客户端平台（`android`、`iphone`、`ipad`、`web`）
    #[serde(default)]
    pub client_type: Option<String>,
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
    let secret = chat_common::get_assistant_secret();
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

    // 2. date 格式校验 + 东8区 → UTC 转换
    let date_str = params.date.trim();
    if date_str.is_empty() {
        return AppError::BadRequest("date is required (format: YYYY-MM-DD HH:MM:SS)".into())
            .into_response::<()>()
            .into_response();
    }

    let naive = match chrono::NaiveDateTime::parse_from_str(date_str, "%Y-%m-%d %H:%M:%S") {
        Ok(dt) => dt,
        Err(_) => {
            return AppError::BadRequest("date must be in YYYY-MM-DD HH:MM:SS format".into())
                .into_response::<()>()
                .into_response();
        }
    };
    // 强制解释为东8区，转为 UTC 后与 DB 比较
    let offset = chrono::FixedOffset::east_opt(8 * 3600).expect("UTC+8 offset");
    let cutoff = naive
        .and_local_timezone(offset)
        .single()
        .map(|dt| dt.naive_utc())
        .unwrap_or(naive);

    // 4. 查询消息：user_id 匹配，且 created_at 在截止日期之前，取最近 10 条
    let mut messages: Vec<ChatMessageUser> = match svc::list_messages_before(&state.pool, params.user_id, cutoff).await {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!(error = %e, user_id = params.user_id, "failed to query messages");
            return AppError::Internal("Failed to query messages".into())
                .into_response::<()>()
                .into_response();
        }
    };

    // 5. 过滤 extensions 中已下架的游戏卡片
    if let Some(ref ext_pool) = state.ext_pool {
        filter_unavailable_games(ext_pool, &mut messages).await;
    }

    axum::Json(MessagesResponse { messages }).into_response()
}

// ── Game card filtering ──

/// 过滤消息中已下架的游戏卡片（cc_logic_game.status != 1）
async fn filter_unavailable_games(
    ext_pool: &sqlx::MySqlPool,
    messages: &mut [ChatMessageUser],
) {
    use crate::services::game_service;
    use std::collections::HashSet;

    let mut game_ids: Vec<i64> = Vec::new();
    for msg in messages.iter() {
        collect_game_ids(&msg.extensions, &mut game_ids);
    }

    if game_ids.is_empty() {
        return;
    }

    let available = match game_service::filter_available_games(ext_pool, &game_ids).await {
        Ok(ids) => ids,
        Err(e) => {
            tracing::warn!(error = %e, "filter_available_games failed, keeping all game cards");
            return;
        }
    };

    for msg in messages.iter_mut() {
        if let Some(ref mut exts) = msg.extensions {
            if let Some(arr) = exts.as_array_mut() {
                arr.retain(|ext| retain_game_card(ext, &available));
                if arr.is_empty() {
                    *exts = serde_json::Value::Null;
                }
            }
        }
    }
}

fn collect_game_ids(extensions: &Option<serde_json::Value>, ids: &mut Vec<i64>) {
    let arr = match extensions.as_ref().and_then(|v| v.as_array()) {
        Some(a) => a,
        None => return,
    };
    for ext in arr {
        if let Some(id) = extract_game_id(ext) {
            ids.push(id);
        }
    }
}

fn retain_game_card(ext: &serde_json::Value, available: &std::collections::HashSet<i64>) -> bool {
    let ct = ext.get("content_type").and_then(|v| v.as_str()).unwrap_or("");
    if ct != "card" {
        return true;
    }
    let payload = ext.get("payload");
    let ptype = payload.and_then(|p| p.get("type")).and_then(|v| v.as_str()).unwrap_or("");
    if ptype != "game" {
        return true;
    }
    match extract_game_id(ext) {
        Some(id) => available.contains(&id),
        None => true,
    }
}

fn extract_game_id(ext: &serde_json::Value) -> Option<i64> {
    let info = ext
        .get("payload")
        .and_then(|p| p.get("info"))?;
    let id_val = info.get("id")?;
    if let Some(s) = id_val.as_str() {
        s.parse::<i64>().ok()
    } else {
        id_val.as_i64()
    }
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
        assert!(chat_common::verify_sign(
            secret,
            "/api/messages",
            body,
            &expected
        ));
    }

    #[test]
    fn verify_sign_rejects_wrong_signature() {
        let body = r#"{"user_id":42,"date":"2026-06-03 15:27:31"}"#;
        assert!(!chat_common::verify_sign(
            "abc123",
            "/api/messages",
            body,
            "wrong"
        ));
    }

    #[test]
    fn verify_sign_rejects_tampered_params() {
        let secret = "abc123";
        let body = r#"{"user_id":42,"date":"2026-06-03 15:27:31"}"#;
        let sign_str = format!("{}/api/messages?body={}", secret, body);
        let sign = format!("{:x}", md5::compute(sign_str.as_bytes()));
        assert!(!chat_common::verify_sign(
            secret,
            "/api/messages",
            r#"{"user_id":99,"date":"2026-06-03 15:27:31"}"#,
            &sign
        ));
    }

    #[test]
    fn verify_sign_empty_sign_fails() {
        assert!(!chat_common::verify_sign(
            "secret",
            "/api/messages",
            "{}",
            ""
        ));
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
