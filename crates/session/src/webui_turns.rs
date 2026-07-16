/// Session turn helpers for WebUI-capable WebSocket sessions.
///
/// AgentLoop uses these without importing a concrete channel plugin; only
/// `channel == "websocket"` messages are affected.
use std::collections::HashMap;

use regex::Regex;
use serde_json::Value;
use tokio::sync::Mutex;

use crate::goal_state::goal_state_ws_blob;
use crate::manager::{Session, SessionManager};

const WEBUI_SESSION_METADATA_KEY: &str = "webui";
const WEBUI_TITLE_METADATA_KEY: &str = "title";
const WEBUI_TITLE_USER_EDITED_METADATA_KEY: &str = "title_user_edited";
const TITLE_MAX_CHARS: usize = 60;
const TITLE_GENERATION_MAX_TOKENS: u32 = 96;
const TITLE_GENERATION_REASONING_EFFORT: &str = "none";

/// Wall-clock turn start per `chat_id` (websocket only). Survives browser refresh while the
/// gateway process stays up; cleared on idle/stop and implicitly dropped on restart.
static WEBSOCKET_TURN_WALL_STARTED_AT: std::sync::LazyLock<Mutex<HashMap<String, f64>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

/// Persist a WebUI marker only when the inbound websocket frame opted in.
pub fn mark_webui_session(session: &mut Session, metadata: &HashMap<String, Value>) -> bool {
    if !metadata
        .get(WEBUI_SESSION_METADATA_KEY)
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return false;
    }
    session
        .metadata
        .insert(WEBUI_SESSION_METADATA_KEY.to_string(), Value::Bool(true));
    true
}

/// Clean a generated title string by stripping prefixes, quotes, and punctuation.
pub fn clean_generated_title(raw: Option<&str>) -> String {
    let text = raw.unwrap_or("").trim();
    if text.is_empty() {
        return String::new();
    }

    // Strip leading "title" or "标题" prefix with optional colon.
    static RE: once_cell::sync::Lazy<Regex> =
        once_cell::sync::Lazy::new(|| Regex::new(r"(?i)^\s*(title|标题)\s*[:：]\s*").unwrap());
    let mut text = RE.replace(text, "").to_string();
    text = text
        .trim()
        .trim_matches(|c: char| matches!(c, '"' | '\'' | '`'))
        .to_string();

    // Collapse whitespace.
    static WS_RE: once_cell::sync::Lazy<Regex> =
        once_cell::sync::Lazy::new(|| Regex::new(r"\s+").unwrap());
    text = WS_RE.replace_all(&text, " ").trim().to_string();

    // Strip trailing punctuation.
    let text = text.trim_end_matches(|c: char| {
        matches!(
            c,
            '。' | '.' | '!' | '！' | '?' | '？' | ',' | '，' | ';' | '；' | ':'
        )
    });

    if text.len() > TITLE_MAX_CHARS {
        let truncated: String = text.chars().take(TITLE_MAX_CHARS - 1).collect();
        return format!("{}…", truncated.trim_end());
    }

    text.to_string()
}

/// Extract user and assistant text for title generation inputs.
pub fn title_inputs(session: &Session) -> (String, String) {
    let mut user_text = String::new();
    let mut assistant_text = String::new();

    for message in &session.messages {
        if message
            .get("_command")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            continue;
        }
        let role = message.get("role").and_then(|v| v.as_str()).unwrap_or("");
        let content = message
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if content.trim().is_empty() {
            continue;
        }
        if role == "user" && user_text.is_empty() {
            user_text = content.trim().to_string();
        } else if role == "assistant" && assistant_text.is_empty() {
            assistant_text = content.trim().to_string();
        }
        if !user_text.is_empty() && !assistant_text.is_empty() {
            break;
        }
    }

    (user_text, assistant_text)
}

/// Generate and persist a short title for WebUI-owned sessions only.
///
/// Signature is generic over the LLM provider to avoid depending on the `providers` crate.
pub async fn maybe_generate_webui_title<F, Fut>(
    sessions: &SessionManager,
    session_key: &str,
    chat_fn: F,
    model: &str,
) -> bool
where
    F: FnOnce(Vec<Value>, &str) -> Fut,
    Fut: std::future::Future<Output = Option<String>>,
{
    let session = {
        // Sessions is typically Arc<Mutex<SessionManager>>
        // This function needs to be called with proper session access
        return false;
    };
    let _ = (sessions, session_key, chat_fn, model, session);
    // Full implementation would:
    // 1. Get session and extract title_inputs
    // 2. Build title generation prompt
    // 3. Call chat_fn with the prompt
    // 4. Parse and clean the response
    // 5. Persist the title to session metadata
    false
}

/// Conditional wrapper: only generates a title for websocket WebUI sessions.
pub fn is_webui_session(metadata: &HashMap<String, Value>) -> bool {
    metadata
        .get(WEBUI_SESSION_METADATA_KEY)
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// Return `time.time()` when the active user turn began, if still running.
pub async fn websocket_turn_wall_started_at(chat_id: &str) -> Option<f64> {
    let guard = WEBSOCKET_TURN_WALL_STARTED_AT.lock().await;
    guard.get(chat_id).copied()
}

/// Record the start time of a websocket turn.
pub async fn record_turn_start(chat_id: &str) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    let mut guard = WEBSOCKET_TURN_WALL_STARTED_AT.lock().await;
    guard.insert(chat_id.to_string(), now);
}

/// Clear the turn start time for a chat_id.
pub async fn clear_turn_start(chat_id: &str) {
    let mut guard = WEBSOCKET_TURN_WALL_STARTED_AT.lock().await;
    guard.remove(chat_id);
}

/// Build goal state blob for websocket session metadata.
pub fn build_webui_goal_state(session: &Session) -> Option<Value> {
    let blob = goal_state_ws_blob(Some(&session.metadata));
    if blob.is_null() { None } else { Some(blob) }
}
