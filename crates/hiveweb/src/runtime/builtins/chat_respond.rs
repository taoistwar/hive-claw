use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{BuiltinContext, BuiltinError, BuiltinResult};

#[derive(Debug, Deserialize)]
struct ChatRespondArgs {
    content: String,
}

#[derive(Debug, Serialize)]
struct ChatRespondReply {
    /// orchestrator uses this field to identify the content as the "final user-visible reply"
    final_content: String,
}

/// chat.respond signals orchestrator to "submit the final reply";
/// orchestrator takes the final_content field as the done event's final content.
/// This function itself just passes through + wraps.
pub fn chat_respond(args: Value, _ctx: &BuiltinContext) -> BuiltinResult {
    let parsed: ChatRespondArgs =
        serde_json::from_value(args).map_err(|e| BuiltinError::BadArgs(format!("{e}")))?;
    Ok(serde_json::to_value(ChatRespondReply {
        final_content: parsed.content,
    })
    .unwrap_or(Value::Null))
}

pub const CHAT_RESPOND_INPUT_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "content": { "type": "string", "description": "Final user-visible message" }
  },
  "required": ["content"]
}"#;

pub const CHAT_RESPOND_OUTPUT_SCHEMA: &str = r#"{
  "type": "object",
  "properties": { "final_content": { "type": "string" } }
}"#;
