//! Event types for the message bus (Rust port of `nanobot.bus.events`).

use std::collections::HashMap;

use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};

/// Message received from a chat channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboundMessage {
    /// e.g. `telegram`, `discord`, `slack`, `whatsapp`.
    pub channel: String,
    /// User identifier.
    pub sender_id: String,
    /// Chat / channel identifier.
    pub chat_id: String,
    /// Message text.
    pub content: String,
    #[serde(default = "now_local")]
    pub timestamp: DateTime<Local>,
    /// Media URLs.
    #[serde(default)]
    pub media: Vec<String>,
    /// Channel-specific metadata.
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
    /// Optional override for thread-scoped sessions.
    #[serde(default)]
    pub session_key_override: Option<String>,
}

fn now_local() -> DateTime<Local> {
    Local::now()
}

impl Default for InboundMessage {
    fn default() -> Self {
        Self {
            channel: String::new(),
            sender_id: String::new(),
            chat_id: String::new(),
            content: String::new(),
            timestamp: now_local(),
            media: Vec::new(),
            metadata: HashMap::new(),
            session_key_override: None,
        }
    }
}

impl InboundMessage {
    /// Unique key for session identification (`{channel}:{chat_id}` unless
    /// overridden).
    pub fn session_key(&self) -> String {
        if let Some(k) = self
            .session_key_override
            .as_ref()
            .filter(|key| !key.is_empty())
        {
            return k.clone();
        }
        format!("{}:{}", self.channel, self.chat_id)
    }
}

/// Message to send to a chat channel.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OutboundMessage {
    pub channel: String,
    pub chat_id: String,
    pub content: String,
    #[serde(default)]
    pub reply_to: Option<String>,
    #[serde(default)]
    pub media: Vec<String>,
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}
