//! Message tool for sending messages to users. Port of
//! `nanobot.agent.tools.message`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;

use async_trait::async_trait;
use serde_json::{Map, Value, json};

use bus::{MessageBus, OutboundMessage};
use utils::helpers::strip_think;

use super::base::{Tool, ToolExecError};
use super::context::RequestContext;
use super::path_utils::resolve_workspace_path;

/// Runtime-mutable channel context (set by the loop each turn).
#[derive(Clone, Default, Debug)]
pub struct MessageContext {
    pub channel: String,
    pub chat_id: String,
    pub message_id: Option<String>,
    pub sent_in_turn: bool,
    pub metadata: std::collections::HashMap<String, Value>,
    pub record_channel_delivery: bool,
    pub turn_delivered_media: Vec<String>,
}

/// `message` tool — forwards content to the caller's [`MessageBus`].
pub struct MessageTool {
    bus: Arc<MessageBus>,
    context: Arc<Mutex<MessageContext>>,
    workspace: PathBuf,
    restrict_to_workspace: bool,
}

impl MessageTool {
    pub fn new(bus: Arc<MessageBus>, workspace: PathBuf, restrict_to_workspace: bool) -> Self {
        Self {
            bus,
            context: Arc::new(Mutex::new(MessageContext::default())),
            workspace,
            restrict_to_workspace,
        }
    }

    pub fn set_context(&self, ctx: &RequestContext) {
        let mut guard = self.context.lock().unwrap();
        guard.channel = ctx.channel.clone();
        guard.chat_id = ctx.chat_id.clone();
        guard.message_id = ctx.message_id.clone();
        guard.metadata = ctx.metadata.clone();
    }

    pub fn start_turn(&self) {
        let mut ctx = self.context.lock().unwrap();
        ctx.sent_in_turn = false;
        ctx.turn_delivered_media.clear();
    }

    pub fn sent_in_turn(&self) -> bool {
        self.context.lock().unwrap().sent_in_turn
    }

    pub fn turn_delivered_media_paths(&self) -> Vec<String> {
        self.context.lock().unwrap().turn_delivered_media.clone()
    }

    fn resolve_media(&self, media: &[String]) -> Result<Vec<String>, String> {
        let mut resolved: Vec<String> = Vec::new();
        let allowed_dir = if self.restrict_to_workspace {
            Some(self.workspace.clone())
        } else {
            None
        };
        for p in media {
            if p.starts_with("http://") || p.starts_with("https://") {
                resolved.push(p.clone());
            } else if !self.restrict_to_workspace {
                let path = Path::new(p);
                if path.is_absolute() {
                    resolved.push(p.clone());
                } else {
                    resolved.push((self.workspace.join(path)).to_string_lossy().to_string());
                }
            } else {
                match resolve_workspace_path(p, Some(&self.workspace), allowed_dir.as_deref(), None)
                {
                    Ok(r) => resolved.push(r.to_string_lossy().to_string()),
                    Err(e) => {
                        return Err(format!("media path is not allowed: {e}"));
                    }
                }
            }
        }
        Ok(resolved)
    }
}

#[async_trait]
impl Tool for MessageTool {
    fn name(&self) -> &str {
        "message"
    }
    fn description(&self) -> String {
        "Proactively send a message to a user/channel, optionally with file attachments. Use this for reminders, cross-channel delivery, or explicit proactive sends. Do not use this for the normal reply in the current chat: answer naturally instead. If channel/chat_id would target the current runtime conversation, do not call this tool unless the user explicitly asked you to proactively send an existing file attachment. When generate_image creates images in the current chat, use the message tool with the artifact paths in the media parameter to deliver the images to the user. For proactive attachment delivery, use the 'media' parameter with file paths. Do NOT use read_file to send files — that only reads content for your own analysis.".into()
    }
    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "content":{"type":"string","description":"Message content for proactive or cross-channel delivery. Do not use this for a normal reply in the current chat."},
                "channel":{"type":"string","description":"Optional target channel for cross-channel/proactive delivery. Do not set this to the current runtime channel for a normal reply."},
                "chat_id":{"type":"string","description":"Optional target chat/user ID for cross-channel/proactive delivery. On WebSocket/WebUI turns: omit chat_id to use the server's conversation id (never pass client_id values like anon-…). Do not set this to the current runtime chat for a normal reply."},
                "media":{"type":"array","items":{"type":"string"},"description":"Optional list of existing file paths to attach. Use artifact paths returned by generate_image here when delivering generated images."},
                "buttons":{"type":"array","items":{"type":"array","items":{"type":"string"}},"description":"Optional: inline keyboard rows with button labels"},
            },
            "required":["content"],
        })
    }
    fn set_tool_context(&self, ctx: &RequestContext) {
        self.set_context(ctx);
    }
    async fn execute(&self, params: Value) -> Result<Value, ToolExecError> {
        let content_raw = params
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let content = strip_think(content_raw);

        let media: Vec<String> = params
            .get("media")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let buttons: Vec<Vec<String>> = params
            .get("buttons")
            .and_then(|v| v.as_array())
            .map(|rows| {
                rows.iter()
                    .filter_map(|row| {
                        row.as_array().map(|cells| {
                            cells
                                .iter()
                                .filter_map(|c| c.as_str().map(String::from))
                                .collect()
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        let (default_channel, default_chat_id, default_message_id, default_metadata) = {
            let ctx = self.context.lock().unwrap();
            (
                ctx.channel.clone(),
                ctx.chat_id.clone(),
                ctx.message_id.clone(),
                ctx.metadata.clone(),
            )
        };

        let channel = params
            .get("channel")
            .and_then(|v| v.as_str())
            .map(String::from)
            .filter(|s| !s.is_empty())
            .unwrap_or(default_channel.clone());
        let explicit_chat_id = params
            .get("chat_id")
            .and_then(|v| v.as_str())
            .map(String::from)
            .filter(|s| !s.is_empty());

        if default_channel == "websocket"
            && channel == "websocket"
            && explicit_chat_id.is_some()
            && explicit_chat_id.as_ref().map(|s| s.trim()) != default_chat_id.trim().into()
        {
            return Ok(Value::String(
                "Error: chat_id does not match the active WebSocket conversation. \
                Omit chat_id (and usually channel) so delivery uses the current \
                conversation id from context — WebSocket client_id strings \
                (e.g. anon-…) are not chat ids."
                    .into(),
            ));
        }

        let chat_id = explicit_chat_id.unwrap_or(default_chat_id.clone());
        let explicit_message_id = params
            .get("message_id")
            .and_then(|v| v.as_str())
            .map(String::from);

        let same_target = channel == default_channel && chat_id == default_chat_id;
        let message_id = if same_target {
            explicit_message_id.or(default_message_id)
        } else {
            None
        };

        if channel.is_empty() || chat_id.is_empty() {
            return Ok(Value::String(
                "Error: No target channel/chat specified".into(),
            ));
        }

        let media = if media.is_empty() {
            Vec::new()
        } else {
            match self.resolve_media(&media) {
                Ok(m) => m,
                Err(e) => return Ok(Value::String(format!("Error: {e}"))),
            }
        };

        let mut metadata: Map<String, Value> = if same_target {
            default_metadata.into_iter().collect()
        } else {
            Map::new()
        };
        if let Some(mid) = &message_id {
            metadata.insert("message_id".into(), Value::String(mid.clone()));
        }
        if metadata.get("_record_channel_delivery").is_none() && (!media.is_empty()) {
            metadata.insert("_record_channel_delivery".into(), Value::Bool(true));
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
            let mut ctx = self.context.lock().unwrap();
            ctx.sent_in_turn = true;
            if !media.is_empty() {
                ctx.turn_delivered_media.extend(media.iter().cloned());
            }
        }
        let media_info = if media.is_empty() {
            String::new()
        } else {
            format!(" with {} attachments", media.len())
        };
        let button_info = if buttons.is_empty() {
            String::new()
        } else {
            format!(
                " with {} button(s)",
                buttons.iter().map(|r| r.len()).sum::<usize>()
            )
        };
        Ok(Value::String(format!(
            "Message sent to {channel}:{chat_id}{media_info}{button_info}"
        )))
    }
}
