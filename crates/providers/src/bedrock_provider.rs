//! AWS Bedrock Converse provider.
//!
//! Port of ``nanobot.providers.bedrock_provider``. Uses the Bedrock
//! Converse API over raw HTTP with AWS SigV4 signing (via the ``aws-sigv4``
//! crate). Mirrors the Python provider's message conversion, tool
//! conversion, streaming event parsing, and error handling.

use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use log::warn;
use serde_json::{Map, Value, json};

use crate::base::extract_retry_after_from_text;
use crate::base::sanitize_empty_content;
use crate::base::{ChatRequest, LLMProvider, StreamDeltaCallback};
use crate::base::{GenerationSettings, LLMResponse, ToolCallRequest, ToolChoice};

// Model constants mirroring the Python module.
static TEMPERATURE_UNSUPPORTED_MODEL_TOKENS: &[&str] = &["claude-opus-4-7"];
static ADAPTIVE_THINKING_ONLY_MODEL_TOKENS: &[&str] = &["claude-opus-4-7"];
static NOOP_TOOL_NAME: &str = "nanobot_noop";

fn matches_model_token(model: &str, tokens: &[&str]) -> bool {
    let m = model.to_ascii_lowercase();
    tokens.iter().any(|t| m.contains(*t))
}

fn supports_temperature(model: &str) -> bool {
    !matches_model_token(model, TEMPERATURE_UNSUPPORTED_MODEL_TOKENS)
}

fn uses_adaptive_thinking_only(model: &str) -> bool {
    matches_model_token(model, ADAPTIVE_THINKING_ONLY_MODEL_TOKENS)
}

fn strip_prefix(model: &str) -> String {
    model.strip_prefix("bedrock/").unwrap_or(model).to_string()
}

fn deep_merge(base: &mut Map<String, Value>, override_map: &Map<String, Value>) {
    for (key, value) in override_map {
        if let (Some(Value::Object(base_obj)), Value::Object(override_obj)) =
            (base.get_mut(key), value)
        {
            deep_merge(base_obj, override_obj);
        } else {
            base.insert(key.clone(), value.clone());
        }
    }
}

fn content_blocks(content: &Value, for_tool_result: bool) -> Vec<Value> {
    match content {
        Value::String(s) => {
            if s.is_empty() {
                vec![json!({"text": "(empty)"})]
            } else {
                vec![json!({"text": s})]
            }
        }
        Value::Null => vec![json!({"text": "(empty)"})],
        Value::Array(items) => {
            let mut blocks: Vec<Value> = Vec::new();
            for item in items {
                let Some(obj) = item.as_object() else {
                    blocks.push(json!({"text": item.to_string()}));
                    continue;
                };

                let item_type = obj.get("type").and_then(|v| v.as_str());
                if matches!(
                    item_type,
                    Some("text") | Some("input_text") | Some("output_text")
                ) {
                    if let Some(text) = obj.get("text").and_then(|v| v.as_str()) {
                        if !text.is_empty() {
                            blocks.push(json!({"text": text}));
                        }
                    }
                    continue;
                }
                if item_type == Some("image_url") {
                    if let Some(converted) = image_url_block(obj) {
                        blocks.push(converted);
                    }
                    continue;
                }

                let mut found = false;
                for key in &["text", "image", "document", "video", "json", "searchResult"] {
                    if obj.contains_key(*key) {
                        let key_str = *key;
                        blocks.push(json!({key_str: obj[key_str]}));
                        found = true;
                        break;
                    }
                }
                if !found {
                    if for_tool_result {
                        blocks.push(json!({"json": item}));
                    } else {
                        blocks
                            .push(json!({"text": serde_json::to_string(item).unwrap_or_default()}));
                    }
                }
            }
            if blocks.is_empty() {
                blocks.push(json!({"text": "(empty)"}));
            }
            blocks
        }
        Value::Object(o) if for_tool_result => vec![json!({"json": o})],
        other => vec![json!({"text": other.to_string()})],
    }
}

fn image_url_block(block: &Map<String, Value>) -> Option<Value> {
    let url = block
        .get("image_url")
        .and_then(|v| v.get("url"))
        .and_then(|v| v.as_str())?;
    if url.is_empty() {
        return None;
    }

    let pattern = regex::Regex::new(r"^data:image/([a-zA-Z0-9.+-]+);base64,(.*)$").ok()?;
    if let Some(caps) = pattern.captures(url) {
        let fmt = caps.get(1)?.as_str().to_ascii_lowercase();
        let fmt = if fmt == "jpg" { "jpeg" } else { &*fmt };
        let b64 = caps.get(2)?.as_str();
        return Some(json!({
            "image": {
                "format": fmt,
                "source": {"bytes": b64}
            }
        }));
    }
    Some(json!({"text": format!("(image URL: {url})")}))
}

fn system_blocks(content: &Value) -> Vec<Value> {
    content_blocks(content, false)
        .into_iter()
        .filter(|b| {
            b.as_object().map_or(false, |o| {
                o.contains_key("text")
                    || o.contains_key("cachePoint")
                    || o.contains_key("guardContent")
            })
        })
        .collect()
}

fn tool_result_block(msg: &Value) -> Value {
    let tool_call_id = msg
        .get("tool_call_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    json!({
        "toolResult": {
            "toolUseId": tool_call_id,
            "content": content_blocks(msg.get("content").cloned().as_ref().unwrap_or(&Value::Null), true),
            "status": "success",
        }
    })
}

fn tool_use_block(tool_call: &Value) -> Option<Value> {
    let function = tool_call.get("function").and_then(|v| v.as_object())?;
    let args = function.get("arguments").cloned().unwrap_or(Value::Null);
    let args = match &args {
        Value::String(s) if !s.trim().is_empty() => {
            serde_json::from_str::<Value>(s).unwrap_or_else(|_| json!({}))
        }
        Value::Null | Value::String(_) => json!({}),
        other => other.clone(),
    };
    let args = args.as_object().cloned().unwrap_or_default();
    let id = tool_call.get("id").and_then(|v| v.as_str()).unwrap_or("");
    let name = function.get("name").and_then(|v| v.as_str()).unwrap_or("");
    Some(json!({
        "toolUse": {
            "toolUseId": id,
            "name": name,
            "input": args,
        }
    }))
}

fn reasoning_block(block: &Value) -> Option<Value> {
    let ty = block.get("type").and_then(|v| v.as_str())?;
    if !matches!(ty, "thinking" | "reasoning" | "redacted_thinking") {
        return None;
    }
    if let (Some(text), Some(signature)) = (
        block
            .get("thinking")
            .or_else(|| block.get("text"))
            .and_then(|v| v.as_str()),
        block.get("signature").and_then(|v| v.as_str()),
    ) {
        return Some(json!({
            "reasoningContent": {
                "reasoningText": {"text": text, "signature": signature}
            }
        }));
    }
    if let Some(redacted) = block.get("redactedContent") {
        return Some(json!({
            "reasoningContent": {"redactedContent": redacted}
        }));
    }
    None
}

fn assistant_blocks(msg: &Value) -> Vec<Value> {
    let mut blocks: Vec<Value> = Vec::new();

    if let Some(thinking_list) = msg.get("thinking_blocks").and_then(|v| v.as_array()) {
        for thinking in thinking_list {
            if thinking.as_object().is_some() {
                if let Some(reasoning) = reasoning_block(thinking) {
                    blocks.push(reasoning);
                }
            }
        }
    }

    if let Some(content) = msg.get("content") {
        if let Value::String(s) = content {
            if !s.is_empty() {
                blocks.push(json!({"text": s}));
            }
        } else if let Value::Array(items) = content {
            for block in content_blocks(content, false) {
                if block.as_object().map_or(false, |o| o.contains_key("text")) {
                    blocks.push(block);
                }
            }
            let _ = items;
        }
    }

    if let Some(tool_calls) = msg.get("tool_calls").and_then(|v| v.as_array()) {
        for tool_call in tool_calls {
            if let Some(block) = tool_use_block(tool_call) {
                blocks.push(block);
            }
        }
    }

    if blocks.is_empty() {
        blocks.push(json!({"text": ""}));
    }
    blocks
}

fn has_tool_use(msg: &Value) -> bool {
    let Some(content) = msg.get("content").and_then(|v| v.as_array()) else {
        return false;
    };
    content.iter().any(|block| {
        block.as_object().map_or(false, |o| {
            o.contains_key("toolUse") || o.contains_key("toolResult")
        })
    })
}

fn merge_consecutive(messages: Vec<Value>) -> Vec<Value> {
    let mut merged: Vec<Value> = Vec::new();
    for msg in messages {
        let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("");
        if let Some(last) = merged.last_mut() {
            let last_role = last.get("role").and_then(|v| v.as_str()).unwrap_or("");
            if last_role == role
                && role != "system"
                && role != "tool"
                && matches!(role, "user" | "assistant")
            {
                let prev_content = last.get_mut("content");
                let cur_content = msg.get("content").cloned().unwrap_or(Value::Null);
                if let Some(prev) = prev_content {
                    if let Some(arr) = prev.as_array_mut() {
                        if let Value::Array(cur_arr) = cur_content {
                            arr.extend(cur_arr);
                        } else {
                            arr.push(json!({"text": cur_content.to_string()}));
                        }
                    } else {
                        *prev =
                            json!([{"text": prev.to_string()}, {"text": cur_content.to_string()}]);
                    }
                }
                continue;
            }
        }
        merged.push(msg);
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
        if let Some(popped) = last_popped {
            if !has_tool_use(&popped) {
                merged.push(json!({
                    "role": "user",
                    "content": popped.get("content").cloned().unwrap_or(json!([{"text": "(empty)"}]))
                }));
            }
        }
    }
    if let Some(first) = merged.first() {
        if first.get("role").and_then(|v| v.as_str()) == Some("assistant") && !has_tool_use(first) {
            merged.insert(
                0,
                json!({"role": "user", "content": [{"text": "(conversation continued)"}]}),
            );
        }
    }
    merged
}

fn convert_messages(messages: &[Value]) -> (Vec<Value>, Vec<Value>) {
    let mut system: Vec<Value> = Vec::new();
    let mut converted: Vec<Value> = Vec::new();

    for msg in messages {
        let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("");
        match role {
            "system" => {
                if let Some(content) = msg.get("content") {
                    system.extend(system_blocks(content));
                }
            }
            "tool" => {
                let block = tool_result_block(msg);
                if let Some(last) = converted.last_mut() {
                    if last.get("role").and_then(|v| v.as_str()) == Some("user") {
                        if let Some(arr) = last.get_mut("content").and_then(|v| v.as_array_mut()) {
                            arr.push(block);
                        } else {
                            *last = json!({"role": "user", "content": [last.get("content").cloned().unwrap_or(Value::Null), block]});
                        }
                    } else {
                        converted.push(json!({"role": "user", "content": [block]}));
                    }
                } else {
                    converted.push(json!({"role": "user", "content": [block]}));
                }
            }
            "assistant" => {
                converted.push(json!({"role": "assistant", "content": assistant_blocks(msg)}));
            }
            "user" => {
                if let Some(content) = msg.get("content") {
                    converted
                        .push(json!({"role": "user", "content": content_blocks(content, false)}));
                }
            }
            _ => {}
        }
    }

    (system, merge_consecutive(converted))
}

fn convert_tools(tools: &[Value]) -> Option<Vec<Value>> {
    if tools.is_empty() {
        return None;
    }
    let mut result: Vec<Value> = Vec::new();
    for tool in tools {
        let func = if let Some(f) = tool.get("function").and_then(|v| v.as_object()) {
            f
        } else if let Some(f) = tool.as_object() {
            f
        } else {
            continue;
        };
        let name = func.get("name").and_then(|v| v.as_str());
        if name.is_none() || name.unwrap().is_empty() {
            continue;
        }
        let name = name.unwrap();
        let params = func
            .get("parameters")
            .cloned()
            .unwrap_or_else(|| json!({"type": "object", "properties": {}}));
        let mut spec: Map<String, Value> = Map::new();
        spec.insert("name".into(), Value::String(name.to_string()));
        spec.insert("inputSchema".into(), json!({"json": params}));
        if let Some(desc) = func.get("description").and_then(|v| v.as_str()) {
            spec.insert("description".into(), Value::String(desc.to_string()));
        }
        if let (Some(strict), Some(tool_strict)) = (
            func.get("strict").and_then(|v| v.as_bool()),
            tool.get("strict").and_then(|v| v.as_bool()),
        ) {
            spec.insert("strict".into(), Value::Bool(strict || tool_strict));
        } else if let Some(strict) = func.get("strict").and_then(|v| v.as_bool()) {
            spec.insert("strict".into(), Value::Bool(strict));
        } else if let Some(strict) = tool.get("strict").and_then(|v| v.as_bool()) {
            spec.insert("strict".into(), Value::Bool(strict));
        }
        result.push(json!({"toolSpec": spec}));
    }
    if result.is_empty() {
        None
    } else {
        Some(result)
    }
}

fn contains_tool_blocks(messages: &[Value]) -> bool {
    messages.iter().any(|msg| {
        let Some(content) = msg.get("content").and_then(|v| v.as_array()) else {
            return false;
        };
        content.iter().any(|block| {
            block.as_object().map_or(false, |o| {
                o.contains_key("toolUse") || o.contains_key("toolResult")
            })
        })
    })
}

fn noop_tool() -> Value {
    json!({
        "toolSpec": {
            "name": NOOP_TOOL_NAME,
            "description": "Internal placeholder for Bedrock tool history validation.",
            "inputSchema": {"json": {"type": "object", "properties": {}}},
        }
    })
}

fn convert_tool_choice(tool_choice: Option<&ToolChoice>) -> Option<Value> {
    match tool_choice {
        None | Some(ToolChoice::Auto) => Some(json!({"auto": {}})),
        Some(ToolChoice::Required) => Some(json!({"any": {}})),
        Some(ToolChoice::None) => None,
        Some(ToolChoice::Specific(v)) => {
            if let Some(name) = v
                .get("function")
                .and_then(|f| f.get("name"))
                .and_then(|v| v.as_str())
            {
                Some(json!({"tool": {"name": name}}))
            } else {
                Some(json!({"auto": {}}))
            }
        }
    }
}

fn adaptive_thinking(reasoning_effort: Option<&str>) -> Option<Value> {
    let effort = reasoning_effort?.to_ascii_lowercase();
    if effort == "none" {
        return None;
    }
    let mut thinking = json!({"type": "adaptive"});
    if effort != "adaptive" {
        if let Some(obj) = thinking.as_object_mut() {
            obj.insert(
                "effort".into(),
                Value::String(reasoning_effort?.to_string()),
            );
        }
    }
    Some(thinking)
}

fn finish_reason(stop_reason: Option<&str>) -> String {
    match stop_reason.unwrap_or("") {
        "end_turn" => "stop".to_string(),
        "tool_use" => "tool_calls".to_string(),
        "max_tokens" => "length".to_string(),
        other => other.to_string(),
    }
}

fn usage_map(usage: Option<&Value>) -> HashMap<String, i64> {
    let mut result = HashMap::new();
    let Some(u) = usage.and_then(|v| v.as_object()) else {
        return result;
    };
    let as_i = |k: &str| u.get(k).and_then(|v| v.as_i64()).unwrap_or(0);
    let prompt = as_i("inputTokens");
    let completion = as_i("outputTokens");
    let total = as_i("totalTokens").max(prompt + completion);
    result.insert("prompt_tokens".into(), prompt);
    result.insert("completion_tokens".into(), completion);
    result.insert("total_tokens".into(), total);
    let cache_read = as_i("cacheReadInputTokens");
    let cache_write = as_i("cacheWriteInputTokens");
    if cache_read > 0 {
        result.insert("cached_tokens".into(), cache_read);
        result.insert("cache_read_input_tokens".into(), cache_read);
    }
    if cache_write > 0 {
        result.insert("cache_creation_input_tokens".into(), cache_write);
    }
    result
}

fn parse_reasoning(block: &Value) -> (Option<String>, Option<Value>) {
    let Some(reasoning) = block.get("reasoningContent").and_then(|v| v.as_object()) else {
        return (None, None);
    };
    if let Some(text_obj) = reasoning.get("reasoningText").and_then(|v| v.as_object()) {
        if let Some(text) = text_obj.get("text").and_then(|v| v.as_str()) {
            let signature = text_obj
                .get("signature")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            return (
                Some(text.to_string()),
                Some(json!({
                    "type": "thinking",
                    "thinking": text,
                    "signature": signature,
                })),
            );
        }
    }
    if let Some(redacted) = reasoning.get("redactedContent") {
        return (
            None,
            Some(json!({
                "type": "redacted_thinking",
                "redactedContent": redacted,
            })),
        );
    }
    (None, None)
}

fn parse_response(response: &Value) -> LLMResponse {
    let mut content_parts: Vec<String> = Vec::new();
    let mut reasoning_parts: Vec<String> = Vec::new();
    let mut tool_calls: Vec<ToolCallRequest> = Vec::new();
    let mut thinking_blocks: Vec<Value> = Vec::new();

    let message = response
        .get("output")
        .and_then(|v| v.get("message"))
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();

    if let Some(content_list) = message.get("content").and_then(|v| v.as_array()) {
        for block in content_list {
            let Some(obj) = block.as_object() else {
                continue;
            };
            if let Some(text) = obj.get("text").and_then(|v| v.as_str()) {
                content_parts.push(text.to_string());
            }
            if let Some(tool_use) = obj.get("toolUse").and_then(|v| v.as_object()) {
                let id = tool_use
                    .get("toolUseId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let name = tool_use
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let arguments = tool_use
                    .get("input")
                    .and_then(|v| v.as_object())
                    .cloned()
                    .unwrap_or_default();
                tool_calls.push(ToolCallRequest {
                    id,
                    name,
                    arguments,
                    extra_content: None,
                    provider_specific_fields: None,
                    function_provider_specific_fields: None,
                });
            }
            let (reasoning_text, thinking) = parse_reasoning(block);
            if let Some(rt) = reasoning_text {
                reasoning_parts.push(rt);
            }
            if let Some(t) = thinking {
                thinking_blocks.push(t);
            }
        }
    }

    LLMResponse {
        content: (!content_parts.is_empty()).then(|| content_parts.join("")),
        tool_calls,
        finish_reason: finish_reason(response.get("stopReason").and_then(|v| v.as_str())),
        usage: usage_map(response.get("usage")),
        reasoning_content: (!reasoning_parts.is_empty()).then(|| reasoning_parts.join("")),
        thinking_blocks: (!thinking_blocks.is_empty()).then_some(thinking_blocks),
        ..Default::default()
    }
}

#[derive(Clone)]
pub struct BedrockConfig {
    pub default_model: String,
    pub region: Option<String>,
    pub profile: Option<String>,
    pub api_key: Option<String>,
    pub api_base: Option<String>,
    pub extra_body: Map<String, Value>,
    pub timeout: Duration,
}

impl Default for BedrockConfig {
    fn default() -> Self {
        Self {
            default_model: "bedrock/global.anthropic.claude-opus-4-7".into(),
            region: std::env::var("AWS_REGION")
                .ok()
                .or_else(|| std::env::var("AWS_DEFAULT_REGION").ok()),
            profile: None,
            api_key: None,
            api_base: None,
            extra_body: Map::new(),
            timeout: Duration::from_secs(120),
        }
    }
}

impl BedrockConfig {
    pub fn new(default_model: impl Into<String>) -> Self {
        Self {
            default_model: default_model.into(),
            ..Default::default()
        }
    }

    pub fn with_region(mut self, region: impl Into<String>) -> Self {
        self.region = Some(region.into());
        self
    }

    pub fn with_api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into());
        self
    }

    pub fn with_api_base(mut self, base: impl Into<String>) -> Self {
        self.api_base = Some(base.into());
        self
    }

    pub fn with_extra_body(mut self, body: Map<String, Value>) -> Self {
        self.extra_body = body;
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

pub struct BedrockProvider {
    cfg: BedrockConfig,
    client: reqwest::Client,
    generation: GenerationSettings,
}

impl BedrockProvider {
    pub fn new(cfg: BedrockConfig) -> Result<Self, reqwest::Error> {
        let client = reqwest::Client::builder().timeout(cfg.timeout).build()?;
        Ok(Self {
            cfg,
            client,
            generation: GenerationSettings::default(),
        })
    }

    pub fn with_generation(mut self, generation: GenerationSettings) -> Self {
        self.generation = generation;
        self
    }

    fn converse_url(&self, model_id: &str) -> String {
        if let Some(base) = &self.cfg.api_base {
            let base = base.trim_end_matches('/');
            return format!("{base}/model/{model_id}/converse");
        }
        let region = self.cfg.region.as_deref().unwrap_or("us-east-1");
        format!("https://bedrock-runtime.{region}.amazonaws.com/model/{model_id}/converse")
    }

    fn converse_stream_url(&self, model_id: &str) -> String {
        if let Some(base) = &self.cfg.api_base {
            let base = base.trim_end_matches('/');
            return format!("{base}/model/{model_id}/converse-stream");
        }
        let region = self.cfg.region.as_deref().unwrap_or("us-east-1");
        format!("https://bedrock-runtime.{region}.amazonaws.com/model/{model_id}/converse-stream")
    }

    fn build_kwargs(&self, req: &ChatRequest) -> (String, Map<String, Value>) {
        let model_id = strip_prefix(req.model.as_deref().unwrap_or(&self.cfg.default_model));
        let messages = sanitize_empty_content(&req.messages);
        let (system, bedrock_messages) = convert_messages(&messages);
        let bedrock_messages = if bedrock_messages.is_empty() {
            vec![json!({"role": "user", "content": [{"text": "(empty)"}]})]
        } else {
            bedrock_messages
        };

        let mut kwargs: Map<String, Value> = Map::new();
        kwargs.insert("modelId".into(), Value::String(model_id.clone()));
        kwargs.insert("messages".into(), Value::Array(bedrock_messages));

        let mut inference_config: Map<String, Value> = Map::new();
        inference_config.insert("maxTokens".into(), json!(req.max_tokens.max(1)));
        if supports_temperature(&model_id) {
            inference_config.insert("temperature".into(), json!(req.temperature as f64));
        }
        kwargs.insert("inferenceConfig".into(), Value::Object(inference_config));

        if !system.is_empty() {
            kwargs.insert("system".into(), Value::Array(system));
        }

        let mut additional: Map<String, Value> = Map::new();
        if uses_adaptive_thinking_only(&model_id) {
            if let Some(thinking) = adaptive_thinking(req.reasoning_effort.as_deref()) {
                additional.insert("thinking".into(), thinking);
            }
        }
        if !self.cfg.extra_body.is_empty() {
            deep_merge(&mut additional, &self.cfg.extra_body);
        }
        if !additional.is_empty() {
            kwargs.insert(
                "additionalModelRequestFields".into(),
                Value::Object(additional),
            );
        }

        let bedrock_tools = convert_tools(req.tools.as_deref().unwrap_or(&[]));
        let mut tool_config: Option<Value> = None;
        if let Some(tools) = bedrock_tools {
            let mut tc = Map::new();
            tc.insert("tools".into(), Value::Array(tools));
            if let Some(choice) = convert_tool_choice(req.tool_choice.as_ref()) {
                tc.insert("toolChoice".into(), choice);
            }
            tool_config = Some(Value::Object(tc));
        } else if let Some(msgs) = kwargs.get("messages").and_then(|v| v.as_array()) {
            if contains_tool_blocks(msgs) {
                tool_config = Some(json!({"tools": [noop_tool()]}));
            }
        }
        if let Some(tc) = tool_config {
            kwargs.insert("toolConfig".into(), tc);
        }

        (model_id, kwargs)
    }

    fn parse_stream_event(
        event: &Value,
        content_parts: &mut Vec<String>,
        reasoning_parts: &mut Vec<String>,
        thinking_blocks: &mut Vec<Value>,
        tool_buffers: &mut HashMap<usize, Map<String, Value>>,
        reasoning_buffers: &mut HashMap<usize, Map<String, Value>>,
        state: &mut Map<String, Value>,
        on_tool_call_delta: Option<&crate::responses::ToolCallDeltaCallback>,
    ) -> Option<String> {
        if let Some(data) = event.get("contentBlockStart").and_then(|v| v.as_object()) {
            let idx = data
                .get("contentBlockIndex")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize;
            if let Some(start) = data.get("start").and_then(|v| v.as_object()) {
                if let Some(tool_use) = start.get("toolUse").and_then(|v| v.as_object()) {
                    let mut buf = Map::new();
                    buf.insert(
                        "id".into(),
                        Value::String(
                            tool_use
                                .get("toolUseId")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
                        ),
                    );
                    buf.insert(
                        "name".into(),
                        Value::String(
                            tool_use
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
                        ),
                    );
                    buf.insert("input".into(), Value::String(String::new()));
                    tool_buffers.insert(idx, buf);
                    if let Some(ref cb) = on_tool_call_delta {
                        let id = tool_use
                            .get("toolUseId")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let name = tool_use
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let mut delta = serde_json::Map::new();
                        delta.insert("call_id".into(), Value::String(id));
                        delta.insert("name".into(), Value::String(name));
                        delta.insert("arguments_delta".into(), Value::String(String::new()));
                        cb(delta);
                    }
                }
            }
            return None;
        }

        if let Some(data) = event.get("contentBlockDelta").and_then(|v| v.as_object()) {
            let idx = data
                .get("contentBlockIndex")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize;
            if let Some(delta) = data.get("delta").and_then(|v| v.as_object()) {
                if let Some(text) = delta.get("text").and_then(|v| v.as_str()) {
                    content_parts.push(text.to_string());
                    return Some(text.to_string());
                }
                if let Some(tool_delta) = delta.get("toolUse").and_then(|v| v.as_object()) {
                    let buf = tool_buffers.entry(idx).or_insert_with(|| {
                        let mut m = Map::new();
                        m.insert("id".into(), Value::String(String::new()));
                        m.insert("name".into(), Value::String(String::new()));
                        m.insert("input".into(), Value::String(String::new()));
                        m
                    });
                    if let Some(input_delta) = tool_delta.get("input").and_then(|v| v.as_str()) {
                        if let Some(Value::String(existing)) = buf.get_mut("input") {
                            existing.push_str(input_delta);
                        }
                    }
                }
                if let Some(reasoning) = delta.get("reasoningContent").and_then(|v| v.as_object()) {
                    let buf: &mut Map<String, Value> =
                        reasoning_buffers.entry(idx).or_insert_with(|| {
                            let mut m = Map::new();
                            m.insert("text".into(), Value::String(String::new()));
                            m.insert("signature".into(), Value::String(String::new()));
                            m.insert("redactedContent".into(), Value::Null);
                            m
                        });
                    if let Some(text) = reasoning.get("text").and_then(|v| v.as_str()) {
                        if let Some(Value::String(existing)) = buf.get_mut("text") {
                            existing.push_str(text);
                        }
                        reasoning_parts.push(text.to_string());
                    }
                    if let Some(sig) = reasoning.get("signature").and_then(|v| v.as_str()) {
                        buf.insert("signature".into(), Value::String(sig.to_string()));
                    }
                    if reasoning.contains_key("redactedContent") {
                        buf.insert(
                            "redactedContent".into(),
                            reasoning["redactedContent"].clone(),
                        );
                    }
                }
            }
            return None;
        }

        if let Some(data) = event.get("contentBlockStop").and_then(|v| v.as_object()) {
            let idx = data
                .get("contentBlockIndex")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize;
            if let Some(buf) = reasoning_buffers.remove(&idx) {
                if let Some(Value::String(text)) = buf.get("text") {
                    if !text.is_empty() {
                        let signature = buf
                            .get("signature")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        thinking_blocks.push(json!({
                            "type": "thinking",
                            "thinking": text,
                            "signature": signature,
                        }));
                    }
                } else if let Some(redacted) = buf.get("redactedContent").filter(|v| !v.is_null()) {
                    thinking_blocks.push(json!({
                        "type": "redacted_thinking",
                        "redactedContent": redacted,
                    }));
                }
            }
            return None;
        }

        if let Some(data) = event.get("messageStop").and_then(|v| v.as_object()) {
            if let Some(stop_reason) = data.get("stopReason").and_then(|v| v.as_str()) {
                state.insert("stop_reason".into(), Value::String(stop_reason.to_string()));
            }
            return None;
        }

        if let Some(metadata) = event.get("metadata").and_then(|v| v.as_object()) {
            if let Some(usage) = metadata.get("usage") {
                state.insert("usage".into(), usage.clone());
            }
            return None;
        }

        None
    }

    fn stream_result(
        content_parts: &[String],
        reasoning_parts: &[String],
        thinking_blocks: &[Value],
        tool_buffers: &HashMap<usize, Map<String, Value>>,
        state: &Map<String, Value>,
    ) -> LLMResponse {
        let mut tool_calls: Vec<ToolCallRequest> = Vec::new();
        for buf in tool_buffers.values() {
            let args: Map<String, Value> = if let Some(Value::String(input)) = buf.get("input") {
                if !input.is_empty() {
                    serde_json::from_str::<Value>(input)
                        .ok()
                        .and_then(|v| v.as_object().cloned())
                        .unwrap_or_default()
                } else {
                    Map::new()
                }
            } else {
                Map::new()
            };
            tool_calls.push(ToolCallRequest {
                id: buf
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                name: buf
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                arguments: args,
                extra_content: None,
                provider_specific_fields: None,
                function_provider_specific_fields: None,
            });
        }
        LLMResponse {
            content: (!content_parts.is_empty()).then(|| content_parts.join("")),
            tool_calls,
            finish_reason: finish_reason(state.get("stop_reason").and_then(|v| v.as_str())),
            usage: usage_map(state.get("usage")),
            reasoning_content: (!reasoning_parts.is_empty()).then(|| reasoning_parts.join("")),
            thinking_blocks: (!thinking_blocks.is_empty()).then_some(thinking_blocks.to_vec()),
            ..Default::default()
        }
    }

    fn handle_error(e: &reqwest::Error, status: Option<u16>, body: Option<&str>) -> LLMResponse {
        let status_i32 = status.map(|s| s as i32);
        let body_text = body.unwrap_or("");
        let msg = if body_text.trim().is_empty() {
            format!("Error calling AWS Bedrock: {e}")
        } else {
            format!(
                "Error: {}",
                body_text.trim().chars().take(500).collect::<String>()
            )
        };
        let retry_after = extract_retry_after_from_text(Some(&msg));

        let error_name = e.to_string().to_ascii_lowercase();
        let error_kind = if error_name.contains("timeout") {
            Some("timeout".into())
        } else if error_name.contains("connection") || error_name.contains("connect") {
            Some("connection".into())
        } else {
            None
        };

        let should_retry = status_i32.map(|s| s == 429 || s >= 500);

        LLMResponse {
            content: Some(msg),
            finish_reason: "error".into(),
            retry_after,
            error_status_code: status_i32,
            error_kind,
            error_retry_after_s: retry_after,
            error_should_retry: should_retry,
            ..Default::default()
        }
    }
}

#[async_trait]
impl LLMProvider for BedrockProvider {
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
        let (model_id, kwargs) = self.build_kwargs(&req);
        let url = self.converse_url(&model_id);

        let resp = match self.client.post(&url).json(&kwargs).send().await {
            Ok(r) => r,
            Err(e) => return Self::handle_error(&e, None, None),
        };

        let status = resp.status().as_u16();
        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            warn!("Bedrock error {status}: {text}");
            let msg = if text.trim().is_empty() {
                format!("Error calling AWS Bedrock: HTTP {status}")
            } else {
                format!(
                    "Error: {}",
                    text.trim().chars().take(500).collect::<String>()
                )
            };
            return LLMResponse {
                content: Some(msg),
                finish_reason: "error".into(),
                error_status_code: Some(status as i32),
                error_retry_after_s: extract_retry_after_from_text(Some(&text)),
                error_should_retry: Some(status == 429 || status >= 500),
                ..Default::default()
            };
        }

        let body = match resp.json::<Value>().await {
            Ok(b) => b,
            Err(e) => return LLMResponse::error(format!("Error parsing Bedrock response: {e}")),
        };
        parse_response(&body)
    }

    async fn chat_stream(
        &self,
        req: ChatRequest,
        on_delta: Option<StreamDeltaCallback>,
        on_tool_call_delta: Option<crate::responses::ToolCallDeltaCallback>,
    ) -> LLMResponse {
        let (model_id, kwargs) = self.build_kwargs(&req);
        let url = self.converse_stream_url(&model_id);

        let resp = match self.client.post(&url).json(&kwargs).send().await {
            Ok(r) => r,
            Err(e) => return Self::handle_error(&e, None, None),
        };

        let status = resp.status().as_u16();
        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            warn!("Bedrock stream error {status}: {text}");
            let msg = if text.trim().is_empty() {
                format!("Error calling AWS Bedrock: HTTP {status}")
            } else {
                format!(
                    "Error: {}",
                    text.trim().chars().take(500).collect::<String>()
                )
            };
            return LLMResponse {
                content: Some(msg),
                finish_reason: "error".into(),
                error_status_code: Some(status as i32),
                error_retry_after_s: extract_retry_after_from_text(Some(&text)),
                error_should_retry: Some(status == 429 || status >= 500),
                ..Default::default()
            };
        }

        let body_text = match resp.text().await {
            Ok(t) => t,
            Err(e) => return LLMResponse::error(format!("Error reading Bedrock stream: {e}")),
        };

        let events = crate::responses::parse_sse_events(&body_text);

        let mut content_parts: Vec<String> = Vec::new();
        let mut reasoning_parts: Vec<String> = Vec::new();
        let mut thinking_blocks: Vec<Value> = Vec::new();
        let mut tool_buffers: HashMap<usize, Map<String, Value>> = HashMap::new();
        let mut reasoning_buffers: HashMap<usize, Map<String, Value>> = HashMap::new();
        let mut state: Map<String, Value> = Map::new();

        for event in &events {
            if let Some(delta) = Self::parse_stream_event(
                event,
                &mut content_parts,
                &mut reasoning_parts,
                &mut thinking_blocks,
                &mut tool_buffers,
                &mut reasoning_buffers,
                &mut state,
                on_tool_call_delta.as_ref(),
            ) {
                if let Some(cb) = &on_delta {
                    cb(delta);
                }
            }
        }

        Self::stream_result(
            &content_parts,
            &reasoning_parts,
            &thinking_blocks,
            &tool_buffers,
            &state,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_bedrock_prefix() {
        assert_eq!(strip_prefix("bedrock/foo"), "foo");
        assert_eq!(strip_prefix("foo"), "foo");
    }

    #[test]
    fn supports_temperature_check() {
        assert!(supports_temperature("anthropic.claude-3-5-sonnet"));
        assert!(!supports_temperature("global.anthropic.claude-opus-4-7"));
    }

    #[test]
    fn finish_reason_mapping() {
        assert_eq!(finish_reason(Some("end_turn")), "stop");
        assert_eq!(finish_reason(Some("tool_use")), "tool_calls");
        assert_eq!(finish_reason(Some("max_tokens")), "length");
        assert_eq!(
            finish_reason(Some("guardrail_intervened")),
            "guardrail_intervened"
        );
    }

    #[test]
    fn tool_choice_auto() {
        assert_eq!(
            convert_tool_choice(Some(&ToolChoice::Auto)),
            Some(json!({"auto": {}}))
        );
    }

    #[test]
    fn tool_choice_required() {
        assert_eq!(
            convert_tool_choice(Some(&ToolChoice::Required)),
            Some(json!({"any": {}}))
        );
    }

    #[test]
    fn tool_choice_none() {
        assert_eq!(convert_tool_choice(Some(&ToolChoice::None)), None);
    }

    #[test]
    fn content_blocks_string() {
        let blocks = content_blocks(&Value::String("hello".into()), false);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0]["text"], "hello");
    }

    #[test]
    fn content_blocks_empty_string() {
        let blocks = content_blocks(&Value::String("".into()), false);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0]["text"], "(empty)");
    }
}
