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

    let params: MessagesBody = match serde_json::from_str::<MessagesBody>(&body) {
        Ok(p) => {
            tracing::debug!(
                user_id = p.user_id,
                date = %p.date,
                channel = ?p.channel,
                client_type = ?p.client_type,
                "list_messages: request parsed"
            );
            p
        }
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
        Ok(rows) => {
            tracing::debug!(count = rows.len(), user_id = params.user_id, "list_messages: queried");
            rows
        }
        Err(e) => {
            tracing::error!(error = %e, user_id = params.user_id, "failed to query messages");
            return AppError::Internal("Failed to query messages".into())
                .into_response::<()>()
                .into_response();
        }
    };

    // 5. 过滤 extensions 中已下架的游戏卡片
    if let Some(ref ext_pool) = state.ext_pool {
        let before = count_game_cards(&messages);
        filter_unavailable_games(ext_pool, &mut messages).await;
        let after = count_game_cards(&messages);
        tracing::debug!(
            game_cards_before = before,
            game_cards_after = after,
            "list_messages: filtered unavailable games"
        );

        // 6. 刷新游戏卡片：如果 client_type/channel 与用户传入的不一致，重新查询
        if let (Some(ref ct), Some(ref ch)) = (params.client_type.as_deref(), params.channel.as_deref()) {
            refresh_game_cards(ext_pool, ct, ch, &mut messages).await;
        }
    }

    tracing::debug!(
        user_id = params.user_id,
        client_type = ?params.client_type,
        channel = ?params.channel,
        message_count = messages.len(),
        "list_messages: returning"
    );

    axum::Json(MessagesResponse { messages }).into_response()
}

/// Count the number of game cards across all messages' extensions.
fn count_game_cards(messages: &[ChatMessageUser]) -> usize {
    let mut count = 0;
    for msg in messages {
        if let Some(ref exts) = msg.extensions {
            if let Some(arr) = exts.as_array() {
                for ext in arr {
                    let ct = ext.get("content_type").and_then(|v| v.as_str()).unwrap_or("");
                    let pt = ext.get("payload").and_then(|p| p.get("type")).and_then(|v| v.as_str()).unwrap_or("");
                    if ct == "card" && pt == "game" {
                        count += 1;
                    }
                }
            }
        }
    }
    count
}

// ── Game card filtering ──

/// 过滤消息中已下架的游戏卡片（cc_logic_game.status != 1）
async fn filter_unavailable_games(
    ext_pool: &sqlx::MySqlPool,
    messages: &mut [ChatMessageUser],
) {
    use crate::services::game_service;

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
                let before = arr.len();
                arr.retain(|ext| retain_game_card(ext, &available));
                // 有游戏卡片被过滤且 content 为空时，提示已下架
                if arr.len() < before && msg.content.as_deref().map_or(true, |c| c.trim().is_empty()) {
                    msg.content = Some("游戏已经下架".into());
                }
                if arr.is_empty() {
                    *exts = serde_json::Value::Null;
                }
            }
        }
    }
}

fn collect_game_ids(extensions: &Option<serde_json::Value>, ids: &mut Vec<i64>) {
    let extensions = match extensions.as_ref().and_then(|v| v.as_array()) {
        Some(a) => a,
        None => return,
    };
    for ext in extensions {
        let ct = ext.get("content_type").and_then(|v| v.as_str()).unwrap_or("");
        if ct != "card" {
            continue;
        }
        let pt = ext.get("payload").and_then(|p| p.get("type")).and_then(|v| v.as_str()).unwrap_or("");
        if pt != "game" {
            continue;
        }
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

/// 刷新游戏卡片：如果 card 中的 client_type/channel 与用户传入的不一致，
/// 根据游戏 ID 重新查询匹配用户 client_type/channel 的游戏信息并替换。
async fn refresh_game_cards(
    ext_pool: &sqlx::MySqlPool,
    client_type: &str,
    channel: &str,
    messages: &mut [ChatMessageUser],
) {
    use crate::services::game_service;
    use serde_json::Value;

    for msg in messages.iter_mut() {
        println!("msg: {:?}", msg);
        let exts = match msg.extensions.as_mut() {
            Some(Value::Array(arr)) => arr,
            _ => continue,
        };
        let mut is_game = false; // 当前消息是否包含游戏卡片
        let mut same = false; // 当前消息中是否有游戏卡片的 client_type/channel 与用户传入的一致
        let mut exits = false; // 根据用户传入的 client_type/channel 查询到的游戏信息是否存在
        for ext in exts.iter_mut() {
            let ct = ext.get("content_type").and_then(|v| v.as_str()).unwrap_or("");
            println!("ext: {:?}, content_type: {}", ext, ct);
            if ct != "card" {
                continue;
            }
            let payload = ext.get("payload");
            let pt = payload.and_then(|p| p.get("type")).and_then(|v| v.as_str()).unwrap_or("");
            println!("payload: {:?}, type: {}", payload, pt);
            if pt != "game" {
                continue;
            }
            is_game = true;
            let info = match payload.and_then(|p| p.get("info")) {
                Some(i) => i,
                None => continue,
            };
            println!("info: {:?}", info);

            let card_ct = info.get("client_type").and_then(|v| v.as_str()).unwrap_or("");
            let card_ch = info.get("channel").and_then(|v| v.as_str()).unwrap_or("");
            println!("card client_type: {}, channel: {}, client_type:{}, channel:{}", card_ct, card_ch, client_type, channel);
            // 如果一致则跳过
            if card_ct.eq_ignore_ascii_case(client_type) && card_ch.eq_ignore_ascii_case(channel) {
                same = true;
                continue;
            }
            same = false;
            let game_id = match info.get("id").and_then(|v| v.as_str()).and_then(|s| s.parse::<i64>().ok()) {
                Some(id) => id,
                None => continue,
            };
            println!("game_id: {}", game_id);
            // 重新查询游戏信息
            let fresh = match game_service::get_external_game_by_id(
                ext_pool, game_id, client_type, channel,
            )
            .await
            {
                Ok(mut games) => {
                    game_service::sort_external_games_by_priority(ext_pool, &mut games).await;
                    games.into_iter().next()
                }
                Err(_) => None,
            };
            println!("fresh: {:?}", fresh);
            if let Some(info_row) = fresh {
                if info_row.logic_game_id != 0 {
                    let obj = ext.as_object_mut().unwrap();
                    obj.insert("payload".into(), Value::Object({
                        let mut p = serde_json::Map::new();
                        p.insert("type".into(), Value::String("game".into()));
                        p.insert("info".into(), serde_json::json!({
                            "id": info_row.logic_game_id.to_string(),
                            "name": info_row.name,
                            "channel": info_row.channel,
                            "client_type": info_row.client_type,
                            "reason": info_row.description,
                            "game_tags": info_row.game_tags,
                            "cover_image": info_row.cover_image,
                            "computer_id": info_row.computer_id,
                            "platform_name": info_row.platform_name,
                            "game_icon": info_row.game_icon,
                        }));
                        p
                    }));
                    exits = true;
                }
            }
        }
        if is_game && !same && !exits {
            msg.content = Some("该游戏当前客户端不支持".into());
            msg.extensions = None;
        }
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
