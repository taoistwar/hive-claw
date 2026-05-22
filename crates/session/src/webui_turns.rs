/// Session turn helpers for WebUI-capable WebSocket sessions.
///
/// AgentLoop uses these without importing a concrete channel plugin; only
/// `channel == "websocket"` messages are affected.

use std::collections::HashMap;
use std::sync::Arc;

use regex::Regex;
use serde_json::Value;
use tokio::sync::Mutex;

use crate::goal_state::goal_state_ws_blob;
use crate::manager::{Session, SessionManager};

type TODO_LLMProvider = dyn std::any::Any;
type TODO_MessageBus = dyn std::any::Any;
type TODO_InboundMessage = dyn std::any::Any;
type TODO_LLMRuntime = dyn std::any::Any;

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
    text = text.trim().trim_matches(|c: char| matches!(c, '"' | '\'' | '`')).to_string();

    // Collapse whitespace.
    static WS_RE: once_cell::sync::Lazy<Regex> =
        once_cell::sync::Lazy::new(|| Regex::new(r"\s+").unwrap());
    text = WS_RE.replace_all(&text, " ").trim().to_string();

    // Strip trailing punctuation.
    let text = text.trim_end_matches(|c: char| {
        matches!(
            c, '。' | '.' | '!' | '！' | '?' | '？' | ',' | '，' | ';' | '；' | ':'
        )
    });

    if text.len() > TITLE_MAX_CHARS {
        let truncated: String = text.chars().take(TITLE_MAX_CHARS - 1).collect();
        return format!("{}…", truncated.trim_end());
    }

    text.to_string()
}

fn _title_inputs(session: &Session) -> (String, String) {
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
/// TODO: This depends on `LLMProvider::chat_with_retry` which requires the
/// `providers` crate. The async implementation is stubbed here.
pub async fn maybe_generate_webui_title(
    _sessions: &SessionManager,
    _session_key: &str,
    _provider: &TODO_LLMProvider,
    _model: &str,
) -> bool {
    // TODO: Implement full title generation flow with provider.chat_with_retry
    false
}

/// Conditional wrapper: only generates a title for websocket WebUI sessions.
pub async fn maybe_generate_webui_title_after_turn(
    channel: &str,
    metadata: &HashMap<String, Value>,
    sessions: &SessionManager,
    session_key: &str,
    provider: &TODO_LLMProvider,
    model: &str,
) -> bool {
    if channel != "websocket"
        || !metadata
            .get(WEBUI_SESSION_METADATA_KEY)
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    {
        return false;
    }
    maybe_generate_webui_title(sessions, session_key, provider, model).await
}

/// Return `time.time()` when the active user turn began, if still running.
pub async fn websocket_turn_wall_started_at(chat_id: &str) -> Option<f64> {
    let guard = WEBSOCKET_TURN_WALL_STARTED_AT.lock().await;
    guard.get(chat_id).copied()
}

/// Notify WebSocket clients while a user turn is executing (timing strip).
///
/// TODO: Depends on `MessageBus`, `InboundMessage`, `OutboundMessage` from
/// the `bus` crate.
pub async fn publish_turn_run_status(
    _bus: &TODO_MessageBus,
    _msg: &TODO_InboundMessage,
    _status: &str,
) {
    // TODO: Implement bus.publish_outbound for websocket channel
}

/// Build the bus progress callback for agent runtime events.
///
/// TODO: Depends on `MessageBus`, `InboundMessage`.
pub fn build_bus_progress_callback(
    _bus: &TODO_MessageBus,
    _msg: &TODO_InboundMessage,
) -> Box<dyn Fn(String, ProgressParams) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> + Send>
{
    Box::new(|_content, _params| {
        Box::pin(async move {
            // TODO: Implement bus.publish_outbound for progress events
        })
    })
}

/// Parameters for the progress callback.
pub struct ProgressParams {
    pub tool_hint: bool,
    pub tool_events: Option<Vec<HashMap<String, Value>>>,
    pub file_edit_events: Option<Vec<HashMap<String, Value>>>,
    pub reasoning: bool,
    pub reasoning_end: bool,
}

/// Own the WebUI/WebSocket wire details that hang off AgentLoop turns.
///
/// TODO: Depends on `MessageBus`, `SessionManager`, `InboundMessage`, `LLMRuntime`.
pub struct WebuiTurnCoordinator {
    pub bus: Arc<TODO_MessageBus>,
    pub sessions: Arc<Mutex<SessionManager>>,
    pub schedule_background: Arc<dyn Fn(std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>) + Send + Sync>,
    title_contexts: Mutex<HashMap<String, Arc<TODO_LLMRuntime>>>,
}

impl WebuiTurnCoordinator {
    pub fn new(
        bus: Arc<TODO_MessageBus>,
        sessions: Arc<Mutex<SessionManager>>,
        schedule_background: Arc<dyn Fn(std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>) + Send + Sync>,
    ) -> Self {
        Self {
            bus,
            sessions,
            schedule_background,
            title_contexts: Mutex::new(HashMap::new()),
        }
    }

    pub async fn capture_title_context(
        &self,
        session_key: &str,
        msg: &TODO_InboundMessage,
        llm: Arc<TODO_LLMRuntime>,
    ) {
        // TODO: Check msg.channel == "websocket" && msg.metadata["webui"] == true
        let _ = (session_key, msg, llm);
    }

    pub async fn discard(&self, session_key: &str) {
        self.title_contexts.lock().await.remove(session_key);
    }

    pub async fn publish_run_status(&self, _msg: &TODO_InboundMessage, _status: &str) {
        // TODO: Implement
    }

    pub async fn handle_turn_end(
        &self,
        _msg: &TODO_InboundMessage,
        _session_key: &str,
        _latency_ms: Option<i64>,
    ) {
        // TODO: Implement turn end handler with goal_state_ws_blob and title scheduling
    }

    fn _schedule_title_update(&self, _msg: &TODO_InboundMessage, _session_key: &str) {
        // TODO: Schedule background title generation
    }
}
