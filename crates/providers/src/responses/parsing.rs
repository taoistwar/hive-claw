//! Parse Responses API SSE streams and SDK response objects.
//!
//! Port of `nanobot.providers.openai_responses.parsing`.

use std::collections::HashMap;

use log::warn;
use serde_json::Value;

use crate::base::{LLMResponse, ToolCallRequest};
use crate::responses::converters::map_finish_reason;

/// Minimal SSE parser: yields one JSON event per blank-line-delimited chunk.
pub fn parse_sse_events(input: &str) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    let mut buffer: Vec<String> = Vec::new();

    let flush = |buffer: &mut Vec<String>, out: &mut Vec<Value>| {
        let data_lines: Vec<String> = buffer
            .iter()
            .filter(|l| l.starts_with("data:"))
            .map(|l| l[5..].trim().to_string())
            .collect();
        buffer.clear();
        if data_lines.is_empty() {
            return;
        }
        let data = data_lines.join("\n").trim().to_string();
        if data.is_empty() || data == "[DONE]" {
            return;
        }
        if let Ok(v) = serde_json::from_str::<Value>(&data) {
            out.push(v);
        }
    };

    for line in input.split('\n') {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            flush(&mut buffer, &mut out);
            continue;
        }
        buffer.push(line.to_string());
    }
    flush(&mut buffer, &mut out);
    out
}

/// Consume a sequence of already-decoded SSE events into a final response.
pub fn consume_events(events: &[Value]) -> Result<(String, Vec<ToolCallRequest>, String), String> {
    let mut content = String::new();
    let mut tool_call_buffers: HashMap<String, (String, String, String)> = HashMap::new();
    let mut tool_calls: Vec<ToolCallRequest> = Vec::new();
    let mut finish_reason = "stop".to_string();

    for event in events {
        let ty = event.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match ty {
            "response.output_item.added" => {
                let Some(item) = event.get("item").and_then(|v| v.as_object()) else {
                    continue;
                };
                if item.get("type").and_then(|v| v.as_str()) != Some("function_call") {
                    continue;
                }
                let Some(call_id) = item.get("call_id").and_then(|v| v.as_str()) else {
                    continue;
                };
                let id = item
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("fc_0")
                    .to_string();
                let name = item
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let arguments = item
                    .get("arguments")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                tool_call_buffers.insert(call_id.to_string(), (id, name, arguments));
            }
            "response.output_text.delta" => {
                if let Some(delta) = event.get("delta").and_then(|v| v.as_str()) {
                    content.push_str(delta);
                }
            }
            "response.function_call_arguments.delta" => {
                let call_id = event.get("call_id").and_then(|v| v.as_str());
                let delta = event.get("delta").and_then(|v| v.as_str()).unwrap_or("");
                if let Some(buf) = call_id.and_then(|id| tool_call_buffers.get_mut(id)) {
                    buf.2.push_str(delta);
                }
            }
            "response.function_call_arguments.done" => {
                let call_id = event.get("call_id").and_then(|v| v.as_str());
                let args = event
                    .get("arguments")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if let Some(buf) = call_id.and_then(|id| tool_call_buffers.get_mut(id)) {
                    buf.2 = args;
                }
            }
            "response.output_item.done" => {
                let Some(item) = event.get("item").and_then(|v| v.as_object()) else {
                    continue;
                };
                if item.get("type").and_then(|v| v.as_str()) != Some("function_call") {
                    continue;
                }
                let Some(call_id) = item.get("call_id").and_then(|v| v.as_str()) else {
                    continue;
                };
                let (item_id, name, args_accum) =
                    tool_call_buffers.remove(call_id).unwrap_or_else(|| {
                        (
                            item.get("id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("fc_0")
                                .to_string(),
                            item.get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
                            item.get("arguments")
                                .and_then(|v| v.as_str())
                                .unwrap_or("{}")
                                .to_string(),
                        )
                    });
                let args_raw = if args_accum.is_empty() {
                    "{}".into()
                } else {
                    args_accum
                };
                let args = match serde_json::from_str::<Value>(&args_raw) {
                    Ok(Value::Object(m)) => m,
                    _ => {
                        warn!(
                            "Failed to parse tool call arguments for '{}': {}",
                            name,
                            &args_raw[..args_raw.len().min(200)],
                        );
                        let mut m = serde_json::Map::new();
                        m.insert("raw".into(), Value::String(args_raw.clone()));
                        m
                    }
                };
                tool_calls.push(ToolCallRequest {
                    id: format!("{call_id}|{item_id}"),
                    name,
                    arguments: args,
                    extra_content: None,
                    provider_specific_fields: None,
                    function_provider_specific_fields: None,
                });
            }
            "response.completed" => {
                let status = event
                    .get("response")
                    .and_then(|r| r.get("status"))
                    .and_then(|v| v.as_str());
                finish_reason = map_finish_reason(status);
            }
            "error" | "response.failed" => {
                let detail = event
                    .get("error")
                    .or_else(|| event.get("message"))
                    .cloned()
                    .unwrap_or_else(|| event.clone());
                let text = serde_json::to_string(&detail).unwrap_or_default();
                let preview: String = text.chars().take(500).collect();
                return Err(format!("Response failed: {preview}"));
            }
            _ => {}
        }
    }
    Ok((content, tool_calls, finish_reason))
}

/// Parse a non-streaming Responses API `Response` payload.
pub fn parse_response_output(response: &Value) -> LLMResponse {
    let output = response
        .get("output")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let mut content_parts: Vec<String> = Vec::new();
    let mut tool_calls: Vec<ToolCallRequest> = Vec::new();
    let mut reasoning_content: Option<String> = None;

    for item in output {
        let Some(obj) = item.as_object() else {
            continue;
        };
        match obj.get("type").and_then(|v| v.as_str()) {
            Some("message") => {
                if let Some(blocks) = obj.get("content").and_then(|v| v.as_array()) {
                    for block in blocks {
                        if block.get("type").and_then(|v| v.as_str()) != Some("output_text") {
                            continue;
                        }
                        if let Some(t) = block.get("text").and_then(|v| v.as_str()) {
                            content_parts.push(t.to_string());
                        }
                    }
                }
            }
            Some("reasoning") => {
                if let Some(summary) = obj.get("summary").and_then(|v| v.as_array()) {
                    for s in summary {
                        if s.get("type").and_then(|v| v.as_str()) != Some("summary_text") {
                            continue;
                        }
                        if let Some(text) = s.get("text").and_then(|v| v.as_str()) {
                            let entry = reasoning_content.get_or_insert_with(String::new);
                            entry.push_str(text);
                        }
                    }
                }
            }
            Some("function_call") => {
                let call_id = obj
                    .get("call_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let item_id = obj
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("fc_0")
                    .to_string();
                let args_raw = obj
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| Value::String("{}".into()));
                let args = match args_raw {
                    Value::String(s) => match serde_json::from_str::<Value>(&s) {
                        Ok(Value::Object(m)) => m,
                        _ => {
                            warn!(
                                "Failed to parse tool call arguments for '{}': {}",
                                obj.get("name").and_then(|v| v.as_str()).unwrap_or(""),
                                &s[..s.len().min(200)],
                            );
                            let mut m = serde_json::Map::new();
                            m.insert("raw".into(), Value::String(s));
                            m
                        }
                    },
                    Value::Object(m) => m,
                    _ => serde_json::Map::new(),
                };
                tool_calls.push(ToolCallRequest {
                    id: format!("{call_id}|{item_id}"),
                    name: obj
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
            _ => {}
        }
    }

    let mut usage: HashMap<String, i64> = HashMap::new();
    if let Some(u) = response.get("usage").and_then(|v| v.as_object()) {
        let as_i = |k: &str| u.get(k).and_then(|v| v.as_i64()).unwrap_or(0);
        usage.insert("prompt_tokens".into(), as_i("input_tokens"));
        usage.insert("completion_tokens".into(), as_i("output_tokens"));
        usage.insert("total_tokens".into(), as_i("total_tokens"));
    }

    let finish_reason = map_finish_reason(response.get("status").and_then(|v| v.as_str()));

    LLMResponse {
        content: (!content_parts.is_empty()).then(|| content_parts.join("")),
        tool_calls,
        finish_reason,
        usage,
        reasoning_content,
        ..Default::default()
    }
}

/// Callback for content delta during SSE consumption.
pub type ContentDeltaCallback = std::sync::Arc<dyn Fn(String) + Send + Sync>;

/// Callback for tool-call delta during SSE consumption.
pub type ToolCallDeltaCallback =
    std::sync::Arc<dyn Fn(serde_json::Map<String, Value>) + Send + Sync>;

/// Consume a Responses API SSE stream (from `reqwest::Response` lines)
/// into `(content, tool_calls, finish_reason)`.
///
/// NOTE: This is an async function. TODO: integrate with actual reqwest
/// streaming. Here we provide a sync version that works on pre-collected
/// SSE text for now.
pub async fn consume_sse(
    sse_text: &str,
    on_content_delta: Option<ContentDeltaCallback>,
    on_tool_call_delta: Option<ToolCallDeltaCallback>,
) -> (String, Vec<ToolCallRequest>, String) {
    let events = parse_sse_events(sse_text);

    let mut content = String::new();
    let mut tool_call_buffers: HashMap<String, (String, String, String)> = HashMap::new();
    let mut tool_calls: Vec<ToolCallRequest> = Vec::new();
    let mut finish_reason = "stop".to_string();

    for event in &events {
        let ty = event.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match ty {
            "response.output_item.added" => {
                let Some(item) = event.get("item").and_then(|v| v.as_object()) else {
                    continue;
                };
                if item.get("type").and_then(|v| v.as_str()) != Some("function_call") {
                    continue;
                }
                let Some(call_id) = item.get("call_id").and_then(|v| v.as_str()) else {
                    continue;
                };
                let id = item
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("fc_0")
                    .to_string();
                let name = item
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let arguments = item
                    .get("arguments")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                tool_call_buffers.insert(call_id.to_string(), (id, name.clone(), arguments));
                if let Some(cb) = &on_tool_call_delta {
                    let mut delta = serde_json::Map::new();
                    delta.insert("call_id".into(), Value::String(call_id.to_string()));
                    delta.insert("name".into(), Value::String(name));
                    delta.insert("arguments_delta".into(), Value::String(String::new()));
                    cb(delta);
                }
            }
            "response.output_text.delta" => {
                let delta_text = event
                    .get("delta")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                content.push_str(&delta_text);
                if let Some(cb) = on_content_delta.as_ref().filter(|_| !delta_text.is_empty()) {
                    cb(delta_text);
                }
            }
            "response.function_call_arguments.delta" => {
                let call_id = event.get("call_id").and_then(|v| v.as_str());
                let delta = event.get("delta").and_then(|v| v.as_str()).unwrap_or("");
                if let Some((id, buf)) =
                    call_id.and_then(|id| tool_call_buffers.get_mut(id).map(|buf| (id, buf)))
                {
                    buf.2.push_str(delta);
                    if let Some(cb) = &on_tool_call_delta {
                        let mut map_delta = serde_json::Map::new();
                        map_delta.insert("call_id".into(), Value::String(id.to_string()));
                        map_delta.insert("name".into(), Value::String(buf.1.clone()));
                        map_delta
                            .insert("arguments_delta".into(), Value::String(delta.to_string()));
                        cb(map_delta);
                    }
                }
            }
            "response.function_call_arguments.done" => {
                let call_id = event.get("call_id").and_then(|v| v.as_str());
                let args = event
                    .get("arguments")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if let Some(buf) = call_id.and_then(|id| tool_call_buffers.get_mut(id)) {
                    buf.2 = args;
                }
            }
            "response.output_item.done" => {
                let Some(item) = event.get("item").and_then(|v| v.as_object()) else {
                    continue;
                };
                if item.get("type").and_then(|v| v.as_str()) != Some("function_call") {
                    continue;
                }
                let Some(call_id) = item.get("call_id").and_then(|v| v.as_str()) else {
                    continue;
                };
                let (item_id, name, args_accum) =
                    tool_call_buffers.remove(call_id).unwrap_or_else(|| {
                        (
                            item.get("id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("fc_0")
                                .to_string(),
                            item.get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
                            item.get("arguments")
                                .and_then(|v| v.as_str())
                                .unwrap_or("{}")
                                .to_string(),
                        )
                    });
                let args_raw = if args_accum.is_empty() {
                    "{}".into()
                } else {
                    args_accum
                };
                let args = match serde_json::from_str::<Value>(&args_raw) {
                    Ok(Value::Object(m)) => m,
                    Ok(_) => {
                        let mut m = serde_json::Map::new();
                        m.insert("raw".into(), Value::String(args_raw.clone()));
                        m
                    }
                    Err(_) => {
                        // TODO: use json_repair equivalent here
                        warn!(
                            "Failed to parse tool call arguments for '{}': {}",
                            name,
                            &args_raw[..args_raw.len().min(200)],
                        );
                        let mut m = serde_json::Map::new();
                        m.insert("raw".into(), Value::String(args_raw.clone()));
                        m
                    }
                };
                tool_calls.push(ToolCallRequest {
                    id: format!("{call_id}|{item_id}"),
                    name,
                    arguments: args,
                    extra_content: None,
                    provider_specific_fields: None,
                    function_provider_specific_fields: None,
                });
            }
            "response.completed" => {
                let status = event
                    .get("response")
                    .and_then(|r| r.get("status"))
                    .and_then(|v| v.as_str());
                finish_reason = map_finish_reason(status);
            }
            "error" | "response.failed" => {
                let detail = event
                    .get("error")
                    .or_else(|| event.get("message"))
                    .cloned()
                    .unwrap_or_else(|| event.clone());
                let text = serde_json::to_string(&detail).unwrap_or_default();
                let preview: String = text.chars().take(500).collect();
                // In async context this would be a panic or error return.
                // For now, set finish_reason to error.
                finish_reason = "error".to_string();
                warn!("Response failed: {}", preview);
            }
            _ => {}
        }
    }

    (content, tool_calls, finish_reason)
}

/// Consume an SDK async stream from `client.responses.create(stream=True)`.
///
/// NOTE: The Python version consumes `AsyncGenerator[event]`. In Rust the
/// SDK stream shape depends on the actual OpenAI SDK. This is a placeholder
/// that works on a pre-collected slice of event Values.
/// TODO: integrate with actual OpenAI Rust SDK stream types.
pub async fn consume_sdk_stream(
    events: &[Value],
    on_content_delta: Option<ContentDeltaCallback>,
    on_tool_call_delta: Option<ToolCallDeltaCallback>,
) -> (
    String,
    Vec<ToolCallRequest>,
    String,
    HashMap<String, i64>,
    Option<String>,
) {
    let mut content = String::new();
    let mut tool_call_buffers: HashMap<String, (String, String, String)> = HashMap::new();
    let mut tool_calls: Vec<ToolCallRequest> = Vec::new();
    let mut finish_reason = "stop".to_string();
    let mut usage: HashMap<String, i64> = HashMap::new();
    let mut reasoning_content: Option<String> = None;

    for event in events {
        let ty = event.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match ty {
            "response.output_item.added" => {
                let Some(item) = event.get("item").and_then(|v| v.as_object()) else {
                    continue;
                };
                if item.get("type").and_then(|v| v.as_str()) != Some("function_call") {
                    continue;
                }
                let Some(call_id) = item.get("call_id").and_then(|v| v.as_str()) else {
                    continue;
                };
                let id = item
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("fc_0")
                    .to_string();
                let name = item
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let arguments = item
                    .get("arguments")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                tool_call_buffers.insert(call_id.to_string(), (id, name.clone(), arguments));
                if let Some(cb) = &on_tool_call_delta {
                    let mut delta = serde_json::Map::new();
                    delta.insert("call_id".into(), Value::String(call_id.to_string()));
                    delta.insert("name".into(), Value::String(name));
                    delta.insert("arguments_delta".into(), Value::String(String::new()));
                    cb(delta);
                }
            }
            "response.output_text.delta" => {
                let delta_text = event
                    .get("delta")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                content.push_str(&delta_text);
                if let Some(cb) = on_content_delta.as_ref().filter(|_| !delta_text.is_empty()) {
                    cb(delta_text);
                }
            }
            "response.function_call_arguments.delta" => {
                let call_id = event.get("call_id").and_then(|v| v.as_str());
                let delta = event.get("delta").and_then(|v| v.as_str()).unwrap_or("");
                if let Some((id, buf)) =
                    call_id.and_then(|id| tool_call_buffers.get_mut(id).map(|buf| (id, buf)))
                {
                    buf.2.push_str(delta);
                    if let Some(cb) = &on_tool_call_delta {
                        let mut map_delta = serde_json::Map::new();
                        map_delta.insert("call_id".into(), Value::String(id.to_string()));
                        map_delta.insert("name".into(), Value::String(buf.1.clone()));
                        map_delta
                            .insert("arguments_delta".into(), Value::String(delta.to_string()));
                        cb(map_delta);
                    }
                }
            }
            "response.function_call_arguments.done" => {
                let call_id = event.get("call_id").and_then(|v| v.as_str());
                let args = event
                    .get("arguments")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if let Some(buf) = call_id.and_then(|id| tool_call_buffers.get_mut(id)) {
                    buf.2 = args;
                }
            }
            "response.output_item.done" => {
                let Some(item) = event.get("item").and_then(|v| v.as_object()) else {
                    continue;
                };
                if item.get("type").and_then(|v| v.as_str()) != Some("function_call") {
                    continue;
                }
                let Some(call_id) = item.get("call_id").and_then(|v| v.as_str()) else {
                    continue;
                };
                let (item_id, name, args_accum) =
                    tool_call_buffers.remove(call_id).unwrap_or_else(|| {
                        (
                            item.get("id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("fc_0")
                                .to_string(),
                            item.get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
                            item.get("arguments")
                                .and_then(|v| v.as_str())
                                .unwrap_or("{}")
                                .to_string(),
                        )
                    });
                let args_raw = if args_accum.is_empty() {
                    "{}".into()
                } else {
                    args_accum
                };
                let args = match serde_json::from_str::<Value>(&args_raw) {
                    Ok(Value::Object(m)) => m,
                    _ => {
                        // TODO: use json_repair equivalent here
                        warn!(
                            "Failed to parse tool call arguments for '{}': {}",
                            name,
                            &args_raw[..args_raw.len().min(200)],
                        );
                        let mut m = serde_json::Map::new();
                        m.insert("raw".into(), Value::String(args_raw.clone()));
                        m
                    }
                };
                tool_calls.push(ToolCallRequest {
                    id: format!("{call_id}|{item_id}"),
                    name,
                    arguments: args,
                    extra_content: None,
                    provider_specific_fields: None,
                    function_provider_specific_fields: None,
                });
            }
            "response.completed" => {
                let resp = event.get("response");
                let status = resp.and_then(|r| r.get("status")).and_then(|v| v.as_str());
                finish_reason = map_finish_reason(status);

                if let Some(resp_obj) = resp.and_then(|v| v.as_object()) {
                    if let Some(usage_obj) = resp_obj.get("usage").and_then(|v| v.as_object()) {
                        let as_i = |k: &str| usage_obj.get(k).and_then(|v| v.as_i64()).unwrap_or(0);
                        usage.insert("prompt_tokens".into(), as_i("input_tokens"));
                        usage.insert("completion_tokens".into(), as_i("output_tokens"));
                        usage.insert("total_tokens".into(), as_i("total_tokens"));
                    }

                    if let Some(output_arr) = resp_obj.get("output").and_then(|v| v.as_array()) {
                        for out_item in output_arr {
                            if out_item.get("type").and_then(|v| v.as_str()) != Some("reasoning") {
                                continue;
                            }
                            let Some(summary) = out_item.get("summary").and_then(|v| v.as_array())
                            else {
                                continue;
                            };
                            for s in summary {
                                if s.get("type").and_then(|v| v.as_str()) != Some("summary_text") {
                                    continue;
                                }
                                if let Some(text) = s.get("text").and_then(|v| v.as_str()) {
                                    let entry = reasoning_content.get_or_insert_with(String::new);
                                    entry.push_str(text);
                                }
                            }
                        }
                    }
                }
            }
            "error" | "response.failed" => {
                let detail = event
                    .get("error")
                    .or_else(|| event.get("message"))
                    .cloned()
                    .unwrap_or_else(|| event.clone());
                let text = serde_json::to_string(&detail).unwrap_or_default();
                let preview: String = text.chars().take(500).collect();
                finish_reason = "error".to_string();
                warn!("Response failed: {}", preview);
            }
            _ => {}
        }
    }

    (content, tool_calls, finish_reason, usage, reasoning_content)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_sse_events_basic() {
        let body = "event: a\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"hi\"}\n\ndata: [DONE]\n\n";
        let evs = parse_sse_events(body);
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0]["type"], "response.output_text.delta");
    }

    #[test]
    fn consume_events_assembles_content_and_tool_calls() {
        let evs = vec![
            json!({
                "type":"response.output_item.added",
                "item":{"type":"function_call","call_id":"c1","id":"fc1","name":"x","arguments":""}
            }),
            json!({
                "type":"response.function_call_arguments.delta",
                "call_id":"c1","delta":"{\"a\":"
            }),
            json!({
                "type":"response.function_call_arguments.delta",
                "call_id":"c1","delta":"1}"
            }),
            json!({
                "type":"response.output_item.done",
                "item":{"type":"function_call","call_id":"c1"}
            }),
            json!({"type":"response.output_text.delta","delta":"hello"}),
            json!({"type":"response.completed","response":{"status":"completed"}}),
        ];
        let (content, tcs, fr) = consume_events(&evs).unwrap();
        assert_eq!(content, "hello");
        assert_eq!(tcs.len(), 1);
        assert_eq!(tcs[0].name, "x");
        assert_eq!(tcs[0].arguments.get("a").and_then(|v| v.as_i64()), Some(1));
        assert_eq!(fr, "stop");
    }

    #[test]
    fn parse_response_output_roundtrip() {
        let body = json!({
            "status":"completed",
            "output":[
                {"type":"message","content":[{"type":"output_text","text":"hi"}]},
                {"type":"function_call","call_id":"c1","id":"fc1","name":"x","arguments":"{\"a\":1}"},
                {"type":"reasoning","summary":[{"type":"summary_text","text":"because"}]}
            ],
            "usage":{"input_tokens":5,"output_tokens":2,"total_tokens":7}
        });
        let out = parse_response_output(&body);
        assert_eq!(out.content.as_deref(), Some("hi"));
        assert_eq!(out.tool_calls.len(), 1);
        assert_eq!(out.reasoning_content.as_deref(), Some("because"));
        assert_eq!(out.usage["total_tokens"], 7);
    }
}
