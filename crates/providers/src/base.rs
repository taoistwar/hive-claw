//! `LLMProvider` trait — async abstraction over chat completion backends.
//!
//! Port of `nanobot.providers.base.LLMProvider`. Heavy helpers (message
//! sanitization, retry policy) are implemented here so concrete backends only
//! need to provide `chat`.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Duration;

use async_trait::async_trait;
use log::warn;
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use tokio::time::sleep;

// ===========================================================================
// Types (merged from types.rs)
// ===========================================================================

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

fn default_finish_reason() -> String {
    "stop".into()
}

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

impl LLMResponse {
    pub fn has_tool_calls(&self) -> bool {
        !self.tool_calls.is_empty()
    }

    /// Tools execute only when `has_tool_calls` AND finish_reason is
    /// `tool_calls` / `function_call` / `stop`. Refusal/content_filter/error paths are blocked.
    pub fn should_execute_tools(&self) -> bool {
        self.has_tool_calls()
            && matches!(
                self.finish_reason.as_str(),
                "tool_calls" | "function_call" | "stop"
            )
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
    pub fn from_config(cfg: &config::Config) -> Self {
        let d = &cfg.agents.defaults;
        Self {
            temperature: d.temperature,
            max_tokens: d.max_tokens,
            reasoning_effort: d.reasoning_effort.clone(),
        }
    }
}

/// Optional tool-selection strategy passed to `chat`.
#[derive(Debug, Clone, Serialize)]
pub enum ToolChoice {
    Auto,
    Required,
    None,
    Specific(Value),
}

// ===========================================================================
// Sanitization helpers (merged from sanitize.rs)
// ===========================================================================

const SYNTHETIC_USER_CONTENT: &str = "(conversation continued)";

/// Placeholder text used to replace image_url blocks.
fn image_placeholder_text(path: &str, empty: &str) -> String {
    if path.is_empty() {
        empty.to_string()
    } else {
        format!("[image: {path}]")
    }
}

/// Sanitize message content: fix empty blocks, strip internal `_meta` fields.
pub fn sanitize_empty_content(messages: &[Value]) -> Vec<Value> {
    messages
        .iter()
        .map(|msg| sanitize_empty_content_one(msg.clone()))
        .collect()
}

fn sanitize_empty_content_one(mut msg: Value) -> Value {
    let Some(obj) = msg.as_object_mut() else {
        return msg;
    };
    let role = obj.get("role").and_then(|v| v.as_str()).map(String::from);
    let has_tool_calls = obj
        .get("tool_calls")
        .map(|v| !v.is_null())
        .unwrap_or(false);

    match obj.get("content").cloned() {
        Some(Value::String(s)) if s.is_empty() => {
            let fixed = if matches!(role.as_deref(), Some("assistant")) && has_tool_calls {
                Value::Null
            } else {
                Value::String("(empty)".into())
            };
            obj.insert("content".into(), fixed);
        }
        Some(Value::Array(items)) => {
            let mut new_items: Vec<Value> = Vec::with_capacity(items.len());
            let mut changed = false;
            for item in items {
                if let Value::Object(ref map) = item {
                    let ty = map.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    if matches!(ty, "text" | "input_text" | "output_text")
                        && map
                            .get("text")
                            .and_then(|v| v.as_str())
                            .map_or(true, |s| s.is_empty())
                    {
                        changed = true;
                        continue;
                    }
                    if map.contains_key("_meta") {
                        let stripped: Map<String, Value> = map
                            .iter()
                            .filter(|(k, _)| k.as_str() != "_meta")
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect();
                        new_items.push(Value::Object(stripped));
                        changed = true;
                        continue;
                    }
                }
                new_items.push(item);
            }
            if changed {
                if !new_items.is_empty() {
                    obj.insert("content".into(), Value::Array(new_items));
                } else if matches!(role.as_deref(), Some("assistant")) && has_tool_calls {
                    obj.insert("content".into(), Value::Null);
                } else {
                    obj.insert("content".into(), Value::String("(empty)".into()));
                }
            }
        }
        Some(Value::Object(single)) => {
            obj.insert("content".into(), Value::Array(vec![Value::Object(single)]));
        }
        _ => {}
    }
    msg
}

/// Merge consecutive same-role messages and drop trailing assistant messages.
pub fn enforce_role_alternation(messages: &[Value]) -> Vec<Value> {
    if messages.is_empty() {
        return Vec::new();
    }

    let mut merged: Vec<Value> = Vec::with_capacity(messages.len());

    for msg in messages.iter().cloned() {
        let role = msg
            .get("role")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let should_merge = {
            if merged.is_empty() {
                false
            } else {
                let prev_role = merged
                    .last()
                    .and_then(|m| m.get("role"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                prev_role == role
                    && (role == "user" || role == "assistant")
                    && role != "system"
                    && role != "tool"
            }
        };

        if should_merge {
            let prev_has_tools = merged
                .last()
                .and_then(|m| m.get("tool_calls"))
                .map(|v| !v.is_null())
                .unwrap_or(false);
            let curr_has_tools = msg
                .get("tool_calls")
                .map(|v| !v.is_null())
                .unwrap_or(false);

            if role == "assistant" {
                if curr_has_tools {
                    *merged.last_mut().unwrap() = msg;
                    continue;
                }
                if prev_has_tools {
                    continue;
                }
            }

            let prev_content = merged
                .last()
                .and_then(|m| m.get("content"))
                .cloned()
                .unwrap_or(Value::String(String::new()));
            let curr_content = msg.get("content").cloned().unwrap_or(Value::Null);
            if let (Value::String(prev), Value::String(curr)) = (&prev_content, &curr_content) {
                if let Some(prev_obj) = merged.last_mut().and_then(|m| m.as_object_mut()) {
                    let s = format!("{prev}\n\n{curr}").trim().to_string();
                    prev_obj.insert("content".into(), Value::String(s));
                }
            } else {
                *merged.last_mut().unwrap() = msg;
            }
        } else {
            merged.push(msg);
        }
    }

    // Drop trailing assistant messages.
    let mut last_popped: Option<Value> = None;
    while let Some(last) = merged.last() {
        if last.get("role").and_then(|v| v.as_str()) == Some("assistant") {
            last_popped = Some(merged.pop().unwrap());
        } else {
            break;
        }
    }

    // If only system messages remain, recover the popped assistant as user.
    let has_user_or_tool = merged.iter().any(|m| {
        matches!(
            m.get("role").and_then(|v| v.as_str()),
            Some("user") | Some("tool")
        )
    });
    if !merged.is_empty() && !has_user_or_tool {
        if let Some(mut recovered) = last_popped {
            if let Some(obj) = recovered.as_object_mut() {
                obj.insert("role".into(), Value::String("user".into()));
            }
            merged.push(recovered);
        }
    }

    // Ensure first non-system message isn't a bare assistant.
    for i in 0..merged.len() {
        let role = merged[i].get("role").and_then(|v| v.as_str()).unwrap_or("");
        if role != "system" {
            let has_tools = merged[i]
                .get("tool_calls")
                .map(|v| !v.is_null())
                .unwrap_or(false);
            if role == "assistant" && !has_tools {
                merged.insert(
                    i,
                    serde_json::json!({"role": "user", "content": SYNTHETIC_USER_CONTENT}),
                );
            }
            break;
        }
    }

    merged
}

/// Replace image_url blocks with text placeholder. Returns None if no images found.
pub fn strip_image_content(messages: &[Value]) -> Option<Vec<Value>> {
    let mut found = false;
    let result: Vec<Value> = messages
        .iter()
        .map(|msg| {
            let content = msg.get("content");
            if let Some(Value::Array(blocks)) = content {
                let new_content: Vec<Value> = blocks
                    .iter()
                    .map(|b| {
                        if let Some(obj) = b.as_object() {
                            if obj.get("type").and_then(|v| v.as_str()) == Some("image_url") {
                                found = true;
                                let path = obj
                                    .get("_meta")
                                    .and_then(|m| m.get("path"))
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("");
                                let placeholder = image_placeholder_text(path, "[image omitted]");
                                json!({"type": "text", "text": placeholder})
                            } else {
                                b.clone()
                            }
                        } else {
                            b.clone()
                        }
                    })
                    .collect();
                let mut new_msg = msg.clone();
                if let Some(obj) = new_msg.as_object_mut() {
                    obj.insert("content".into(), Value::Array(new_content));
                }
                new_msg
            } else {
                msg.clone()
            }
        })
        .collect();

    if found {
        Some(result)
    } else {
        None
    }
}

/// Replace image_url blocks with text placeholder *in-place*.
///
/// Mutates the content lists of the original message dicts so that
/// callers holding references to those dicts also see the stripped
/// version. Returns true if any images were found and replaced.
pub fn strip_image_content_inplace(messages: &mut Vec<Value>) -> bool {
    let mut found = false;
    for msg in messages.iter_mut() {
        if let Some(content) = msg.get_mut("content") {
            if let Value::Array(blocks) = content {
                for b in blocks.iter_mut() {
                    if let Some(obj) = b.as_object_mut() {
                        if obj.get("type").and_then(|v| v.as_str()) == Some("image_url") {
                            found = true;
                            let path = obj
                                .get("_meta")
                                .and_then(|m| m.get("path"))
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            let placeholder = image_placeholder_text(path, "[image omitted]");
                            *b = json!({"type": "text", "text": placeholder});
                        }
                    }
                }
            }
        }
    }
    found
}

/// Extract tool name from either OpenAI or Anthropic-style tool schemas.
fn tool_name(tool: &Value) -> String {
    if let Some(name) = tool.get("name").and_then(|v| v.as_str()) {
        return name.to_string();
    }
    if let Some(fn_obj) = tool.get("function").and_then(|v| v.as_object()) {
        if let Some(fname) = fn_obj.get("name").and_then(|v| v.as_str()) {
            return fname.to_string();
        }
    }
    String::new()
}

/// Return cache marker indices: builtin/MCP boundary and tail index.
fn tool_cache_marker_indices(tools: &[Value]) -> Vec<usize> {
    if tools.is_empty() {
        return vec![];
    }

    let tail_idx = tools.len() - 1;
    let mut last_builtin_idx: Option<usize> = None;
    for i in (0..=tail_idx).rev() {
        if !tool_name(&tools[i]).starts_with("mcp_") {
            last_builtin_idx = Some(i);
            break;
        }
    }

    let mut ordered_unique: Vec<usize> = Vec::with_capacity(2);
    for idx in last_builtin_idx.into_iter().chain(Some(tail_idx)) {
        if !ordered_unique.contains(&idx) {
            ordered_unique.push(idx);
        }
    }
    ordered_unique
}

/// Keep only provider-safe message keys and normalize assistant content.
pub fn sanitize_request_messages(messages: &[Value], allowed_keys: &std::collections::HashSet<String>) -> Vec<Value> {
    messages
        .iter()
        .map(|msg| {
            if let Some(obj) = msg.as_object() {
                let clean: Map<String, Value> = obj
                    .iter()
                    .filter(|(k, _)| allowed_keys.contains(*k))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                let mut clean_val = Value::Object(clean);
                if let Some(obj) = clean_val.as_object_mut() {
                    if obj.get("role").and_then(|v| v.as_str()) == Some("assistant")
                        && !obj.contains_key("content")
                    {
                        obj.insert("content".into(), Value::Null);
                    }
                }
                clean_val
            } else {
                msg.clone()
            }
        })
        .collect()
}

/// Extract retry-after from HTTP headers (Retry-After, Retry-After-Ms).
pub fn extract_retry_after_from_headers(headers: &Value) -> Option<f64> {
    fn header_value(headers: &Value, name: &str) -> Option<String> {
        if let Some(obj) = headers.as_object() {
            if let Some(v) = obj.get(name) {
                return Some(v.as_str()?.to_string());
            }
            let name_lower = name.to_lowercase();
            for (k, v) in obj {
                if k.to_lowercase() == name_lower {
                    return Some(v.as_str()?.to_string());
                }
            }
        }
        None
    }

    if let Some(retry_ms_str) = header_value(headers, "retry-after-ms") {
        if let Ok(value) = retry_ms_str.parse::<f64>() {
            if value > 0.0 {
                return Some(value / 1000.0);
            }
        }
    }

    let retry_after_str = header_value(headers, "retry-after")?;
    let retry_after_text = retry_after_str.trim();
    if retry_after_text.is_empty() {
        return None;
    }

    if retry_after_text
        .parse::<f64>()
        .is_ok()
    {
        if let Ok(seconds) = retry_after_text.parse::<f64>() {
            return Some(to_retry_seconds(seconds, Some("s")));
        }
    }

    None
}

// ===========================================================================
// Retry helpers (merged from retry.rs)
// ===========================================================================

const TRANSIENT_ERROR_MARKERS: &[&str] = &[
    "429",
    "rate limit",
    "500",
    "502",
    "503",
    "504",
    "overloaded",
    "timeout",
    "timed out",
    "connection",
    "server error",
    "temporarily unavailable",
    "速率限制",
    "访问量过大",
];

const NON_RETRYABLE_429_TEXT: &[&str] = &[
    "insufficient_quota",
    "insufficient quota",
    "quota exceeded",
    "quota exhausted",
    "billing hard limit",
    "billing_hard_limit_reached",
    "billing not active",
    "insufficient balance",
    "insufficient_balance",
    "credit balance too low",
    "payment required",
    "out of credits",
    "out of quota",
    "exceeded your current quota",
];

const RETRYABLE_429_TEXT: &[&str] = &[
    "rate limit",
    "rate_limit",
    "too many requests",
    "retry after",
    "try again in",
    "temporarily unavailable",
    "overloaded",
    "concurrency limit",
    "速率限制",
];

const NON_RETRYABLE_429_TOKENS: &[&str] = &[
    "insufficient_quota",
    "quota_exceeded",
    "quota_exhausted",
    "billing_hard_limit_reached",
    "insufficient_balance",
    "credit_balance_too_low",
    "billing_not_active",
    "payment_required",
];

const RETRYABLE_429_TOKENS: &[&str] = &[
    "rate_limit_exceeded",
    "rate_limit_error",
    "too_many_requests",
    "request_limit_exceeded",
    "requests_limit_exceeded",
    "overloaded_error",
];

const RETRYABLE_STATUS_CODES: &[i32] = &[408, 409, 429];
const TRANSIENT_ERROR_KINDS: &[&str] = &["timeout", "connection"];

pub fn is_transient_text(content: Option<&str>) -> bool {
    let Some(s) = content else {
        return false;
    };
    let lower = s.to_ascii_lowercase();
    TRANSIENT_ERROR_MARKERS
        .iter()
        .any(|m| lower.contains(m))
}

pub fn is_transient_response(resp: &LLMResponse) -> bool {
    if let Some(flag) = resp.error_should_retry {
        return flag;
    }

    if let Some(status) = resp.error_status_code {
        if status == 429 {
            return is_retryable_429(resp);
        }
        if RETRYABLE_STATUS_CODES.contains(&status) || status >= 500 {
            return true;
        }
    }

    if let Some(kind) = resp.error_kind.as_deref() {
        let k = kind.trim().to_ascii_lowercase();
        if TRANSIENT_ERROR_KINDS.contains(&k.as_str()) {
            return true;
        }
    }

    is_transient_text(resp.content.as_deref())
}

fn normalize_token(v: Option<&str>) -> Option<String> {
    v.map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
}

fn is_retryable_429(resp: &LLMResponse) -> bool {
    let ty = normalize_token(resp.error_type.as_deref());
    let code = normalize_token(resp.error_code.as_deref());
    let tokens: Vec<&str> = [ty.as_deref(), code.as_deref()]
        .into_iter()
        .flatten()
        .collect();

    if tokens
        .iter()
        .any(|t| NON_RETRYABLE_429_TOKENS.contains(t))
    {
        return false;
    }

    let content_lower = resp
        .content
        .as_deref()
        .unwrap_or("")
        .to_ascii_lowercase();
    if NON_RETRYABLE_429_TEXT
        .iter()
        .any(|m| content_lower.contains(m))
    {
        return false;
    }

    if tokens.iter().any(|t| RETRYABLE_429_TOKENS.contains(t)) {
        return true;
    }
    if RETRYABLE_429_TEXT
        .iter()
        .any(|m| content_lower.contains(m))
    {
        return true;
    }
    // Unknown 429 => retry.
    true
}

static RETRY_AFTER_RES: Lazy<[Regex; 4]> = Lazy::new(|| {
    [
        Regex::new(r"retry after\s+(\d+(?:\.\d+)?)\s*(ms|milliseconds|s|sec|secs|seconds|m|min|minutes)?").unwrap(),
        Regex::new(r"try again in\s+(\d+(?:\.\d+)?)\s*(ms|milliseconds|s|sec|secs|seconds|m|min|minutes)").unwrap(),
        Regex::new(r"wait\s+(\d+(?:\.\d+)?)\s*(ms|milliseconds|s|sec|secs|seconds|m|min|minutes)\s*before retry").unwrap(),
        Regex::new(r#"retry[_-]?after["'\s:=]+(\d+(?:\.\d+)?)"#).unwrap(),
    ]
});

pub fn extract_retry_after_from_text(content: Option<&str>) -> Option<f64> {
    let text = content?.to_ascii_lowercase();
    for (idx, re) in RETRY_AFTER_RES.iter().enumerate() {
        if let Some(cap) = re.captures(&text) {
            let value: f64 = cap.get(1)?.as_str().parse().ok()?;
            let unit = if idx < 3 {
                cap.get(2).map(|m| m.as_str().to_string())
            } else {
                Some("s".into())
            };
            return Some(to_retry_seconds(value, unit.as_deref()));
        }
    }
    None
}

fn to_retry_seconds(value: f64, unit: Option<&str>) -> f64 {
    let u = unit.unwrap_or("s").to_ascii_lowercase();
    match u.as_str() {
        "ms" | "milliseconds" => (value / 1000.0).max(0.1),
        "m" | "min" | "minutes" => (value * 60.0).max(0.1),
        _ => value.max(0.1),
    }
}

/// Preferred delay source: explicit `error_retry_after_s` > `retry_after` > text.
pub fn pick_delay(resp: &LLMResponse) -> Option<f64> {
    if let Some(v) = resp.error_retry_after_s {
        if v > 0.0 {
            return Some(v);
        }
    }
    if let Some(v) = resp.retry_after {
        if v > 0.0 {
            return Some(v);
        }
    }
    extract_retry_after_from_text(resp.content.as_deref())
}

// ===========================================================================
// LLMProvider trait
// ===========================================================================

/// Request passed to [`LLMProvider::chat`].
///
/// `messages` and `tools` follow OpenAI's JSON schema shape but are kept
/// as `serde_json::Value` so providers can adapt to their own wire formats.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ChatRequest {
    pub messages: Vec<Value>,
    pub tools: Option<Vec<Value>>,
    pub model: Option<String>,
    pub max_tokens: u32,
    pub temperature: f32,
    pub reasoning_effort: Option<String>,
    pub tool_choice: Option<ToolChoice>,
}

/// Mode for the built-in retry policy (mirrors Python `"standard" | "persistent"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryMode {
    Standard,
    Persistent,
}

impl RetryMode {
    pub fn from_str(s: &str) -> Self {
        if s == "persistent" {
            Self::Persistent
        } else {
            Self::Standard
        }
    }
}

/// Callback invoked during retry backoff (for UI/heartbeat).
pub type RetryWaitCallback =
    std::sync::Arc<dyn Fn(String) -> futures::future::BoxFuture<'static, ()> + Send + Sync>;

/// Callback invoked on each streaming content delta.
pub type StreamDeltaCallback = std::sync::Arc<dyn Fn(String) + Send + Sync>;

/// Callback invoked on each streaming tool call delta (from SSE parsing).
pub use crate::responses::parsing::ToolCallDeltaCallback;

const PERSISTENT_MAX_DELAY: u64 = 60;
const PERSISTENT_IDENTICAL_ERROR_LIMIT: u32 = 10;
const RETRY_HEARTBEAT_CHUNK: u64 = 30;

// ---------------------------------------------------------------------------
// Global Langfuse client (set once at startup)
// ---------------------------------------------------------------------------

static LANGFUSE_CLIENT: OnceLock<Option<std::sync::Arc<langfuse::LangfuseClient>>> = OnceLock::new();

/// Initialise the global Langfuse client.
///
/// Call once at startup (e.g. from `hiveweb::main` or `cli::main`).
/// Pass `None` to disable tracing.
pub fn set_langfuse_client(client: Option<std::sync::Arc<langfuse::LangfuseClient>>) {
    langfuse::lf_debug!("providers::set_langfuse_client called, client_is_some={}", client.is_some());
    match LANGFUSE_CLIENT.set(client) {
        Ok(()) => {
            langfuse::lf_debug!("providers::set_langfuse_client -> OK (global client set)");
            log::info!("Langfuse: global client set successfully");
        }
        Err(_) => {
            langfuse::lf_debug!("providers::set_langfuse_client -> ERROR (OnceLock already set)");
            log::error!("Langfuse: set_langfuse_client called more than once (OnceLock already set)");
        }
    }
}

/// Returns a reference to the global Langfuse client, if initialized.
pub fn get_langfuse_client() -> Option<&'static std::sync::Arc<langfuse::LangfuseClient>> {
    let client = LANGFUSE_CLIENT.get();
    match client {
        None => {
            langfuse::lf_debug!("providers::get_langfuse_client -> None (OnceLock not initialized)");
            log::warn!("Langfuse: LANGFUSE_CLIENT OnceLock not initialized");
            None
        }
        Some(None) => {
            langfuse::lf_debug!("providers::get_langfuse_client -> None (explicitly disabled)");
            // Langfuse was explicitly disabled at startup
            None
        }
        Some(Some(c)) => {
            langfuse::lf_debug!("providers::get_langfuse_client -> Some(client)");
            Some(c)
        }
    }
}

// ---------------------------------------------------------------------------
// LLMProvider trait
// ---------------------------------------------------------------------------
#[async_trait]
pub trait LLMProvider: Send + Sync {
    /// Provider's default model when none is passed explicitly.
    fn default_model(&self) -> String;

    /// Generation defaults (temperature, max_tokens, ...).
    fn generation(&self) -> GenerationSettings {
        GenerationSettings::default()
    }

    /// Single chat completion call. Concrete providers implement this.
    async fn chat(&self, req: ChatRequest) -> LLMResponse;

    /// Whether this provider supports progress delta streaming. Default: false.
    fn supports_progress_deltas(&self) -> bool {
        false
    }

    /// Streaming chat completion. Concrete providers implement this.
    async fn chat_stream(
        &self,
        req: ChatRequest,
        on_delta: Option<StreamDeltaCallback>,
        on_tool_call_delta: Option<ToolCallDeltaCallback>,
    ) -> LLMResponse {
        let response = self.chat(req).await;
        if let Some(cb) = on_delta {
            if let Some(text) = response.content.clone() {
                if !text.is_empty() {
                    cb(text);
                }
            }
        }
        response
    }

    /// Wrapper with retry policy on transient errors.
    async fn chat_with_retry(
        &self,
        mut req: ChatRequest,
        mode: RetryMode,
        on_retry_wait: Option<RetryWaitCallback>,
        langfuse_trace: Option<&langfuse::TraceHandle>,
    ) -> LLMResponse {
        // --- Langfuse instrumentation (non-blocking) ---
        let lf = langfuse_trace
            .map(|t| std::borrow::Cow::Borrowed(t))
            .or_else(|| {
                get_langfuse_client().map(|c| {
                    std::borrow::Cow::Owned(c.trace("chat_with_retry", None))
                })
            });
        langfuse::lf_debug!("chat_with_retry: langfuse trace available: trace_id={}", lf.as_ref().map(|t| t.id()).unwrap_or("None"));
        let full_request = json!({
            "model": &req.model,
            "messages": &req.messages,
            "tools": &req.tools,
            "tool_choice": &req.tool_choice,
            "max_tokens": req.max_tokens,
            "temperature": req.temperature,
            "reasoning_effort": &req.reasoning_effort,
        });
        let model_params = json!({
            "max_tokens": req.max_tokens,
            "temperature": req.temperature,
            "reasoning_effort": req.reasoning_effort,
            "tools": req.tools.as_ref(),
            "tool_choice": req.tool_choice.as_ref(),
        });
        let gen_handle = lf.as_ref().map(|t| {
            langfuse::lf_debug!("chat_with_retry: creating generation under trace_id={} model={:?}",
                t.id(), req.model);
            t.generation(
                "LLM Chat",
                req.model.clone(),
                Some(full_request),
                Some(model_params),
            )
        });
        langfuse::lf_debug!("chat_with_retry: gen_handle created, is_some={}", gen_handle.is_some());

        let defaults = self.generation();
        if req.max_tokens == 0 {
            req.max_tokens = defaults.max_tokens;
        }
        if !req.temperature.is_finite() {
            req.temperature = defaults.temperature;
        }
        if req.reasoning_effort.is_none() {
            req.reasoning_effort = defaults.reasoning_effort.clone();
        }

        // Enforce role alternation to avoid provider errors (e.g., consecutive same-role messages)
        req.messages = enforce_role_alternation(&req.messages);

        let delays: [u64; 3] = [1, 2, 4];
        let persistent = mode == RetryMode::Persistent;
        let mut attempt: u32 = 0;
        let mut last_error_key: Option<String> = None;
        let mut identical_count: u32 = 0;
        let mut last_response: Option<LLMResponse>;
        let mut images_stripped = false;

        loop {
            attempt += 1;
            let response = self.chat(req.clone()).await;
            if response.finish_reason != "error" {
                langfuse::lf_debug!("chat_with_retry: success on attempt {}, ending generation", attempt);
                let usage = extract_usage_tuple(&response.usage);
                let output = build_langfuse_output(&response);
                if let Some(g) = gen_handle {
                    langfuse::lf_debug!("chat_with_retry: calling gen_handle.end() success path, usage={:?}", usage);
                    g.emit_tools_from_output(&output);
                    g.end(
                        Some(output.clone()),
                        usage,
                        Some(response.finish_reason.clone()),
                        false,
                    );
                }
                if let Some(t) = lf.as_ref() {
                    t.set_output(Some(output));
                }
                langfuse::lf_debug!("chat_with_retry: returning success response");
                return response;
            }

            langfuse::lf_debug!("chat_with_retry: error on attempt {}, finish_reason={}", attempt, response.finish_reason);
            let key = response
                .content
                .as_deref()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            if !key.is_empty() && Some(&key) == last_error_key.as_ref() {
                identical_count = identical_count.saturating_add(1);
            } else {
                last_error_key = if key.is_empty() { None } else { Some(key) };
                identical_count = if last_error_key.is_some() { 1 } else { 0 };
            }
            last_response = Some(response.clone());

            if !is_transient_response(&response) {
                langfuse::lf_debug!("chat_with_retry: non-transient error, images_stripped={}", images_stripped);
                // Non-transient error: try stripping images and retrying once immediately
                if !images_stripped && strip_image_content_inplace(&mut req.messages) {
                    warn!("Non-transient LLM error with image content, retrying without images");
                    images_stripped = true;
                    let result = self.chat(req.clone()).await;
                    if result.finish_reason != "error" {
                        // Permanently strip images from the original messages so
                        // subsequent iterations do not repeat the error-retry cycle.
                        strip_image_content_inplace(&mut req.messages);
                    }
                    let usage = extract_usage_tuple(&result.usage);
                    let output = build_langfuse_output(&result);
                    if let Some(g) = gen_handle {
                        langfuse::lf_debug!("chat_with_retry: ending generation after image strip, is_error={}", result.finish_reason == "error");
                        g.emit_tools_from_output(&output);
                        g.end(
                            Some(output.clone()),
                            usage,
                            Some(result.finish_reason.clone()),
                            result.finish_reason == "error",
                        );
                    }
                    if !result.is_error() {
                        if let Some(t) = lf.as_ref() { t.set_output(Some(output)); }
                    }
                    langfuse::lf_debug!("chat_with_retry: returning after image strip");
                    return result;
                }
                let usage = extract_usage_tuple(&response.usage);
                if let Some(g) = gen_handle {
                    langfuse::lf_debug!("chat_with_retry: ending generation non-transient error path");
                    g.end(
                        response.content.clone().map(|c| Value::String(c)),
                        usage,
                        Some(response.finish_reason.clone()),
                        true,
                    );
                }
                langfuse::lf_debug!("chat_with_retry: returning non-transient error response");
                return response;
            }

            if persistent && identical_count >= PERSISTENT_IDENTICAL_ERROR_LIMIT {
                warn!(
                    "Stopping persistent retry after {identical_count} identical transient errors: {}",
                    response.content.as_deref().unwrap_or("").chars().take(120).collect::<String>().to_lowercase()
                );
                langfuse::lf_debug!("chat_with_retry: persistent retry limit reached, identical_count={}", identical_count);
                if let Some(cb) = on_retry_wait.as_ref() {
                    (cb)(format!(
                        "Persistent retry stopped after {identical_count} identical errors."
                    ))
                    .await;
                }
                let usage = extract_usage_tuple(&response.usage);
                if let Some(g) = gen_handle {
                    langfuse::lf_debug!("chat_with_retry: ending generation persistent retry limit path");
                    g.emit_tools_from_output(&build_langfuse_output(&response));
                    g.end(
                        response.content.clone().map(|c| Value::String(c)),
                        usage,
                        Some(response.finish_reason.clone()),
                        true,
                    );
                }
                langfuse::lf_debug!("chat_with_retry: returning after persistent retry limit");
                return response;
            }

            if !persistent && attempt as usize > delays.len() {
                warn!(
                    "LLM request failed after {attempt} retries, giving up: {}",
                    response.content.as_deref().unwrap_or("").chars().take(120).collect::<String>().to_lowercase()
                );
                langfuse::lf_debug!("chat_with_retry: max retries ({}) exceeded, breaking out of loop", delays.len());
                if let Some(cb) = on_retry_wait.as_ref() {
                    (cb)(format!(
                        "Model request failed after {attempt} retries, giving up."
                    ))
                    .await;
                }
                break;
            }

            let base_delay = delays[(attempt as usize - 1).min(delays.len() - 1)];
            let mut delay = pick_delay(&response).unwrap_or(base_delay as f64);
            if persistent {
                delay = delay.min(PERSISTENT_MAX_DELAY as f64);
            }

            let counter = if persistent && attempt as usize > delays.len() {
                format!("{attempt}+")
            } else {
                format!("{attempt}/{}", delays.len())
            };
            warn!(
                "LLM transient error (attempt {counter}), retrying in {}s: {}",
                delay.round() as i64,
                response.content.as_deref().unwrap_or("").chars().take(120).collect::<String>().to_lowercase()
            );
            langfuse::lf_debug!("chat_with_retry: sleeping {}s before retry attempt {}", delay, attempt + 1);
            sleep_with_heartbeat(delay, attempt, persistent, on_retry_wait.as_ref()).await;
        }

        langfuse::lf_debug!("chat_with_retry: loop ended, final response is_some={}", last_response.is_some());
        let final_response = last_response.unwrap_or_else(|| LLMResponse::error("LLM request failed"));
        let usage = extract_usage_tuple(&final_response.usage);
        if let Some(g) = gen_handle {
            langfuse::lf_debug!("chat_with_retry: ending generation final error path, usage={:?}", usage);
            let final_output = build_langfuse_output(&final_response);
            g.emit_tools_from_output(&final_output);
            g.end(
                Some(final_output),
                usage,
                Some(final_response.finish_reason.clone()),
                true,
            );
        }
        if let Some(t) = lf.as_ref() {
            t.set_output(Some(build_langfuse_output(&final_response)));
        }
        langfuse::lf_debug!("chat_with_retry: returning final error response");
        final_response
    }

    /// Wrapper for streaming chat with retry policy on transient errors.
    async fn chat_stream_with_retry(
        &self,
        mut req: ChatRequest,
        on_delta: Option<StreamDeltaCallback>,
        on_tool_call_delta: Option<ToolCallDeltaCallback>,
        mode: RetryMode,
        on_retry_wait: Option<RetryWaitCallback>,
        langfuse_trace: Option<&langfuse::TraceHandle>,
    ) -> LLMResponse {
        // --- Langfuse instrumentation (non-blocking) ---
        let lf = langfuse_trace
            .map(|t| std::borrow::Cow::Borrowed(t))
            .or_else(|| {
                get_langfuse_client().map(|c| {
                    std::borrow::Cow::Owned(c.trace("chat_stream_with_retry", None))
                })
            });
        langfuse::lf_debug!("chat_stream_with_retry: langfuse trace available: trace_id={}", lf.as_ref().map(|t| t.id()).unwrap_or("None"));
        log::info!("Langfuse trace handle available: trace_id={}", lf.as_ref().map(|t| t.id()).unwrap_or("None"));
        let full_request = json!({
            "model": &req.model,
            "messages": &req.messages,
            "tools": &req.tools,
            "tool_choice": &req.tool_choice,
            "max_tokens": req.max_tokens,
            "temperature": req.temperature,
            "reasoning_effort": &req.reasoning_effort,
        });
        let model_params = json!({
            "max_tokens": req.max_tokens,
            "temperature": req.temperature,
            "reasoning_effort": req.reasoning_effort,
            "tools": req.tools.as_ref(),
            "tool_choice": req.tool_choice.as_ref(),
        });
        let gen_handle = lf.as_ref().map(|t| {
            langfuse::lf_debug!("chat_stream_with_retry: creating generation under trace_id={} model={:?}",
                t.id(), req.model);
            t.generation(
                "LLM Chat (stream)",
                req.model.clone(),
                Some(full_request),
                Some(model_params),
            )
        });
        langfuse::lf_debug!("chat_stream_with_retry: gen_handle created, is_some={}", gen_handle.is_some());

        let defaults = self.generation();
        if req.max_tokens == 0 {
            req.max_tokens = defaults.max_tokens;
        }
        if !req.temperature.is_finite() {
            req.temperature = defaults.temperature;
        }
        if req.reasoning_effort.is_none() {
            req.reasoning_effort = defaults.reasoning_effort.clone();
        }

        // Enforce role alternation to avoid provider errors
        req.messages = enforce_role_alternation(&req.messages);

        let delays: [u64; 3] = [1, 2, 4];
        let persistent = mode == RetryMode::Persistent;
        let mut attempt: u32 = 0;
        let mut last_error_key: Option<String> = None;
        let mut identical_count: u32 = 0;
        let mut last_response: Option<LLMResponse>;
        let mut images_stripped = false;

        println!("HOP llm: req={:?}", req);
        loop {
            attempt += 1;
            let response = self.chat_stream(req.clone(), on_delta.clone(), on_tool_call_delta.clone()).await;
            if response.finish_reason != "error" {
                langfuse::lf_debug!("chat_stream_with_retry: success on attempt {}, ending generation", attempt);
                let usage = extract_usage_tuple(&response.usage);
                let output = build_langfuse_output(&response);
                if let Some(g) = gen_handle {
                    langfuse::lf_debug!("chat_stream_with_retry: calling gen_handle.end() success path, usage={:?}", usage);
                    g.emit_tools_from_output(&output);
                    g.end(
                        Some(output.clone()),
                        usage,
                        Some(response.finish_reason.clone()),
                        false,
                    );
                }
                if let Some(t) = lf.as_ref() {
                    t.set_output(Some(output));
                }
                langfuse::lf_debug!("chat_stream_with_retry: returning success response");
                return response;
            }

            langfuse::lf_debug!("chat_stream_with_retry: error on attempt {}, finish_reason={}", attempt, response.finish_reason);
            let key = response
                .content
                .as_deref()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            if !key.is_empty() && Some(&key) == last_error_key.as_ref() {
                identical_count = identical_count.saturating_add(1);
            } else {
                last_error_key = if key.is_empty() { None } else { Some(key) };
                identical_count = if last_error_key.is_some() { 1 } else { 0 };
            }
            last_response = Some(response.clone());

            if !is_transient_response(&response) {
                langfuse::lf_debug!("chat_stream_with_retry: non-transient error, images_stripped={}", images_stripped);
                // Non-transient error: try stripping images and retrying once immediately
                if !images_stripped && strip_image_content_inplace(&mut req.messages) {
                    warn!("Non-transient LLM error with image content, retrying without images");
                    images_stripped = true;
                    let result = self.chat_stream(req.clone(), on_delta.clone(), on_tool_call_delta.clone()).await;
                    if result.finish_reason != "error" {
                        strip_image_content_inplace(&mut req.messages);
                    }
                    let usage = extract_usage_tuple(&result.usage);
                    let output = build_langfuse_output(&result);
                    if let Some(g) = gen_handle {
                        langfuse::lf_debug!("chat_stream_with_retry: ending generation after image strip, is_error={}", result.finish_reason == "error");
                        g.emit_tools_from_output(&output);
                        g.end(
                            Some(output.clone()),
                            usage,
                            Some(result.finish_reason.clone()),
                            result.finish_reason == "error",
                        );
                    }
                    if !result.is_error() {
                        if let Some(t) = lf.as_ref() { t.set_output(Some(output)); }
                    }
                    langfuse::lf_debug!("chat_stream_with_retry: returning after image strip");
                    return result;
                }
                let usage = extract_usage_tuple(&response.usage);
                if let Some(g) = gen_handle {
                    langfuse::lf_debug!("chat_stream_with_retry: ending generation non-transient error path");
                    g.end(
                        response.content.clone().map(|c| Value::String(c)),
                        usage,
                        Some(response.finish_reason.clone()),
                        true,
                    );
                }
                langfuse::lf_debug!("chat_stream_with_retry: returning non-transient error response");
                return response;
            }

            if persistent && identical_count >= PERSISTENT_IDENTICAL_ERROR_LIMIT {
                warn!(
                    "Stopping persistent retry after {identical_count} identical transient errors: {}",
                    response.content.as_deref().unwrap_or("").chars().take(120).collect::<String>().to_lowercase()
                );
                langfuse::lf_debug!("chat_stream_with_retry: persistent retry limit reached, identical_count={}", identical_count);
                if let Some(cb) = on_retry_wait.as_ref() {
                    (cb)(format!(
                        "Persistent retry stopped after {identical_count} identical errors."
                    ))
                    .await;
                }
                let usage = extract_usage_tuple(&response.usage);
                if let Some(g) = gen_handle {
                    langfuse::lf_debug!("chat_stream_with_retry: ending generation persistent retry limit path");
                    g.emit_tools_from_output(&build_langfuse_output(&response));
                    g.end(
                        response.content.clone().map(|c| Value::String(c)),
                        usage,
                        Some(response.finish_reason.clone()),
                        true,
                    );
                }
                langfuse::lf_debug!("chat_stream_with_retry: returning after persistent retry limit");
                return response;
            }

            if !persistent && attempt as usize > delays.len() {
                warn!(
                    "LLM stream request failed after {attempt} retries, giving up: {}",
                    response.content.as_deref().unwrap_or("").chars().take(120).collect::<String>().to_lowercase()
                );
                langfuse::lf_debug!("chat_stream_with_retry: max retries ({}) exceeded, breaking out of loop", delays.len());
                if let Some(cb) = on_retry_wait.as_ref() {
                    (cb)(format!(
                        "Model stream request failed after {attempt} retries, giving up."
                    ))
                    .await;
                }
                break;
            }

            let base_delay = delays[(attempt as usize - 1).min(delays.len() - 1)];
            let mut delay = pick_delay(&response).unwrap_or(base_delay as f64);
            if persistent {
                delay = delay.min(PERSISTENT_MAX_DELAY as f64);
            }

            let counter = if persistent && attempt as usize > delays.len() {
                format!("{attempt}+")
            } else {
                format!("{attempt}/{}", delays.len())
            };
            warn!(
                "LLM stream transient error (attempt {counter}), retrying in {}s: {}",
                delay.round() as i64,
                response.content.as_deref().unwrap_or("").chars().take(120).collect::<String>().to_lowercase()
            );
            langfuse::lf_debug!("chat_stream_with_retry: sleeping {}s before retry attempt {}", delay, attempt + 1);
            sleep_with_heartbeat(delay, attempt, persistent, on_retry_wait.as_ref()).await;
        }

        langfuse::lf_debug!("chat_stream_with_retry: loop ended, final response is_some={}", last_response.is_some());
        let final_response = last_response.unwrap_or_else(|| LLMResponse::error("LLM stream request failed"));
        let usage = extract_usage_tuple(&final_response.usage);
        if let Some(g) = gen_handle {
            langfuse::lf_debug!("chat_stream_with_retry: ending generation final error path, usage={:?}", usage);
            let final_output = build_langfuse_output(&final_response);
            g.emit_tools_from_output(&final_output);
            g.end(
                Some(final_output),
                usage,
                Some(final_response.finish_reason.clone()),
                true,
            );
        }
        if let Some(t) = lf.as_ref() {
            t.set_output(Some(build_langfuse_output(&final_response)));
        }
        langfuse::lf_debug!("chat_stream_with_retry: returning final error response");
        final_response
    }
}

async fn sleep_with_heartbeat(
    delay: f64,
    attempt: u32,
    persistent: bool,
    on_retry_wait: Option<&RetryWaitCallback>,
) {
    let mut remaining = delay.max(0.0);
    while remaining > 0.0 {
        if let Some(cb) = on_retry_wait {
            let kind = if persistent {
                "persistent retry"
            } else {
                "retry"
            };
            (cb)(format!(
                "Model request failed, {kind} in {}s (attempt {attempt}).",
                remaining.round().max(1.0) as i64
            ))
            .await;
        }
        let chunk = remaining.min(RETRY_HEARTBEAT_CHUNK as f64);
        sleep(Duration::from_secs_f64(chunk)).await;
        remaining -= chunk;
    }
}

/// Extract (prompt_tokens, completion_tokens, total_tokens) from the usage map.
///
/// Tries common LLM response keys: `prompt_tokens`/`completion_tokens`/
/// `total_tokens` (OpenAI) and `input_tokens`/`output_tokens` (Anthropic).
fn extract_usage_tuple(usage: &HashMap<String, i64>) -> Option<(i64, i64, i64)> {
    if usage.is_empty() {
        return None;
    }
    let prompt = usage
        .get("prompt_tokens")
        .or_else(|| usage.get("input_tokens"))
        .copied()
        .unwrap_or(0);
    let completion = usage
        .get("completion_tokens")
        .or_else(|| usage.get("output_tokens"))
        .copied()
        .unwrap_or(0);
    let total = usage
        .get("total_tokens")
        .copied()
        .unwrap_or(prompt.saturating_add(completion));
    if prompt == 0 && completion == 0 && total == 0 {
        None
    } else {
        Some((prompt, completion, total))
    }
}

/// Build a structured Langfuse output JSON from an LLMResponse containing
/// the full content, tool_calls, and finish_reason.
fn build_langfuse_output(resp: &LLMResponse) -> Value {
    let tool_calls: Vec<Value> = resp.tool_calls.iter().map(|tc| {
        json!({
            "id": tc.id,
            "name": tc.name,
            "arguments": tc.arguments,
        })
    }).collect();

    let mut out = json!({
        "finish_reason": resp.finish_reason,
    });
    if let Some(ref content) = resp.content {
        out["content"] = Value::String(content.clone());
    }
    if !tool_calls.is_empty() {
        out["tool_calls"] = Value::Array(tool_calls);
    }
    out
}
