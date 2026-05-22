//! LLM-based heartbeat decider implementation.
//!
//! Mirrors the Python `_decide` method which uses tool-use to get
//! structured skip/run decisions from the LLM.

use std::sync::Arc;

use async_trait::async_trait;
use log::warn;
use providers::{
    base::{ChatRequest, LLMProvider, RetryMode},
    types::ToolCallRequest,
};
use serde_json::json;

use crate::service::{HeartbeatAction, HeartbeatDecider, HeartbeatDecision};

/// Tool definition sent to the LLM for heartbeat decisions.
/// Mirrors the Python `_HEARTBEAT_TOOL` constant.
const HEARTBEAT_TOOL: &str = r#"
{
    "type": "function",
    "function": {
        "name": "heartbeat",
        "description": "Report heartbeat decision after reviewing tasks.",
        "parameters": {
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["skip", "run"],
                    "description": "skip = nothing to do, run = has active tasks"
                },
                "tasks": {
                    "type": "string",
                    "description": "Natural-language summary of active tasks (required for run)"
                }
            },
            "required": ["action"]
        }
    }
}
"#;

/// LLM-backed heartbeat decider.
pub struct LLMHeartbeatDecider {
    provider: Arc<dyn LLMProvider>,
    model: String,
}

impl LLMHeartbeatDecider {
    pub fn new(provider: Arc<dyn LLMProvider>, model: String) -> Self {
        Self { provider, model }
    }

    /// Build the system and user messages for the heartbeat decision.
    fn build_messages(&self, content: &str, timezone: Option<&str>) -> Vec<serde_json::Value> {
        let time_str = utils::helpers::current_time_str(timezone);

        vec![
            json!({
                "role": "system",
                "content": "You are a heartbeat agent. Call the heartbeat tool to report your decision."
            }),
            json!({
                "role": "user",
                "content": format!(
                    "Current Time: {time_str}\n\nReview the following HEARTBEAT.md and decide whether there are active tasks.\n\n{content}"
                )
            }),
        ]
    }

    /// Parse tool call arguments from the LLM response.
    fn parse_tool_call(tool_call: &ToolCallRequest) -> (HeartbeatAction, String) {
        let action = tool_call
            .arguments
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("skip");

        let tasks = tool_call
            .arguments
            .get("tasks")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let action = if action == "run" {
            HeartbeatAction::Run
        } else {
            HeartbeatAction::Skip
        };

        (action, tasks)
    }
}

#[async_trait]
impl HeartbeatDecider for LLMHeartbeatDecider {
    async fn decide(&self, content: &str, timezone: Option<&str>) -> HeartbeatDecision {
        let messages = self.build_messages(content, timezone);
        let tool_def: serde_json::Value =
            serde_json::from_str(HEARTBEAT_TOOL).expect("valid HEARTBEAT_TOOL JSON");

        let req = ChatRequest {
            messages,
            tools: Some(vec![tool_def]),
            model: Some(self.model.clone()),
            max_tokens: 512,
            temperature: 0.0,
            reasoning_effort: None,
            tool_choice: None,
        };

        let response = self.provider.chat_with_retry(req, RetryMode::Standard, None).await;

        // Check if we should execute tools (has tool calls and finish_reason is appropriate)
        if !response.should_execute_tools() {
            if response.has_tool_calls() {
                warn!(
                    "Ignoring heartbeat tool calls under finish_reason='{}'",
                    response.finish_reason
                );
            }
            return HeartbeatDecision {
                action: HeartbeatAction::Skip,
                tasks: String::new(),
            };
        }

        // Parse the first tool call
        if let Some(tool_call) = response.tool_calls.first() {
            let (action, tasks) = Self::parse_tool_call(tool_call);
            HeartbeatDecision { action, tasks }
        } else {
            HeartbeatDecision {
                action: HeartbeatAction::Skip,
                tasks: String::new(),
            }
        }
    }
}
