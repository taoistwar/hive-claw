//! `LLMProvider` trait — async abstraction over chat completion backends.
//!
//! Port of `nanobot.providers.base.LLMProvider`. Heavy helpers (message
//! sanitization, retry policy) are implemented here so concrete backends only
//! need to provide `chat`.

use std::collections::HashMap;
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
#[derive(Debug, Clone)]
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
#[derive(Debug, Clone, Default)]
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

const PERSISTENT_MAX_DELAY: u64 = 60;
const PERSISTENT_IDENTICAL_ERROR_LIMIT: u32 = 10;
const RETRY_HEARTBEAT_CHUNK: u64 = 30;

/// Base trait every LLM backend implements.
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

    /// Streaming chat. Default: fallback to `chat` and emit one delta.
    async fn chat_stream(
        &self,
        req: ChatRequest,
        on_delta: Option<StreamDeltaCallback>,
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
    ) -> LLMResponse {
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
                return response;
            }

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
                // Non-transient error: try stripping images and retrying once
                if !images_stripped && strip_image_content_inplace(&mut req.messages) {
                    warn!("Non-transient LLM error with image content, retrying without images");
                    images_stripped = true;
                    continue;
                }
                return response;
            }

            if persistent && identical_count >= PERSISTENT_IDENTICAL_ERROR_LIMIT {
                warn!(
                    "Stopping persistent retry after {identical_count} identical transient errors"
                );
                if let Some(cb) = on_retry_wait.as_ref() {
                    (cb)(format!(
                        "Persistent retry stopped after {identical_count} identical errors."
                    ))
                    .await;
                }
                return response;
            }

            if !persistent && attempt as usize > delays.len() {
                warn!("LLM request failed after {attempt} retries, giving up");
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

            warn!(
                "LLM transient error (attempt {attempt}), retrying in {}s",
                delay.round() as i64
            );
            sleep_with_heartbeat(delay, attempt, persistent, on_retry_wait.as_ref()).await;
        }

        last_response.unwrap_or_else(|| LLMResponse::error("LLM request failed"))
    }

    /// Wrapper for streaming chat with retry policy on transient errors.
    async fn chat_stream_with_retry(
        &self,
        mut req: ChatRequest,
        on_delta: Option<StreamDeltaCallback>,
        mode: RetryMode,
        on_retry_wait: Option<RetryWaitCallback>,
    ) -> LLMResponse {
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

        loop {
            attempt += 1;
            let response = self.chat_stream(req.clone(), on_delta.clone()).await;
            if response.finish_reason != "error" {
                return response;
            }

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
                // Non-transient error: try stripping images and retrying once
                if !images_stripped && strip_image_content_inplace(&mut req.messages) {
                    warn!("Non-transient LLM error with image content, retrying without images");
                    images_stripped = true;
                    continue;
                }
                return response;
            }

            if persistent && identical_count >= PERSISTENT_IDENTICAL_ERROR_LIMIT {
                warn!(
                    "Stopping persistent retry after {identical_count} identical transient errors"
                );
                if let Some(cb) = on_retry_wait.as_ref() {
                    (cb)(format!(
                        "Persistent retry stopped after {identical_count} identical errors."
                    ))
                    .await;
                }
                return response;
            }

            if !persistent && attempt as usize > delays.len() {
                warn!("LLM stream request failed after {attempt} retries, giving up");
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

            warn!(
                "LLM stream transient error (attempt {attempt}), retrying in {}s",
                delay.round() as i64
            );
            sleep_with_heartbeat(delay, attempt, persistent, on_retry_wait.as_ref()).await;
        }

        last_response.unwrap_or_else(|| LLMResponse::error("LLM stream request failed"))
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
