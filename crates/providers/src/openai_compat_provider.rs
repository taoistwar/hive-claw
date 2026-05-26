//! HTTP-based OpenAI-compatible [`LLMProvider`].
//!
//! Port of `nanobot.providers.openai_compat_provider`. Talks to any
//! `/v1/chat/completions` endpoint (OpenAI, DeepSeek, Gemini, DashScope,
//! Moonshot, SiliconFlow, Zhipu, MiniMax, VolcEngine, etc.).
//!
//! Scope vs Python:
//! * No OpenAI Responses API path (that lives behind a circuit-breaker in
//!   Python; here we always use Chat Completions which is universally
//!   supported).
//! * No native streaming — `chat_stream` falls back to a single-delta call
//!   via the default trait impl.
//! * `json_repair` is replaced by `serde_json` parsing; malformed tool
//!   arguments fall through to `{}`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use log::warn;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::base::{ChatRequest, LLMProvider};
use crate::registry::ProviderSpec;
use crate::base::extract_retry_after_from_text;
use crate::base::{enforce_role_alternation, sanitize_empty_content};
use crate::base::{GenerationSettings, LLMResponse, ToolCallRequest, ToolChoice};

const ALLOWED_MSG_KEYS: &[&str] = &[
    "role",
    "content",
    "tool_calls",
    "tool_call_id",
    "name",
    "reasoning_content",
    "extra_content",
];
const OPENROUTER_HEADER_REFERER: &str = "https://github.com/HKUDS/nanobot";
const OPENROUTER_HEADER_TITLE: &str = "nanobot";
const OPENROUTER_HEADER_CATS: &str = "cli-agent,personal-agent";

const KIMI_THINKING_MODELS: &[&str] = &["kimi-k2.5", "kimi-k2.6", "k2.6-code-preview"];

fn is_kimi_thinking(model: &str) -> bool {
    let name = model.to_ascii_lowercase();
    if KIMI_THINKING_MODELS.contains(&name.as_str()) {
        return true;
    }
    if let Some((_, tail)) = name.rsplit_once('/') {
        if KIMI_THINKING_MODELS.contains(&tail) {
            return true;
        }
    }
    false
}

fn uses_openrouter_attribution(spec: Option<&ProviderSpec>, api_base: &str) -> bool {
    if let Some(s) = spec {
        if s.name == "openrouter" {
            return true;
        }
    }
    api_base.to_ascii_lowercase().contains("openrouter")
}

fn supports_temperature(model: &str, reasoning_effort: Option<&str>) -> bool {
    if let Some(r) = reasoning_effort {
        if r.to_ascii_lowercase() != "none" {
            return false;
        }
    }
    let m = model.to_ascii_lowercase();
    !(m.contains("gpt-5") || m.contains("o1") || m.contains("o3") || m.contains("o4"))
}

/// 9-char alphanumeric tool-call ID (provider-safe; Mistral caps at 9).
fn short_tool_id() -> String {
    let u = uuid::Uuid::new_v4().simple().to_string();
    u.chars().take(9).collect()
}

/// Normalise a tool-call ID into a 9-char alphanumeric form.
fn normalize_tool_call_id(id: &str) -> String {
    if id.len() == 9 && id.chars().all(|c| c.is_ascii_alphanumeric()) {
        return id.to_string();
    }
    let mut hasher = Sha256::new();
    hasher.update(id.as_bytes());
    let hex = format!("{:x}", hasher.finalize());
    hex.chars().take(9).collect()
}

fn normalize_tool_call_arguments(arguments: &Value) -> String {
    match arguments {
        Value::String(s) => {
            let stripped = s.trim();
            if stripped.is_empty() {
                return "{}".into();
            }
            match serde_json::from_str::<Value>(stripped) {
                Ok(Value::Object(m)) => {
                    serde_json::to_string(&Value::Object(m)).unwrap_or_else(|_| "{}".into())
                }
                _ => "{}".into(),
            }
        }
        Value::Object(_) => serde_json::to_string(arguments).unwrap_or_else(|_| "{}".into()),
        _ => "{}".into(),
    }
}

fn sanitize_request_messages(messages: &[Value]) -> Vec<Value> {
    messages
        .iter()
        .map(|m| {
            let Some(obj) = m.as_object() else {
                return m.clone();
            };
            let mut cleaned = Map::new();
            for (k, v) in obj {
                if ALLOWED_MSG_KEYS.contains(&k.as_str()) {
                    cleaned.insert(k.clone(), v.clone());
                }
            }
            let role = cleaned
                .get("role")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if role == "assistant" && !cleaned.contains_key("content") {
                cleaned.insert("content".into(), Value::Null);
            }
            Value::Object(cleaned)
        })
        .collect()
}

/// Strip non-standard keys, normalise tool-call IDs + args, enforce role alt.
fn finalize_messages(messages: &[Value]) -> Vec<Value> {
    let sanitized = sanitize_request_messages(messages);
    let mut id_map: HashMap<String, String> = HashMap::new();
    let mut map_id = |v: &mut Value| {
        if let Value::String(s) = v {
            let canonical = id_map
                .entry(s.clone())
                .or_insert_with(|| normalize_tool_call_id(s))
                .clone();
            *s = canonical;
        }
    };

    let mut out: Vec<Value> = Vec::with_capacity(sanitized.len());
    for mut msg in sanitized {
        if let Some(obj) = msg.as_object_mut() {
            let role = obj.get("role").and_then(|v| v.as_str()).map(String::from);
            if let Some(Value::Array(tcs)) = obj.get_mut("tool_calls").cloned().as_ref() {
                let mut normalized_tcs: Vec<Value> = Vec::with_capacity(tcs.len());
                for tc in tcs {
                    if let Value::Object(tc_obj) = tc {
                        let mut tc_clean = tc_obj.clone();
                        if let Some(id_val) = tc_clean.get_mut("id") {
                            map_id(id_val);
                        }
                        if let Some(Value::Object(fn_obj)) = tc_clean.get_mut("function") {
                            let raw_args = fn_obj.get("arguments").cloned().unwrap_or(Value::Null);
                            let canonical = normalize_tool_call_arguments(&raw_args);
                            fn_obj.insert("arguments".into(), Value::String(canonical));
                        } else {
                            // Ensure function.arguments always present.
                            let mut fn_obj = Map::new();
                            fn_obj.insert("arguments".into(), Value::String("{}".into()));
                            tc_clean.insert("function".into(), Value::Object(fn_obj));
                        }
                        normalized_tcs.push(Value::Object(tc_clean));
                    } else {
                        normalized_tcs.push(tc.clone());
                    }
                }
                obj.insert("tool_calls".into(), Value::Array(normalized_tcs));
                if role.as_deref() == Some("assistant") {
                    // Gateways reject assistant messages that mix non-empty
                    // content with tool_calls.
                    obj.insert("content".into(), Value::Null);
                }
            }
            if let Some(tc_id) = obj.get_mut("tool_call_id") {
                map_id(tc_id);
            }
        }
        out.push(msg);
    }

    enforce_role_alternation(&out)
}

/// Configuration for [`OpenAICompatProvider`].
#[derive(Clone)]
pub struct OpenAICompatConfig {
    pub api_key: Option<String>,
    pub api_base: Option<String>,
    pub default_model: String,
    pub extra_headers: HashMap<String, String>,
    pub extra_body: Option<Map<String, Value>>,
    pub spec: Option<&'static ProviderSpec>,
    pub timeout: Duration,
    pub session_affinity: String,
}

impl OpenAICompatConfig {
    pub fn new(default_model: impl Into<String>) -> Self {
        Self {
            api_key: None,
            api_base: None,
            default_model: default_model.into(),
            extra_headers: HashMap::new(),
            extra_body: None,
            spec: None,
            timeout: Duration::from_secs(120),
            session_affinity: uuid::Uuid::new_v4().simple().to_string(),
        }
    }

    pub fn with_api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into());
        self
    }
    pub fn with_api_base(mut self, base: impl Into<String>) -> Self {
        self.api_base = Some(base.into());
        self
    }
    pub fn with_spec(mut self, spec: &'static ProviderSpec) -> Self {
        self.spec = Some(spec);
        self
    }
    pub fn with_extra_header(mut self, k: impl Into<String>, v: impl Into<String>) -> Self {
        self.extra_headers.insert(k.into(), v.into());
        self
    }
    pub fn with_extra_body(mut self, body: Map<String, Value>) -> Self {
        self.extra_body = Some(body);
        self
    }
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

/// OpenAI-compatible provider using plain HTTP.
pub struct OpenAICompatProvider {
    cfg: OpenAICompatConfig,
    client: reqwest::Client,
    effective_base: String,
    generation: GenerationSettings,
}

impl OpenAICompatProvider {
    pub fn new(cfg: OpenAICompatConfig) -> Self {
        let effective_base = cfg
            .api_base
            .clone()
            .or_else(|| cfg.spec.map(|s| s.default_api_base.to_string()))
            // from env var
            .or_else(|| {
                std::env::var("OPENAI_BASE_URL")
                    .ok()
                    .map(|v| v.trim().to_string())
            })
            .unwrap_or_else(|| "https://api.openai.com/v1".into());
        let client = reqwest::Client::builder()
            .timeout(cfg.timeout)
            .build()
            .expect("reqwest client");
        Self {
            cfg,
            client,
            effective_base,
            generation: GenerationSettings::default(),
        }
    }

    pub fn with_generation(mut self, generation: GenerationSettings) -> Self {
        self.generation = generation;
        self
    }

    fn endpoint(&self) -> String {
        let base = self.effective_base.trim_end_matches('/');
        format!("{base}/chat/completions")
    }

    fn resolve_model(&self, req_model: Option<&str>) -> String {
        let mut m = req_model
            .map(String::from)
            .unwrap_or_else(|| self.cfg.default_model.clone());
        if let Some(spec) = self.cfg.spec {
            if spec.strip_model_prefix {
                if let Some((_, tail)) = m.rsplit_once('/') {
                    m = tail.to_string();
                }
            }
        }
        m
    }

    fn tool_choice_to_value(tc: Option<&ToolChoice>) -> Value {
        match tc {
            Some(ToolChoice::Required) => Value::String("required".into()),
            Some(ToolChoice::None) => Value::String("none".into()),
            Some(ToolChoice::Specific(v)) => v.clone(),
            _ => Value::String("auto".into()),
        }
    }

    fn build_body(&self, req: &ChatRequest) -> Value {
        let model = self.resolve_model(req.model.as_deref());
        let spec = self.cfg.spec;

        let finalized = finalize_messages(&sanitize_empty_content(&req.messages));

        let mut body = Map::new();
        body.insert("model".into(), Value::String(model.clone()));
        body.insert("messages".into(), Value::Array(finalized));

        let reasoning_effort = req.reasoning_effort.as_deref();
        if supports_temperature(&model, reasoning_effort) {
            body.insert("temperature".into(), json!(req.temperature as f64));
        }

        let max_tokens = req.max_tokens.max(1);
        if spec
            .map(|s| s.supports_max_completion_tokens)
            .unwrap_or(false)
        {
            body.insert("max_completion_tokens".into(), json!(max_tokens));
        } else {
            body.insert("max_tokens".into(), json!(max_tokens));
        }

        if let Some(spec) = spec {
            let model_lower = model.to_ascii_lowercase();
            for (pattern, overrides) in spec.model_overrides {
                if model_lower.contains(pattern) {
                    for (k, raw) in *overrides {
                        let parsed: Value = serde_json::from_str(raw)
                            .unwrap_or_else(|_| Value::String(raw.to_string()));
                        body.insert((*k).to_string(), parsed);
                    }
                    break;
                }
            }
        }

        // reasoning_effort wire form + DashScope quirk.
        let semantic_effort = reasoning_effort.map(|s| s.to_ascii_lowercase()).map(|s| {
            if s == "minimum" {
                "minimal".to_string()
            } else {
                s
            }
        });
        let wire_effort = match (
            reasoning_effort,
            spec.map(|s| s.name),
            semantic_effort.as_deref(),
        ) {
            (Some(_), Some("dashscope"), Some("minimal")) => Some("minimum".to_string()),
            (Some(v), _, _) => Some(v.to_string()),
            _ => None,
        };
        if let Some(we) = wire_effort {
            body.insert("reasoning_effort".into(), Value::String(we));
        }

        // Provider-specific thinking parameters — only when caller
        // explicitly passed reasoning_effort.
        if reasoning_effort.is_some() {
            let thinking_enabled = semantic_effort.as_deref() != Some("minimal");
            let extra: Option<Value> = spec.and_then(|spec| match spec.name {
                "dashscope" => Some(json!({"enable_thinking": thinking_enabled})),
                "minimax" => Some(json!({"reasoning_split": thinking_enabled})),
                "volcengine" | "volcengine_coding_plan" | "byteplus" | "byteplus_coding_plan" => {
                    Some(json!({
                        "thinking": {"type": if thinking_enabled {"enabled"} else {"disabled"}}
                    }))
                }
                _ => None,
            });
            if let Some(extra) = extra {
                merge_into_extra_body(&mut body, extra);
            }
            if is_kimi_thinking(&model) {
                merge_into_extra_body(
                    &mut body,
                    json!({
                        "thinking": {"type": if thinking_enabled {"enabled"} else {"disabled"}}
                    }),
                );
            }
        }

        if let Some(tools) = &req.tools {
            if !tools.is_empty() {
                body.insert("tools".into(), Value::Array(tools.clone()));
                body.insert(
                    "tool_choice".into(),
                    Self::tool_choice_to_value(req.tool_choice.as_ref()),
                );
            }
        }

        // Merge user-configured extra_body last so it can override defaults
        if let Some(extra) = &self.cfg.extra_body {
            for (k, v) in extra {
                body.insert(k.clone(), v.clone());
            }
        }

        Value::Object(body)
    }

    async fn send(&self, body: &Value) -> Result<reqwest::Response, reqwest::Error> {
        let mut request = self
            .client
            .post(self.endpoint())
            .header("Content-Type", "application/json")
            .header("x-session-affinity", self.cfg.session_affinity.as_str());

        if let Some(key) = self.cfg.api_key.as_deref() {
            request = request.header("Authorization", format!("Bearer {key}"));
        }
        if uses_openrouter_attribution(self.cfg.spec, &self.effective_base) {
            request = request
                .header("HTTP-Referer", OPENROUTER_HEADER_REFERER)
                .header("X-OpenRouter-Title", OPENROUTER_HEADER_TITLE)
                .header("X-OpenRouter-Categories", OPENROUTER_HEADER_CATS);
        }
        for (k, v) in &self.cfg.extra_headers {
            request = request.header(k, v);
        }
        request.json(body).send().await
    }
}

fn merge_into_extra_body(body: &mut Map<String, Value>, extra: Value) {
    let entry = body
        .entry("extra_body".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if let (Value::Object(e), Value::Object(src)) = (entry, &extra) {
        for (k, v) in src {
            e.insert(k.clone(), v.clone());
        }
    }
}

// ---------------------------------------------------------------------------
// Response parsing
// ---------------------------------------------------------------------------

fn extract_text(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        Value::Array(items) => {
            let mut parts = Vec::new();
            for item in items {
                if let Some(t) = item.get("text").and_then(|v| v.as_str()) {
                    parts.push(t.to_string());
                } else if let Value::String(s) = item {
                    parts.push(s.clone());
                }
            }
            if parts.is_empty() {
                None
            } else {
                Some(parts.join(""))
            }
        }
        other => Some(other.to_string()),
    }
}

fn extract_usage(response: &Value) -> HashMap<String, i64> {
    let mut out = HashMap::new();
    let usage = response.get("usage");
    let Some(usage) = usage.and_then(|v| v.as_object()) else {
        return out;
    };
    let as_i64 = |k: &str| -> i64 { usage.get(k).and_then(|v| v.as_i64()).unwrap_or(0) };
    out.insert("prompt_tokens".into(), as_i64("prompt_tokens"));
    out.insert("completion_tokens".into(), as_i64("completion_tokens"));
    out.insert("total_tokens".into(), as_i64("total_tokens"));

    for path in [
        &["prompt_tokens_details", "cached_tokens"] as &[&str],
        &["cached_tokens"],
        &["prompt_cache_hit_tokens"],
    ] {
        let usage_value = Value::Object(usage.clone());
        let mut current: Option<&Value> = Some(&usage_value);
        for seg in path.iter() {
            current = current.and_then(|v| v.get(*seg));
        }
        if let Some(v) = current.and_then(|v| v.as_i64()) {
            if v > 0 {
                out.insert("cached_tokens".into(), v);
                break;
            }
        }
    }
    out
}

fn parse_tool_calls(raw: &[Value]) -> Vec<ToolCallRequest> {
    let mut out = Vec::with_capacity(raw.len());
    for tc in raw {
        let fn_obj = tc.get("function").and_then(|v| v.as_object());
        let name = fn_obj
            .and_then(|m| m.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let args = fn_obj
            .and_then(|m| m.get("arguments"))
            .cloned()
            .unwrap_or(Value::Null);
        let parsed: Map<String, Value> = match args {
            Value::String(ref s) if !s.trim().is_empty() => {
                match serde_json::from_str::<Value>(s) {
                    Ok(Value::Object(m)) => m,
                    _ => Map::new(),
                }
            }
            Value::Object(m) => m,
            _ => Map::new(),
        };
        out.push(ToolCallRequest {
            id: short_tool_id(),
            name,
            arguments: parsed,
            extra_content: None,
            provider_specific_fields: None,
            function_provider_specific_fields: None,
        });
    }
    out
}

pub(crate) fn parse_response(body: &Value) -> LLMResponse {
    let choices = body.get("choices").and_then(|v| v.as_array());
    let Some(choices) = choices else {
        let content = body
            .get("content")
            .or_else(|| body.get("output_text"))
            .and_then(extract_text);
        let reasoning = body.get("reasoning_content").and_then(extract_text);
        if let Some(c) = content {
            return LLMResponse {
                content: Some(c),
                reasoning_content: reasoning,
                finish_reason: body
                    .get("finish_reason")
                    .and_then(|v| v.as_str())
                    .unwrap_or("stop")
                    .to_string(),
                usage: extract_usage(body),
                ..Default::default()
            };
        }
        return LLMResponse::error("Error: API returned empty choices.");
    };

    if choices.is_empty() {
        return LLMResponse::error("Error: API returned empty choices.");
    }

    let mut content: Option<String> = None;
    let mut reasoning_content: Option<String> = None;
    let mut finish_reason = "stop".to_string();
    let mut raw_tool_calls: Vec<Value> = Vec::new();

    for (i, ch) in choices.iter().enumerate() {
        let m = ch.get("message").cloned().unwrap_or(Value::Null);
        let fr = ch
            .get("finish_reason")
            .and_then(|v| v.as_str())
            .unwrap_or("stop")
            .to_string();
        if i == 0 {
            finish_reason = fr.clone();
        }
        if let Some(tcs) = m.get("tool_calls").and_then(|v| v.as_array()) {
            if !tcs.is_empty() {
                raw_tool_calls.extend(tcs.clone());
                if fr == "tool_calls" || fr == "stop" {
                    finish_reason = fr;
                }
            }
        }
        if content.is_none() {
            content = m.get("content").and_then(extract_text);
            if content.is_none() {
                content = m.get("reasoning").and_then(extract_text);
            }
        }
        if reasoning_content.is_none() {
            reasoning_content = m
                .get("reasoning_content")
                .and_then(extract_text)
                .or_else(|| m.get("reasoning").and_then(extract_text));
        }
    }

    let tool_calls = parse_tool_calls(&raw_tool_calls);

    LLMResponse {
        content,
        tool_calls,
        finish_reason,
        usage: extract_usage(body),
        reasoning_content,
        ..Default::default()
    }
}

fn parse_error_response(
    status: reqwest::StatusCode,
    headers: &reqwest::header::HeaderMap,
    payload: &str,
) -> LLMResponse {
    let status_i32 = status.as_u16() as i32;
    let msg = if payload.trim().is_empty() {
        format!("Error calling LLM: HTTP {status_i32}")
    } else {
        let preview: String = payload.trim().chars().take(500).collect();
        format!("Error: {preview}")
    };

    let parsed: Option<Value> = serde_json::from_str(payload).ok();
    let (error_type, error_code) = parsed
        .as_ref()
        .map(extract_error_type_code)
        .unwrap_or((None, None));

    let retry_after =
        extract_retry_after_header(headers).or_else(|| extract_retry_after_from_text(Some(&msg)));

    LLMResponse {
        content: Some(msg),
        finish_reason: "error".into(),
        retry_after,
        error_status_code: Some(status_i32),
        error_type,
        error_code,
        error_retry_after_s: retry_after,
        error_should_retry: header_should_retry(headers),
        ..Default::default()
    }
}

fn header_should_retry(headers: &reqwest::header::HeaderMap) -> Option<bool> {
    let raw = headers.get("x-should-retry")?.to_str().ok()?;
    match raw.trim().to_ascii_lowercase().as_str() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

fn extract_retry_after_header(headers: &reqwest::header::HeaderMap) -> Option<f64> {
    if let Some(v) = headers
        .get("retry-after-ms")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<f64>().ok())
    {
        let secs = v / 1000.0;
        if secs > 0.0 {
            return Some(secs);
        }
    }
    let raw = headers
        .get("retry-after")?
        .to_str()
        .ok()?
        .trim()
        .to_string();
    if raw.is_empty() {
        return None;
    }
    if let Ok(n) = raw.parse::<f64>() {
        return Some(n.max(0.1));
    }
    // HTTP-date form is rare in practice; fall through.
    None
}

fn extract_error_type_code(payload: &Value) -> (Option<String>, Option<String>) {
    let mut error_type = payload
        .get("type")
        .and_then(|v| v.as_str())
        .map(|s| s.to_ascii_lowercase());
    let mut error_code = payload
        .get("code")
        .and_then(|v| v.as_str())
        .map(|s| s.to_ascii_lowercase());
    if let Some(err_obj) = payload.get("error").and_then(|v| v.as_object()) {
        if let Some(t) = err_obj.get("type").and_then(|v| v.as_str()) {
            error_type = Some(t.to_ascii_lowercase());
        }
        if let Some(c) = err_obj.get("code").and_then(|v| v.as_str()) {
            error_code = Some(c.to_ascii_lowercase());
        }
    }
    (error_type, error_code)
}

#[async_trait]
impl LLMProvider for OpenAICompatProvider {
    fn default_model(&self) -> String {
        self.cfg.default_model.clone()
    }

    fn generation(&self) -> GenerationSettings {
        self.generation.clone()
    }

    async fn chat(&self, req: ChatRequest) -> LLMResponse {
        let body = self.build_body(&req);
        let resp = match self.send(&body).await {
            Ok(r) => r,
            Err(e) => {
                let msg = format!("Error calling LLM: {e}");
                let kind = if e.is_timeout() {
                    Some("timeout".to_string())
                } else if e.is_connect() {
                    Some("connection".to_string())
                } else {
                    None
                };
                return LLMResponse {
                    content: Some(msg.clone()),
                    finish_reason: "error".into(),
                    error_kind: kind,
                    ..Default::default()
                };
            }
        };
        let status = resp.status();
        let headers = resp.headers().clone();
        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_default();
            warn!("LLM error {status}: {body_text}");
            return parse_error_response(status, &headers, &body_text);
        }
        let body_text = match resp.text().await {
            Ok(t) => t,
            Err(e) => return LLMResponse::error(format!("Error reading LLM body: {e}")),
        };
        let Ok(value) = serde_json::from_str::<Value>(&body_text) else {
            return LLMResponse::error(format!("Error: malformed JSON from provider: {body_text}"));
        };
        parse_response(&value)
    }
}

/// Convenience factory from a config map. `api_key`, `api_base`, and
/// `default_model` are honoured; everything else defaults.
pub fn from_env(
    default_model: impl Into<String>,
    spec: Option<&'static ProviderSpec>,
) -> Arc<dyn LLMProvider> {
    let mut cfg = OpenAICompatConfig::new(default_model);
    if let Some(spec) = spec {
        cfg = cfg.with_spec(spec);
        if !spec.env_key.is_empty() {
            if let Ok(v) = std::env::var(spec.env_key) {
                cfg = cfg.with_api_key(v);
            }
        }
    }
    Arc::new(OpenAICompatProvider::new(cfg))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::find_by_name;

    #[test]
    fn short_id_is_9_chars() {
        let id = short_tool_id();
        assert_eq!(id.len(), 9);
        assert!(id.chars().all(|c| c.is_ascii_alphanumeric()));
    }

    #[test]
    fn normalize_tool_call_id_passthrough_when_valid() {
        assert_eq!(normalize_tool_call_id("abc123XYZ"), "abc123XYZ");
    }

    #[test]
    fn normalize_tool_call_id_hashes_long_ids() {
        let out = normalize_tool_call_id("call_this_is_a_very_long_id_1234");
        assert_eq!(out.len(), 9);
        assert!(out.chars().all(|c| c.is_ascii_alphanumeric()));
    }

    #[test]
    fn normalize_tool_call_arguments_string_object() {
        let v = Value::String(r#"{"x":1}"#.into());
        assert_eq!(normalize_tool_call_arguments(&v), r#"{"x":1}"#);
    }

    #[test]
    fn normalize_tool_call_arguments_empty_and_malformed() {
        assert_eq!(
            normalize_tool_call_arguments(&Value::String("".into())),
            "{}"
        );
        assert_eq!(
            normalize_tool_call_arguments(&Value::String("not json".into())),
            "{}"
        );
    }

    #[test]
    fn supports_temperature_rejects_reasoning_models() {
        assert!(supports_temperature("gpt-4o-mini", None));
        assert!(!supports_temperature("gpt-5-chat", None));
        assert!(!supports_temperature("o3-mini", None));
        assert!(!supports_temperature("gpt-4o", Some("high")));
    }

    #[test]
    fn openai_build_body_minimal() {
        let spec = find_by_name("openai").unwrap();
        let cfg = OpenAICompatConfig::new("gpt-4o").with_spec(spec);
        let p = OpenAICompatProvider::new(cfg);
        let req = ChatRequest {
            messages: vec![json!({"role":"user","content":"hi"})],
            tools: None,
            model: None,
            max_tokens: 1024,
            temperature: 0.7,
            reasoning_effort: None,
            tool_choice: None,
        };
        let body = p.build_body(&req);
        assert_eq!(body["model"], "gpt-4o");
        // OpenAI spec uses max_completion_tokens.
        assert_eq!(body["max_completion_tokens"], 1024);
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["messages"][0]["content"], "hi");
    }

    #[test]
    fn dashscope_reasoning_minimum_alias() {
        let spec = find_by_name("dashscope").unwrap();
        let cfg = OpenAICompatConfig::new("qwen-max").with_spec(spec);
        let p = OpenAICompatProvider::new(cfg);
        let req = ChatRequest {
            messages: vec![json!({"role":"user","content":"hi"})],
            tools: None,
            model: None,
            max_tokens: 512,
            temperature: 0.7,
            reasoning_effort: Some("minimal".into()),
            tool_choice: None,
        };
        let body = p.build_body(&req);
        assert_eq!(body["reasoning_effort"], "minimum");
        // enable_thinking=false when effort is minimal.
        assert_eq!(body["extra_body"]["enable_thinking"], false);
    }

    #[test]
    fn moonshot_kimi_override_applied() {
        let spec = find_by_name("moonshot").unwrap();
        let cfg = OpenAICompatConfig::new("kimi-k2.5").with_spec(spec);
        let p = OpenAICompatProvider::new(cfg);
        let req = ChatRequest {
            messages: vec![json!({"role":"user","content":"hi"})],
            tools: None,
            model: None,
            max_tokens: 2048,
            temperature: 0.3,
            reasoning_effort: None,
            tool_choice: None,
        };
        let body = p.build_body(&req);
        // Override sets temperature to 1.0.
        assert_eq!(body["temperature"], 1.0);
    }

    #[test]
    fn parse_response_chat_completion_basic() {
        let body = json!({
            "choices": [{
                "finish_reason": "stop",
                "message": {"role":"assistant","content":"hi there"}
            }],
            "usage": {"prompt_tokens":1,"completion_tokens":2,"total_tokens":3}
        });
        let out = parse_response(&body);
        assert_eq!(out.content.as_deref(), Some("hi there"));
        assert_eq!(out.finish_reason, "stop");
        assert_eq!(out.usage.get("total_tokens").copied(), Some(3));
    }

    #[test]
    fn parse_response_tool_calls() {
        let body = json!({
            "choices": [{
                "finish_reason": "tool_calls",
                "message": {
                    "role":"assistant",
                    "content": null,
                    "tool_calls":[{
                        "id":"call_1",
                        "type":"function",
                        "function": {"name":"read_file","arguments":"{\"path\":\"/a\"}"}
                    }]
                }
            }]
        });
        let out = parse_response(&body);
        assert_eq!(out.finish_reason, "tool_calls");
        assert_eq!(out.tool_calls.len(), 1);
        assert_eq!(out.tool_calls[0].name, "read_file");
        assert_eq!(
            out.tool_calls[0]
                .arguments
                .get("path")
                .and_then(|v| v.as_str()),
            Some("/a")
        );
    }

    #[test]
    fn parse_response_empty_choices_is_error() {
        let body = json!({"choices": []});
        let out = parse_response(&body);
        assert!(out.is_error());
    }

    #[test]
    fn finalize_messages_sanitises_tool_call_ids() {
        let msgs = vec![
            json!({"role":"user","content":"please"}),
            json!({
                "role":"assistant",
                "content":"done",
                "tool_calls":[{
                    "id":"call_something_long_here_xyz_123",
                    "type":"function",
                    "function": {"name":"x","arguments":"{\"a\":1}"}
                }]
            }),
            json!({
                "role":"tool",
                "tool_call_id":"call_something_long_here_xyz_123",
                "content":"42"
            }),
        ];
        let out = finalize_messages(&msgs);
        // user, assistant(tool_calls w/ null content), tool
        assert_eq!(out.len(), 3);
        let assistant = &out[1];
        assert!(assistant["content"].is_null());
        let tc = &assistant["tool_calls"][0];
        let id = tc["id"].as_str().unwrap();
        assert_eq!(id.len(), 9);
        assert!(tc["function"]["arguments"].is_string());
        // tool_call_id is mapped to the same 9-char id.
        assert_eq!(out[2]["tool_call_id"], tc["id"]);
    }
}
