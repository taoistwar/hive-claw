//! OpenAI-compatible JSON payload types.
//!
//! Mirrors what `nanobot.api.server` returns without committing to a
//! full OpenAI schema port — the server only implements the subset it
//! actually uses.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Incoming `POST /v1/chat/completions` body (JSON path).
#[derive(Debug, Clone, Deserialize)]
pub struct ChatCompletionRequest {
    #[serde(default)]
    pub model: Option<String>,
    pub messages: Vec<ChatMessage>,
    #[serde(default)]
    pub stream: bool,
    /// Extension: non-standard field used by the Python server.
    #[serde(default)]
    pub session_id: Option<String>,
}

/// One message in a chat completion request.
///
/// Content is kept as `serde_json::Value` so callers can submit either a
/// plain string or an OpenAI-style content-block array (text + image_url).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChatMessage {
    pub role: String,
    #[serde(default)]
    pub content: Value,
}

/// `/v1/chat/completions` non-streaming response.
#[derive(Debug, Clone, Serialize)]
pub struct ChatCompletionResponse {
    pub id: String,
    pub object: &'static str,
    pub created: i64,
    pub model: String,
    pub choices: Vec<ChatCompletionChoice>,
    pub usage: Usage,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatCompletionChoice {
    pub index: u32,
    pub message: ChatMessage,
    pub finish_reason: &'static str,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

/// `/v1/models` list payload.
#[derive(Debug, Clone, Serialize)]
pub struct ModelsList {
    pub object: &'static str,
    pub data: Vec<ModelInfo>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelInfo {
    pub id: String,
    pub object: &'static str,
    pub created: i64,
    pub owned_by: &'static str,
}

/// Build a Chat Completions response shell for a plain assistant text
/// reply. Used by both the non-streaming handler and tests.
pub fn assistant_completion(id: String, model: String, content: String) -> ChatCompletionResponse {
    ChatCompletionResponse {
        id,
        object: "chat.completion",
        created: chrono::Utc::now().timestamp(),
        model,
        choices: vec![ChatCompletionChoice {
            index: 0,
            message: ChatMessage {
                role: "assistant".into(),
                content: Value::String(content),
            },
            finish_reason: "stop",
        }],
        usage: Usage::default(),
    }
}
