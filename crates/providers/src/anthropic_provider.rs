//! HTTP-based Anthropic provider (`/v1/messages`).
//!
//! Port of `nanobot.providers.anthropic_provider`. Talks directly to
//! Anthropic's Messages API, converting OpenAI-style chat messages into
//! Anthropic's `content` blocks.
//!
//! Scope vs Python:
//! * No native streaming — fallback via default trait impl.
//! * Prompt-caching `cache_control` markers are applied when *spec*
//!   declares `supports_prompt_caching = true`.
//! * Extended thinking / `reasoning_effort` supported with the same
//!   `low/medium/high/adaptive` semantics.

use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use log::warn;
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{json, Map, Value};

use crate::base::{ChatRequest, LLMProvider};
use crate::base::extract_retry_after_from_text;
use crate::base::sanitize_empty_content;
use crate::base::{GenerationSettings, LLMResponse, ToolCallRequest, ToolChoice};

const DEFAULT_API_VERSION: &str = "2023-06-01";

static DATA_URL_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^data:(image/\w+);base64,(.+)$").unwrap());

fn gen_tool_id() -> String {
    let u = uuid::Uuid::new_v4().simple().to_string();
    format!("toolu_{}", u.chars().take(22).collect::<String>())
}

#[derive(Clone)]
pub struct AnthropicConfig {
    pub api_key: Option<String>,
    pub api_base: Option<String>,
    pub default_model: String,
    pub extra_headers: HashMap<String, String>,
    pub api_version: String,
    pub timeout: Duration,
    pub supports_caching: bool,
}

impl AnthropicConfig {
    pub fn new(default_model: impl Into<String>) -> Self {
        Self {
            api_key: None,
            api_base: None,
            default_model: default_model.into(),
            extra_headers: HashMap::new(),
            api_version: DEFAULT_API_VERSION.into(),
            timeout: Duration::from_secs(120),
            supports_caching: true,
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
    pub fn with_extra_header(mut self, k: impl Into<String>, v: impl Into<String>) -> Self {
        self.extra_headers.insert(k.into(), v.into());
        self
    }
    pub fn with_api_version(mut self, v: impl Into<String>) -> Self {
        self.api_version = v.into();
        self
    }
    pub fn with_caching(mut self, on: bool) -> Self {
        self.supports_caching = on;
        self
    }
}

pub struct AnthropicProvider {
    cfg: AnthropicConfig,
    client: reqwest::Client,
    effective_base: String,
    generation: GenerationSettings,
}

impl AnthropicProvider {
    pub fn new(cfg: AnthropicConfig) -> Self {
        let effective_base = cfg
            .api_base
            .clone()
            .unwrap_or_else(|| "https://api.anthropic.com".into());
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
        format!("{base}/messages")
    }

    fn strip_prefix(model: &str) -> String {
        if let Some(tail) = model.strip_prefix("anthropic/") {
            tail.to_string()
        } else {
            model.to_string()
        }
    }

    /// Convert OpenAI-style messages into (system, anthropic_messages).
    fn convert_messages(messages: &[Value]) -> (Value, Vec<Value>) {
        let mut system: Value = Value::String(String::new());
        let mut raw: Vec<Value> = Vec::new();

        for msg in messages {
            let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("");
            let content = msg.get("content").cloned().unwrap_or(Value::Null);

            match role {
                "system" => {
                    system = match &content {
                        Value::String(_) | Value::Array(_) => content.clone(),
                        Value::Null => Value::String(String::new()),
                        other => Value::String(other.to_string()),
                    };
                }
                "tool" => {
                    let block = tool_result_block(msg);
                    if let Some(last) = raw.last_mut() {
                        if last.get("role").and_then(|v| v.as_str()) == Some("user") {
                            match last.get_mut("content").unwrap() {
                                Value::Array(arr) => arr.push(block),
                                other => {
                                    let text = other
                                        .as_str()
                                        .unwrap_or("")
                                        .to_string();
                                    *other = Value::Array(vec![
                                        json!({"type":"text","text":text}),
                                        block,
                                    ]);
                                }
                            }
                            continue;
                        }
                    }
                    raw.push(json!({"role":"user","content":[block]}));
                }
                "assistant" => {
                    let blocks = assistant_blocks(msg);
                    raw.push(json!({"role":"assistant","content":blocks}));
                }
                "user" => {
                    raw.push(json!({"role":"user","content": convert_user_content(&content)}));
                }
                _ => {}
            }
        }

        (system, merge_consecutive(raw))
    }

    fn convert_tools(tools: Option<&Vec<Value>>) -> Option<Vec<Value>> {
        let tools = tools?;
        if tools.is_empty() {
            return None;
        }
        let mut out = Vec::with_capacity(tools.len());
        for t in tools {
            let func = t.get("function").unwrap_or(t);
            let name = func
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let schema = func
                .get("parameters")
                .cloned()
                .unwrap_or_else(|| json!({"type":"object","properties":{}}));
            let mut entry = json!({"name":name,"input_schema":schema});
            if let Some(d) = func.get("description").and_then(|v| v.as_str()) {
                entry["description"] = Value::String(d.into());
            }
            if let Some(cc) = t.get("cache_control").cloned() {
                entry["cache_control"] = cc;
            }
            out.push(entry);
        }
        Some(out)
    }

    fn convert_tool_choice(tc: Option<&ToolChoice>, thinking_enabled: bool) -> Option<Value> {
        if thinking_enabled {
            return Some(json!({"type":"auto"}));
        }
        match tc {
            None | Some(ToolChoice::Auto) => Some(json!({"type":"auto"})),
            Some(ToolChoice::Required) => Some(json!({"type":"any"})),
            Some(ToolChoice::None) => None,
            Some(ToolChoice::Specific(v)) => {
                let name = v
                    .get("function")
                    .and_then(|f| f.get("name"))
                    .and_then(|n| n.as_str())
                    .unwrap_or("");
                if name.is_empty() {
                    Some(json!({"type":"auto"}))
                } else {
                    Some(json!({"type":"tool","name":name}))
                }
            }
        }
    }

    fn apply_cache_control(
        system: Value,
        messages: Vec<Value>,
        tools: Option<Vec<Value>>,
    ) -> (Value, Vec<Value>, Option<Vec<Value>>) {
        let marker = json!({"type":"ephemeral"});
        let system = match system {
            Value::String(ref s) if !s.is_empty() => {
                json!([{"type":"text","text":s,"cache_control":marker}])
            }
            Value::Array(mut arr) if !arr.is_empty() => {
                if let Some(Value::Object(m)) = arr.last_mut() {
                    m.insert("cache_control".into(), marker.clone());
                }
                Value::Array(arr)
            }
            other => other,
        };

        let mut msgs = messages;
        if msgs.len() >= 3 {
            let idx = msgs.len() - 2;
            if let Some(m) = msgs.get_mut(idx).and_then(|v| v.as_object_mut()) {
                match m.get("content").cloned() {
                    Some(Value::String(s)) => {
                        m.insert(
                            "content".into(),
                            json!([{"type":"text","text":s,"cache_control":marker}]),
                        );
                    }
                    Some(Value::Array(mut arr)) if !arr.is_empty() => {
                        if let Some(Value::Object(mm)) = arr.last_mut() {
                            mm.insert("cache_control".into(), marker.clone());
                        }
                        m.insert("content".into(), Value::Array(arr));
                    }
                    _ => {}
                }
            }
        }

        let tools = tools.map(|mut t| {
            let indices = tool_cache_marker_indices(&t);
            for idx in indices {
                if let Some(entry) = t.get_mut(idx).and_then(|v| v.as_object_mut()) {
                    entry.insert("cache_control".into(), marker.clone());
                }
            }
            t
        });

        (system, msgs, tools)
    }

    fn build_body(&self, req: &ChatRequest) -> Value {
        let model = Self::strip_prefix(
            req.model
                .as_deref()
                .unwrap_or(&self.cfg.default_model),
        );

        let sanitized = sanitize_empty_content(&req.messages);
        let (mut system, mut msgs) = Self::convert_messages(&sanitized);
        let mut tools = Self::convert_tools(req.tools.as_ref());

        if self.cfg.supports_caching {
            let (s, m, t) = Self::apply_cache_control(system, msgs, tools);
            system = s;
            msgs = m;
            tools = t;
        }

        let max_tokens = req.max_tokens.max(1);
        let thinking_enabled = req.reasoning_effort.is_some();

        let mut body = Map::new();
        body.insert("model".into(), Value::String(model));
        body.insert("messages".into(), Value::Array(msgs));
        body.insert("max_tokens".into(), json!(max_tokens));

        match &system {
            Value::String(s) if s.is_empty() => {}
            Value::Null => {}
            v => {
                body.insert("system".into(), v.clone());
            }
        }

        let reasoning = req.reasoning_effort.as_deref().map(str::to_ascii_lowercase);
        if reasoning.as_deref() == Some("adaptive") {
            body.insert("thinking".into(), json!({"type":"adaptive"}));
            body.insert("temperature".into(), json!(1.0));
        } else if thinking_enabled {
            let budget = match reasoning.as_deref() {
                Some("low") => 1024u32,
                Some("medium") => 4096,
                Some("high") => std::cmp::max(8192, max_tokens),
                _ => 4096,
            };
            body.insert(
                "thinking".into(),
                json!({"type":"enabled","budget_tokens":budget}),
            );
            body.insert(
                "max_tokens".into(),
                json!(std::cmp::max(max_tokens, budget + 4096)),
            );
            body.insert("temperature".into(), json!(1.0));
        } else {
            body.insert("temperature".into(), json!(req.temperature as f64));
        }

        if let Some(tools) = tools {
            if !tools.is_empty() {
                body.insert("tools".into(), Value::Array(tools));
                if let Some(tc) = Self::convert_tool_choice(req.tool_choice.as_ref(), thinking_enabled)
                {
                    body.insert("tool_choice".into(), tc);
                }
            }
        }

        Value::Object(body)
    }

    async fn send(&self, body: &Value) -> Result<reqwest::Response, reqwest::Error> {
        let mut request = self
            .client
            .post(self.endpoint())
            .header("Content-Type", "application/json")
            .header("anthropic-version", self.cfg.api_version.as_str());
        if let Some(key) = self.cfg.api_key.as_deref() {
            request = request.header("x-api-key", key);
        }
        for (k, v) in &self.cfg.extra_headers {
            request = request.header(k, v);
        }
        request.json(body).send().await
    }
}

fn tool_result_block(msg: &Value) -> Value {
    let tool_use_id = msg
        .get("tool_call_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let content = msg.get("content").cloned().unwrap_or(Value::Null);
    let content_value = match content {
        Value::String(s) => Value::String(s),
        Value::Null => Value::String(String::new()),
        other => match convert_user_content(&other) {
            Value::String(s) => Value::String(s),
            v => v,
        },
    };
    json!({"type":"tool_result","tool_use_id":tool_use_id,"content":content_value})
}

fn assistant_blocks(msg: &Value) -> Vec<Value> {
    let mut blocks: Vec<Value> = Vec::new();
    // Preserve extended thinking blocks first so Anthropic keeps the signature chain intact.
    if let Some(tbs) = msg.get("thinking_blocks").and_then(|v| v.as_array()) {
        for tb in tbs {
            if tb.get("type").and_then(|v| v.as_str()) == Some("thinking") {
                blocks.push(json!({
                    "type":"thinking",
                    "thinking": tb.get("thinking").and_then(|v| v.as_str()).unwrap_or(""),
                    "signature": tb.get("signature").and_then(|v| v.as_str()).unwrap_or(""),
                }));
            }
        }
    }

    match msg.get("content") {
        Some(Value::String(s)) if !s.is_empty() => {
            blocks.push(json!({"type":"text","text":s}));
        }
        Some(Value::Array(arr)) => {
            for item in arr {
                if item.is_object() {
                    blocks.push(item.clone());
                } else if let Value::String(s) = item {
                    blocks.push(json!({"type":"text","text":s}));
                }
            }
        }
        _ => {}
    }

    if let Some(tcs) = msg.get("tool_calls").and_then(|v| v.as_array()) {
        for tc in tcs {
            let func = tc.get("function").cloned().unwrap_or(json!({}));
            let args_raw = func.get("arguments").cloned().unwrap_or(json!({}));
            let args: Value = match args_raw {
                Value::String(s) => {
                    serde_json::from_str(&s).unwrap_or_else(|_| json!({}))
                }
                Value::Object(_) => args_raw,
                _ => json!({}),
            };
            let id = tc
                .get("id")
                .and_then(|v| v.as_str())
                .map(String::from)
                .unwrap_or_else(gen_tool_id);
            let name = func.get("name").and_then(|v| v.as_str()).unwrap_or("");
            blocks.push(json!({
                "type":"tool_use",
                "id": id,
                "name": name,
                "input": args,
            }));
        }
    }

    if blocks.is_empty() {
        blocks.push(json!({"type":"text","text":""}));
    }
    blocks
}

fn convert_user_content(content: &Value) -> Value {
    match content {
        Value::Null => Value::String("(empty)".into()),
        Value::String(s) if s.is_empty() => Value::String("(empty)".into()),
        Value::String(s) => Value::String(s.clone()),
        Value::Array(items) => {
            let mut out: Vec<Value> = Vec::with_capacity(items.len());
            for item in items {
                if let Some(obj) = item.as_object() {
                    if obj.get("type").and_then(|v| v.as_str()) == Some("image_url") {
                        if let Some(b) = convert_image_block(item) {
                            out.push(b);
                        }
                        continue;
                    }
                    out.push(item.clone());
                } else {
                    out.push(json!({"type":"text","text": item.to_string()}));
                }
            }
            if out.is_empty() {
                Value::String("(empty)".into())
            } else {
                Value::Array(out)
            }
        }
        other => Value::String(other.to_string()),
    }
}

fn convert_image_block(block: &Value) -> Option<Value> {
    let url = block
        .get("image_url")
        .and_then(|v| v.get("url"))
        .and_then(|v| v.as_str())?;
    if url.is_empty() {
        return None;
    }
    if let Some(caps) = DATA_URL_RE.captures(url) {
        return Some(json!({
            "type":"image",
            "source":{"type":"base64","media_type":&caps[1],"data":&caps[2]},
        }));
    }
    Some(json!({
        "type":"image",
        "source":{"type":"url","url":url},
    }))
}

fn has_tool_use(msg: &Value) -> bool {
    let Some(arr) = msg.get("content").and_then(|v| v.as_array()) else {
        return false;
    };
    arr.iter().any(|b| {
        b.get("type").and_then(|v| v.as_str()) == Some("tool_use")
    })
}

fn merge_consecutive(msgs: Vec<Value>) -> Vec<Value> {
    let mut merged: Vec<Value> = Vec::with_capacity(msgs.len());
    for msg in msgs {
        let role = msg
            .get("role")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let should_merge = merged
            .last()
            .and_then(|m| m.get("role"))
            .and_then(|v| v.as_str())
            == Some(&role);
        if should_merge {
            let last = merged.last_mut().unwrap();
            let prev = last.get("content").cloned().unwrap_or(Value::Null);
            let cur = msg.get("content").cloned().unwrap_or(Value::Null);
            let mut prev_arr = match prev {
                Value::Array(a) => a,
                Value::String(s) => vec![json!({"type":"text","text":s})],
                Value::Null => Vec::new(),
                other => vec![json!({"type":"text","text": other.to_string()})],
            };
            let cur_arr = match cur {
                Value::Array(a) => a,
                Value::String(s) => vec![json!({"type":"text","text":s})],
                Value::Null => Vec::new(),
                other => vec![json!({"type":"text","text": other.to_string()})],
            };
            prev_arr.extend(cur_arr);
            if let Some(obj) = last.as_object_mut() {
                obj.insert("content".into(), Value::Array(prev_arr));
            }
        } else {
            merged.push(msg);
        }
    }

    let mut last_popped: Option<Value> = None;
    while let Some(last) = merged.last() {
        if last.get("role").and_then(|v| v.as_str()) == Some("assistant") {
            last_popped = Some(merged.pop().unwrap());
        } else {
            break;
        }
    }
    if merged.is_empty() {
        if let Some(popped) = last_popped.as_ref() {
            if !has_tool_use(popped) {
                merged.push(json!({
                    "role":"user",
                    "content": popped.get("content").cloned().unwrap_or(Value::Null),
                }));
            }
        }
    }

    if let Some(first) = merged.first() {
        if first.get("role").and_then(|v| v.as_str()) == Some("assistant") && !has_tool_use(first) {
            merged.insert(
                0,
                json!({"role":"user","content":"(conversation continued)"}),
            );
        }
    }
    merged
}

fn tool_cache_marker_indices(tools: &[Value]) -> Vec<usize> {
    if tools.is_empty() {
        return Vec::new();
    }
    let tail = tools.len() - 1;
    let mut last_builtin: Option<usize> = None;
    for i in (0..=tail).rev() {
        let name = tools[i]
            .get("name")
            .and_then(|v| v.as_str())
            .or_else(|| tools[i].get("function").and_then(|f| f.get("name")).and_then(|v| v.as_str()))
            .unwrap_or("");
        if !name.starts_with("mcp_") {
            last_builtin = Some(i);
            break;
        }
    }
    let mut out = Vec::new();
    for idx in [last_builtin, Some(tail)] {
        if let Some(i) = idx {
            if !out.contains(&i) {
                out.push(i);
            }
        }
    }
    out
}

fn parse_response(body: &Value) -> LLMResponse {
    let mut content_parts: Vec<String> = Vec::new();
    let mut thinking_blocks: Vec<Value> = Vec::new();
    let mut tool_calls: Vec<ToolCallRequest> = Vec::new();

    if let Some(blocks) = body.get("content").and_then(|v| v.as_array()) {
        for block in blocks {
            let ty = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
            match ty {
                "text" => {
                    if let Some(t) = block.get("text").and_then(|v| v.as_str()) {
                        content_parts.push(t.to_string());
                    }
                }
                "tool_use" => {
                    let id = block
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let name = block
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let input = block.get("input").cloned().unwrap_or(Value::Object(Map::new()));
                    let args = match input {
                        Value::Object(m) => m,
                        _ => Map::new(),
                    };
                    tool_calls.push(ToolCallRequest {
                        id,
                        name,
                        arguments: args,
                        extra_content: None,
                        provider_specific_fields: None,
                        function_provider_specific_fields: None,
                    });
                }
                "thinking" => {
                    thinking_blocks.push(json!({
                        "type":"thinking",
                        "thinking": block.get("thinking").and_then(|v| v.as_str()).unwrap_or(""),
                        "signature": block.get("signature").and_then(|v| v.as_str()).unwrap_or(""),
                    }));
                }
                _ => {}
            }
        }
    }

    let stop_reason = body
        .get("stop_reason")
        .and_then(|v| v.as_str())
        .unwrap_or("stop");
    let finish_reason = match stop_reason {
        "tool_use" => "tool_calls",
        "end_turn" => "stop",
        "max_tokens" => "length",
        other => other,
    }
    .to_string();

    let mut usage: HashMap<String, i64> = HashMap::new();
    if let Some(u) = body.get("usage").and_then(|v| v.as_object()) {
        let input_tokens = u.get("input_tokens").and_then(|v| v.as_i64()).unwrap_or(0);
        let cache_creation = u
            .get("cache_creation_input_tokens")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let cache_read = u
            .get("cache_read_input_tokens")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let output_tokens = u.get("output_tokens").and_then(|v| v.as_i64()).unwrap_or(0);
        let total_prompt = input_tokens + cache_creation + cache_read;
        usage.insert("prompt_tokens".into(), total_prompt);
        usage.insert("completion_tokens".into(), output_tokens);
        usage.insert("total_tokens".into(), total_prompt + output_tokens);
        if cache_creation > 0 {
            usage.insert("cache_creation_input_tokens".into(), cache_creation);
        }
        if cache_read > 0 {
            usage.insert("cache_read_input_tokens".into(), cache_read);
            usage.insert("cached_tokens".into(), cache_read);
        }
    }

    LLMResponse {
        content: (!content_parts.is_empty()).then(|| content_parts.join("")),
        tool_calls,
        finish_reason,
        usage,
        thinking_blocks: (!thinking_blocks.is_empty()).then_some(thinking_blocks),
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
    let mut error_type: Option<String> = None;
    let mut error_code: Option<String> = None;
    if let Some(p) = parsed.as_ref() {
        if let Some(obj) = p.get("error").and_then(|v| v.as_object()) {
            error_type = obj
                .get("type")
                .and_then(|v| v.as_str())
                .map(|s| s.to_ascii_lowercase());
            error_code = obj
                .get("code")
                .and_then(|v| v.as_str())
                .map(|s| s.to_ascii_lowercase());
        }
    }

    let retry_after = headers
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<f64>().ok())
        .or_else(|| extract_retry_after_from_text(Some(&msg)));
    let should_retry = headers
        .get("x-should-retry")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| match s.trim().to_ascii_lowercase().as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        });

    LLMResponse {
        content: Some(msg),
        finish_reason: "error".into(),
        retry_after,
        error_status_code: Some(status_i32),
        error_type,
        error_code,
        error_retry_after_s: retry_after,
        error_should_retry: should_retry,
        ..Default::default()
    }
}

#[async_trait]
impl LLMProvider for AnthropicProvider {
    fn default_model(&self) -> String {
        self.cfg.default_model.clone()
    }

    fn generation(&self) -> GenerationSettings {
        self.generation.clone()
    }

    fn supports_progress_deltas(&self) -> bool {
        true
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
                    content: Some(msg),
                    finish_reason: "error".into(),
                    error_kind: kind,
                    ..Default::default()
                };
            }
        };
        let status = resp.status();
        let headers = resp.headers().clone();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            warn!("Anthropic error {status}: {text}");
            return parse_error_response(status, &headers, &text);
        }
        let text = match resp.text().await {
            Ok(t) => t,
            Err(e) => return LLMResponse::error(format!("Error reading body: {e}")),
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            return LLMResponse::error(format!("Error: malformed JSON: {text}"));
        };
        parse_response(&value)
    }

    async fn chat_stream(
        &self,
        req: ChatRequest,
        on_delta: Option<crate::base::StreamDeltaCallback>,
        on_tool_call_delta: Option<crate::responses::ToolCallDeltaCallback>,
    ) -> LLMResponse {
        use std::env;

        let idle_timeout_s: u64 = env::var("NANOBOT_STREAM_IDLE_TIMEOUT_S")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(90);

        let mut body = self.build_body(&req);
        // Enable streaming by setting stream=true
        if let Some(obj) = body.as_object_mut() {
            obj.insert("stream".into(), serde_json::json!(true));
        }

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
                    content: Some(msg),
                    finish_reason: "error".into(),
                    error_kind: kind,
                    ..Default::default()
                };
            }
        };

        let status = resp.status();
        let headers = resp.headers().clone();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            warn!("Anthropic stream error {status}: {text}");
            return parse_error_response(status, &headers, &text);
        }

        // Parse SSE stream
        let mut stream = resp.bytes_stream();
        let mut content_parts: Vec<String> = Vec::new();
        let mut tool_calls: Vec<ToolCallRequest> = Vec::new();
        let mut thinking_blocks: Vec<Value> = Vec::new();
        let mut finish_reason = "stop".to_string();
        let mut usage: std::collections::HashMap<String, i64> = std::collections::HashMap::new();

        // Accumulator for SSE events
        let mut current_event_data = String::new();
        let mut current_event_type: Option<String> = None;

        use futures::StreamExt;
        let timeout_duration = tokio::time::Duration::from_secs(idle_timeout_s);

        loop {
            let chunk = match tokio::time::timeout(timeout_duration, stream.next()).await {
                Ok(Some(Ok(chunk))) => chunk,
                Ok(Some(Err(e))) => {
                    return LLMResponse {
                        content: Some(format!("Error reading stream: {e}")),
                        finish_reason: "error".into(),
                        error_kind: Some("connection".to_string()),
                        ..Default::default()
                    };
                }
                Ok(None) => break, // Stream ended
                Err(_) => {
                    return LLMResponse {
                        content: Some(format!(
                            "Error calling LLM: stream stalled for more than {idle_timeout_s} seconds"
                        )),
                        finish_reason: "error".into(),
                        error_kind: Some("timeout".to_string()),
                        ..Default::default()
                    };
                }
            };

            let text = String::from_utf8_lossy(&chunk);
            for line in text.lines() {
                let line = line.trim();
                if line.is_empty() {
                    // Empty line marks end of event, process accumulated data
                    if !current_event_data.is_empty() {
                        if let Some(event_type) = &current_event_type {
                            process_sse_event(
                                event_type,
                                &current_event_data,
                                &mut content_parts,
                                &mut tool_calls,
                                &mut thinking_blocks,
                                &mut finish_reason,
                                &mut usage,
                                on_delta.as_ref(),
                                on_tool_call_delta.as_ref(),
                            );
                        }
                        current_event_data.clear();
                        current_event_type = None;
                    }
                    continue;
                }

                if let Some(stripped) = line.strip_prefix("event: ") {
                    current_event_type = Some(stripped.to_string());
                } else if let Some(stripped) = line.strip_prefix("data: ") {
                    current_event_data.push_str(stripped);
                }
            }
        }

        // Process any remaining event
        if !current_event_data.is_empty() {
            if let Some(event_type) = &current_event_type {
                process_sse_event(
                    event_type,
                    &current_event_data,
                    &mut content_parts,
                    &mut tool_calls,
                    &mut thinking_blocks,
                    &mut finish_reason,
                    &mut usage,
                    on_delta.as_ref(),
                    on_tool_call_delta.as_ref(),
                );
            }
        }

        LLMResponse {
            content: (!content_parts.is_empty()).then(|| content_parts.join("")),
            tool_calls,
            finish_reason,
            usage,
            thinking_blocks: (!thinking_blocks.is_empty()).then_some(thinking_blocks),
            ..Default::default()
        }
    }
}

/// Process a single SSE event from Anthropic's streaming API.
fn process_sse_event(
    event_type: &str,
    data: &str,
    content_parts: &mut Vec<String>,
    tool_calls: &mut Vec<ToolCallRequest>,
    thinking_blocks: &mut Vec<Value>,
    finish_reason: &mut String,
    usage: &mut std::collections::HashMap<String, i64>,
    on_delta: Option<&crate::base::StreamDeltaCallback>,
    on_tool_call_delta: Option<&crate::responses::ToolCallDeltaCallback>,
) {
    let Ok(value) = serde_json::from_str::<Value>(data) else {
        return;
    };

    match event_type {
        "content_block_start" => {
            if let Some(block_type) = value.get("content_block").and_then(|v| v.get("type")).and_then(|v| v.as_str()) {
                match block_type {
                    "text" => {
                        if let Some(text) = value.get("content_block").and_then(|v| v.get("text")).and_then(|v| v.as_str()) {
                            if !text.is_empty() {
                                content_parts.push(text.to_string());
                            }
                        }
                    }
                    "tool_use" => {
                        if let Some(block) = value.get("content_block") {
                            let id = block.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                            let name = block.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                            let input = block.get("input").cloned().unwrap_or(serde_json::json!({}));
                            let args = match input {
                                serde_json::Value::Object(m) => m,
                                _ => serde_json::Map::new(),
                            };
                            tool_calls.push(ToolCallRequest {
                                id: id.clone(),
                                name: name.clone(),
                                arguments: args,
                                extra_content: None,
                                provider_specific_fields: None,
                                function_provider_specific_fields: None,
                            });
                            if let Some(ref cb) = on_tool_call_delta {
                                let mut delta = serde_json::Map::new();
                                delta.insert("call_id".into(), serde_json::Value::String(id));
                                delta.insert("name".into(), serde_json::Value::String(name));
                                delta.insert("arguments_delta".into(), serde_json::Value::String(String::new()));
                                cb(delta);
                            }
                        }
                    }
                    "thinking" => {
                        if let Some(block) = value.get("content_block") {
                            thinking_blocks.push(serde_json::json!({
                                "type": "thinking",
                                "thinking": block.get("thinking").and_then(|v| v.as_str()).unwrap_or(""),
                                "signature": block.get("signature").and_then(|v| v.as_str()).unwrap_or(""),
                            }));
                        }
                    }
                    _ => {}
                }
            }
        }
        "content_block_delta" => {
            if let Some(delta_type) = value.get("delta").and_then(|v| v.get("type")).and_then(|v| v.as_str()) {
                match delta_type {
                    "text_delta" => {
                        if let Some(text) = value.get("delta").and_then(|v| v.get("text")).and_then(|v| v.as_str()) {
                            if !text.is_empty() {
                                if let Some(ref cb) = on_delta {
                                    cb(text.to_string());
                                }
                            }
                        }
                    }
                    "input_json_delta" => {
                        if let Some(partial_json) = value.get("delta").and_then(|v| v.get("partial_json")).and_then(|v| v.as_str()) {
                            if let Some(ref cb) = on_tool_call_delta {
                                let mut delta = serde_json::Map::new();
                                delta.insert("arguments_delta".into(), serde_json::Value::String(partial_json.to_string()));
                                cb(delta);
                            }
                        }
                    }
                    "thinking_delta" => {
                        if let Some(thinking) = value.get("delta").and_then(|v| v.get("thinking")).and_then(|v| v.as_str()) {
                            if !thinking_blocks.is_empty() {
                                if let Some(last_thinking) = thinking_blocks.last_mut() {
                                    if let Some(obj) = last_thinking.as_object_mut() {
                                        let existing = obj.get("thinking").and_then(|v| v.as_str()).unwrap_or("");
                                        obj.insert("thinking".into(), serde_json::Value::String(format!("{existing}{thinking}")));
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        "message_delta" => {
            if let Some(stop_reason) = value.get("delta").and_then(|v| v.get("stop_reason")).and_then(|v| v.as_str()) {
                *finish_reason = match stop_reason {
                    "tool_use" => "tool_calls".to_string(),
                    "end_turn" => "stop".to_string(),
                    "max_tokens" => "length".to_string(),
                    other => other.to_string(),
                };
            }
            if let Some(u) = value.get("usage") {
                if let Some(output_tokens) = u.get("output_tokens").and_then(|v| v.as_i64()) {
                    usage.insert("completion_tokens".into(), output_tokens);
                }
            }
        }
        "message_start" => {
            if let Some(u) = value.get("message").and_then(|v| v.get("usage")) {
                let input_tokens = u.get("input_tokens").and_then(|v| v.as_i64()).unwrap_or(0);
                let cache_creation = u.get("cache_creation_input_tokens").and_then(|v| v.as_i64()).unwrap_or(0);
                let cache_read = u.get("cache_read_input_tokens").and_then(|v| v.as_i64()).unwrap_or(0);
                let total_prompt = input_tokens + cache_creation + cache_read;
                usage.insert("prompt_tokens".into(), total_prompt);
                usage.insert("total_tokens".into(), total_prompt);
                if cache_creation > 0 {
                    usage.insert("cache_creation_input_tokens".into(), cache_creation);
                }
                if cache_read > 0 {
                    usage.insert("cache_read_input_tokens".into(), cache_read);
                    usage.insert("cached_tokens".into(), cache_read);
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn convert_simple_chat() {
        let msgs = vec![
            json!({"role":"system","content":"you are helpful"}),
            json!({"role":"user","content":"hi"}),
        ];
        let (system, out) = AnthropicProvider::convert_messages(&msgs);
        assert_eq!(system, json!("you are helpful"));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["role"], "user");
        assert_eq!(out[0]["content"], "hi");
    }

    #[test]
    fn convert_tool_call_roundtrip() {
        let msgs = vec![
            json!({"role":"user","content":"do it"}),
            json!({
                "role":"assistant",
                "content":"",
                "tool_calls":[{
                    "id":"toolu_1",
                    "type":"function",
                    "function":{"name":"x","arguments":"{\"a\":1}"}
                }]
            }),
            json!({"role":"tool","tool_call_id":"toolu_1","content":"ok"}),
            json!({"role":"user","content":"thanks"}),
        ];
        let (_system, out) = AnthropicProvider::convert_messages(&msgs);
        // user, assistant(tool_use), user(tool_result), user(thanks)
        // merge_consecutive collapses the tool_result user with the thanks user.
        assert_eq!(out.len(), 3);
        assert_eq!(out[1]["role"], "assistant");
        let blocks = out[1]["content"].as_array().unwrap();
        assert!(blocks.iter().any(|b| b["type"] == "tool_use" && b["name"] == "x"));
        let last = &out[2]["content"];
        // Last user turn has [tool_result, text:thanks]
        assert!(last.is_array());
    }

    #[test]
    fn convert_tools_translates_schema() {
        let tools = vec![json!({
            "type":"function",
            "function":{
                "name":"read_file",
                "description":"read a file",
                "parameters":{"type":"object","properties":{"path":{"type":"string"}}}
            }
        })];
        let out = AnthropicProvider::convert_tools(Some(&tools)).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["name"], "read_file");
        assert_eq!(out[0]["description"], "read a file");
        assert!(out[0]["input_schema"]["properties"]["path"].is_object());
    }

    #[test]
    fn build_body_thinking_high() {
        let cfg = AnthropicConfig::new("claude-opus-4-20250514").with_caching(false);
        let p = AnthropicProvider::new(cfg);
        let req = ChatRequest {
            messages: vec![json!({"role":"user","content":"think hard"})],
            tools: None,
            model: None,
            max_tokens: 2000,
            temperature: 0.3,
            reasoning_effort: Some("high".into()),
            tool_choice: None,
        };
        let body = p.build_body(&req);
        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["temperature"], 1.0);
        // High budget => 8192; max_tokens bumped to budget + 4096.
        assert_eq!(body["max_tokens"], 8192 + 4096);
    }

    #[test]
    fn build_body_adaptive_sets_temp() {
        let cfg = AnthropicConfig::new("claude-sonnet-4-6").with_caching(false);
        let p = AnthropicProvider::new(cfg);
        let req = ChatRequest {
            messages: vec![json!({"role":"user","content":"x"})],
            tools: None,
            model: None,
            max_tokens: 1024,
            temperature: 0.7,
            reasoning_effort: Some("adaptive".into()),
            tool_choice: None,
        };
        let body = p.build_body(&req);
        assert_eq!(body["thinking"]["type"], "adaptive");
        assert_eq!(body["temperature"], 1.0);
    }

    #[test]
    fn parse_response_tool_use() {
        let body = json!({
            "content":[
                {"type":"text","text":"ok"},
                {"type":"tool_use","id":"toolu_9","name":"read_file","input":{"path":"/a"}}
            ],
            "stop_reason":"tool_use",
            "usage":{"input_tokens":10,"output_tokens":5}
        });
        let out = parse_response(&body);
        assert_eq!(out.content.as_deref(), Some("ok"));
        assert_eq!(out.finish_reason, "tool_calls");
        assert_eq!(out.tool_calls.len(), 1);
        assert_eq!(out.tool_calls[0].name, "read_file");
        assert_eq!(out.usage["total_tokens"], 15);
    }

    #[test]
    fn parse_response_thinking_blocks() {
        let body = json!({
            "content":[
                {"type":"thinking","thinking":"hm","signature":"sig"},
                {"type":"text","text":"answer"}
            ],
            "stop_reason":"end_turn",
            "usage":{"input_tokens":1,"output_tokens":1}
        });
        let out = parse_response(&body);
        assert_eq!(out.finish_reason, "stop");
        assert!(out.thinking_blocks.is_some());
        assert_eq!(out.thinking_blocks.as_ref().unwrap().len(), 1);
    }

    #[test]
    fn strip_prefix_removes_gateway_prefix() {
        assert_eq!(AnthropicProvider::strip_prefix("anthropic/claude-x"), "claude-x");
        assert_eq!(AnthropicProvider::strip_prefix("claude-x"), "claude-x");
    }
}
