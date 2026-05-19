//! Core value types shared by all LLM providers.
//!
//! Port of `nanobot.providers.base` data structures (ToolCallRequest, LLMResponse, ...).

use std::collections::HashMap;

use config::Config;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One tool call emitted by an LLM (function-calling style).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ToolCallRequest {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub arguments: serde_json::Map<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra_content: Option<HashMap<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_specific_fields: Option<HashMap<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function_provider_specific_fields: Option<HashMap<String, Value>>,
}

impl ToolCallRequest {
    /// Serialize to an OpenAI-style `tool_call` payload.
    pub fn to_openai_tool_call(&self) -> Value {
        let args_str = serde_json::to_string(&self.arguments).unwrap_or_else(|_| "{}".to_string());
        let mut func = serde_json::json!({
            "name": self.name,
            "arguments": args_str,
        });
        if let Some(ref f) = self.function_provider_specific_fields {
            func["provider_specific_fields"] = serde_json::to_value(f).unwrap_or(Value::Null);
        }

        let mut out = serde_json::json!({
            "id": self.id,
            "type": "function",
            "function": func,
        });
        if let Some(ref e) = self.extra_content {
            out["extra_content"] = serde_json::to_value(e).unwrap_or(Value::Null);
        }
        if let Some(ref p) = self.provider_specific_fields {
            out["provider_specific_fields"] = serde_json::to_value(p).unwrap_or(Value::Null);
        }
        out
    }
}

/// Finish reason mirroring OpenAI semantics. Kept as a string for forward-compat.
pub type FinishReason = String;

/// Response from an LLM provider.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LLMResponse {
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub tool_calls: Vec<ToolCallRequest>,
    #[serde(default = "default_finish_reason")]
    pub finish_reason: FinishReason,
    #[serde(default)]
    pub usage: HashMap<String, i64>,
    #[serde(default)]
    pub retry_after: Option<f64>,
    #[serde(default)]
    pub reasoning_content: Option<String>,
    #[serde(default)]
    pub thinking_blocks: Option<Vec<Value>>,
    // Structured error metadata (used by retry policy).
    #[serde(default)]
    pub error_status_code: Option<i32>,
    #[serde(default)]
    pub error_kind: Option<String>,
    #[serde(default)]
    pub error_type: Option<String>,
    #[serde(default)]
    pub error_code: Option<String>,
    #[serde(default)]
    pub error_retry_after_s: Option<f64>,
    #[serde(default)]
    pub error_should_retry: Option<bool>,
}

fn default_finish_reason() -> String {
    "stop".into()
}

impl LLMResponse {
    pub fn has_tool_calls(&self) -> bool {
        !self.tool_calls.is_empty()
    }

    /// Tools execute only when `has_tool_calls` AND finish_reason is
    /// `tool_calls` / `stop`. Refusal/content_filter/error paths are blocked.
    pub fn should_execute_tools(&self) -> bool {
        self.has_tool_calls() && matches!(self.finish_reason.as_str(), "tool_calls" | "stop")
    }

    pub fn is_error(&self) -> bool {
        self.finish_reason == "error"
    }

    pub fn error(msg: impl Into<String>) -> Self {
        Self {
            content: Some(msg.into()),
            finish_reason: "error".into(),
            ..Default::default()
        }
    }
}

/// Generation defaults for a provider.
#[derive(Debug, Clone)]
pub struct GenerationSettings {
    pub temperature: f32,
    pub max_tokens: u32,
    pub reasoning_effort: Option<String>,
}

impl Default for GenerationSettings {
    fn default() -> Self {
        Self {
            temperature: 0.7,
            max_tokens: 4096,
            reasoning_effort: None,
        }
    }
}

impl GenerationSettings {
    pub fn from_config(cfg: &Config) -> Self {
        let d = &cfg.agents.defaults;
        Self {
            temperature: d.temperature,
            max_tokens: d.max_tokens,
            reasoning_effort: d.reasoning_effort.clone(),
        }
    }
}

/// Optional tool-selection strategy passed to `chat`.
#[derive(Debug, Clone)]
pub enum ToolChoice {
    Auto,
    Required,
    None,
    Specific(Value),
}
