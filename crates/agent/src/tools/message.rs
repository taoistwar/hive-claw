//! Message tool for sending messages to users. Port of
//! `nanobot.agent.tools.message`.

use std::sync::Arc;
use std::sync::Mutex;

use async_trait::async_trait;
use serde_json::{json, Map, Value};

use bus::{MessageBus, OutboundMessage};
use utils::helpers::strip_think;

use super::base::{Tool, ToolExecError};

/// Runtime-mutable channel context (set by the loop each turn).
#[derive(Clone, Default, Debug)]
pub struct MessageContext {
    pub channel: String,
    pub chat_id: String,
    pub message_id: Option<String>,
    pub sent_in_turn: bool,
}

/// `message` tool — forwards content to the caller's [`MessageBus`].
pub struct MessageTool {
    bus: Arc<MessageBus>,
    context: Arc<Mutex<MessageContext>>,
}

impl MessageTool {
    pub fn new(bus: Arc<MessageBus>) -> Self {
        Self {
            bus,
            context: Arc::new(Mutex::new(MessageContext::default())),
        }
    }

    pub fn set_context(&self, channel: &str, chat_id: &str, message_id: Option<String>) {
        let mut ctx = self.context.lock().unwrap();
        ctx.channel = channel.to_string();
        ctx.chat_id = chat_id.to_string();
        ctx.message_id = message_id;
    }

    pub fn start_turn(&self) {
        self.context.lock().unwrap().sent_in_turn = false;
    }

    pub fn sent_in_turn(&self) -> bool {
        self.context.lock().unwrap().sent_in_turn
    }
}

#[async_trait]
impl Tool for MessageTool {
    fn name(&self) -> &str {
        "message"
    }
    fn description(&self) -> &str {
        "Send a message to the user, optionally with file attachments. This is the ONLY way to deliver files (images, documents, audio, video) to the user. Use the 'media' parameter with file paths to attach files. Do NOT use read_file to send files — that only reads content for your own analysis."
    }
    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "content":{"type":"string","description":"The message content to send"},
                "channel":{"type":"string","description":"Optional: target channel (telegram, discord, etc.)"},
                "chat_id":{"type":"string","description":"Optional: target chat/user ID"},
                "media":{"type":"array","items":{"type":"string"},"description":"Optional: list of file paths to attach"},
            },
            "required":["content"],
        })
    }
    async fn execute(&self, params: Value) -> Result<Value, ToolExecError> {
        let content_raw = params
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let content = strip_think(content_raw);

        let (default_channel, default_chat_id, default_message_id) = {
            let ctx = self.context.lock().unwrap();
            (
                ctx.channel.clone(),
                ctx.chat_id.clone(),
                ctx.message_id.clone(),
            )
        };

        let channel = params
            .get("channel")
            .and_then(|v| v.as_str())
            .map(String::from)
            .filter(|s| !s.is_empty())
            .unwrap_or(default_channel.clone());
        let chat_id = params
            .get("chat_id")
            .and_then(|v| v.as_str())
            .map(String::from)
            .filter(|s| !s.is_empty())
            .unwrap_or(default_chat_id.clone());
        let explicit_message_id = params
            .get("message_id")
            .and_then(|v| v.as_str())
            .map(String::from);

        let message_id = if channel == default_channel && chat_id == default_chat_id {
            explicit_message_id.or(default_message_id)
        } else {
            None
        };

        if channel.is_empty() || chat_id.is_empty() {
            return Ok(Value::String(
                "Error: No target channel/chat specified".into(),
            ));
        }
        let media: Vec<String> = params
            .get("media")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let mut metadata: Map<String, Value> = Map::new();
        if let Some(mid) = &message_id {
            metadata.insert("message_id".into(), Value::String(mid.clone()));
        }

        let msg = OutboundMessage {
            channel: channel.clone(),
            chat_id: chat_id.clone(),
            content,
            reply_to: None,
            media: media.clone(),
            metadata: metadata.into_iter().collect(),
        };
        self.bus.publish_outbound(msg).await;
        if channel == default_channel && chat_id == default_chat_id {
            self.context.lock().unwrap().sent_in_turn = true;
        }
        let media_info = if media.is_empty() {
            String::new()
        } else {
            format!(" with {} attachments", media.len())
        };
        Ok(Value::String(format!(
            "Message sent to {channel}:{chat_id}{media_info}"
        )))
    }
}
