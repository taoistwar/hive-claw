//! OpenAI Responses API helpers — converters + SSE stream parsing.
//!
//! Port of `nanobot.providers.openai_responses`. Used by the Codex
//! provider and any future Responses-API-backed backend.

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::types::{LLMResponse, ToolCallRequest};

/// Map a Responses API status to a Chat-Completions-style finish reason.
pub fn map_finish_reason(status: Option<&str>) -> String {
    match status.unwrap_or("completed") {
        "completed" => "stop",
        "incomplete" => "length",
        "failed" | "cancelled" => "error",
        other => other,
    }
    .to_string()
}

/// Split a compound `call_id|item_id` string. Returns `(call_id, item_id)`.
pub fn split_tool_call_id(tool_call_id: Option<&str>) -> (String, Option<String>) {
    let Some(id) = tool_call_id.filter(|s| !s.is_empty()) else {
        return ("call_0".to_string(), None);
    };
    if let Some((a, b)) = id.split_once('|') {
        let item = if b.is_empty() { None } else { Some(b.to_string()) };
        return (a.to_string(), item);
    }
    (id.to_string(), None)
}

/// Convert a single Chat Completions user message content into a
/// Responses API user input item.
pub fn convert_user_message(content: &Value) -> Value {
    match content {
        Value::String(s) => json!({
            "role":"user",
            "content":[{"type":"input_text","text":s}]
        }),
        Value::Array(items) => {
            let mut converted: Vec<Value> = Vec::new();
            for item in items {
                let Some(obj) = item.as_object() else { continue };
                match obj.get("type").and_then(|v| v.as_str()) {
                    Some("text") => {
                        let text = obj.get("text").and_then(|v| v.as_str()).unwrap_or("");
                        converted.push(json!({"type":"input_text","text":text}));
                    }
                    Some("image_url") => {
                        if let Some(url) = obj
                            .get("image_url")
                            .and_then(|v| v.get("url"))
                            .and_then(|v| v.as_str())
                        {
                            converted.push(json!({
                                "type":"input_image",
                                "image_url":url,
                                "detail":"auto"
                            }));
                        }
                    }
                    _ => {}
                }
            }
            if !converted.is_empty() {
                json!({"role":"user","content":converted})
            } else {
                json!({"role":"user","content":[{"type":"input_text","text":""}]})
            }
        }
        _ => json!({"role":"user","content":[{"type":"input_text","text":""}]}),
    }
}

/// Convert an OpenAI Chat Completions message list to a Responses API
/// `(system, input)` pair.
pub fn convert_messages(messages: &[Value]) -> (String, Vec<Value>) {
    let mut system = String::new();
    let mut input: Vec<Value> = Vec::new();

    for (idx, msg) in messages.iter().enumerate() {
        let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("");
        let content = msg.get("content").cloned().unwrap_or(Value::Null);
        match role {
            "system" => {
                if let Some(s) = content.as_str() {
                    system = s.to_string();
                }
            }
            "user" => input.push(convert_user_message(&content)),
            "assistant" => {
                if let Some(s) = content.as_str() {
                    if !s.is_empty() {
                        input.push(json!({
                            "type":"message",
                            "role":"assistant",
                            "content":[{"type":"output_text","text":s}],
                            "status":"completed",
                            "id": format!("msg_{idx}"),
                        }));
                    }
                }
                if let Some(tcs) = msg.get("tool_calls").and_then(|v| v.as_array()) {
                    for tc in tcs {
                        let fn_obj = tc.get("function").cloned().unwrap_or(Value::Null);
                        let (call_id, item_id) = split_tool_call_id(
                            tc.get("id").and_then(|v| v.as_str()),
                        );
                        let item_id_out = item_id.unwrap_or_else(|| format!("fc_{idx}"));
                        let call_id_out = if call_id.is_empty() || call_id == "call_0" {
                            format!("call_{idx}")
                        } else {
                            call_id
                        };
                        let arguments = fn_obj
                            .get("arguments")
                            .cloned()
                            .unwrap_or_else(|| Value::String("{}".into()));
                        let name = fn_obj
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        input.push(json!({
                            "type":"function_call",
                            "id": item_id_out,
                            "call_id": call_id_out,
                            "name": name,
                            "arguments": arguments,
                        }));
                    }
                }
            }
            "tool" => {
                let (call_id, _) = split_tool_call_id(
                    msg.get("tool_call_id").and_then(|v| v.as_str()),
                );
                let output_text = match &content {
                    Value::String(s) => s.clone(),
                    other => serde_json::to_string(other).unwrap_or_default(),
                };
                input.push(json!({
                    "type":"function_call_output",
                    "call_id": call_id,
                    "output": output_text,
                }));
            }
            _ => {}
        }
    }
    (system, input)
}

/// Convert Chat Completions tools to Responses API flat format.
pub fn convert_tools(tools: &[Value]) -> Vec<Value> {
    let mut out = Vec::with_capacity(tools.len());
    for tool in tools {
        let fn_value: &Value = if tool.get("type").and_then(|v| v.as_str()) == Some("function") {
            tool.get("function").unwrap_or(tool)
        } else {
            tool
        };
        let Some(name) = fn_value.get("name").and_then(|v| v.as_str()) else {
            continue;
        };
        if name.is_empty() {
            continue;
        }
        let params = fn_value
            .get("parameters")
            .cloned()
            .unwrap_or_else(|| json!({}));
        out.push(json!({
            "type":"function",
            "name":name,
            "description": fn_value.get("description").and_then(|v| v.as_str()).unwrap_or(""),
            "parameters": if params.is_object() { params } else { json!({}) },
        }));
    }
    out
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
        let Some(obj) = item.as_object() else { continue };
        match obj.get("type").and_then(|v| v.as_str()) {
            Some("message") => {
                if let Some(blocks) = obj.get("content").and_then(|v| v.as_array()) {
                    for block in blocks {
                        if block.get("type").and_then(|v| v.as_str()) == Some("output_text") {
                            if let Some(t) = block.get("text").and_then(|v| v.as_str()) {
                                content_parts.push(t.to_string());
                            }
                        }
                    }
                }
            }
            Some("reasoning") => {
                if let Some(summary) = obj.get("summary").and_then(|v| v.as_array()) {
                    for s in summary {
                        if s.get("type").and_then(|v| v.as_str()) == Some("summary_text") {
                            if let Some(text) = s.get("text").and_then(|v| v.as_str()) {
                                let entry = reasoning_content.get_or_insert_with(String::new);
                                entry.push_str(text);
                            }
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

// ---------------------------------------------------------------------------
// SSE stream consumption
// ---------------------------------------------------------------------------

/// Minimal SSE parser: yields one JSON event per blank-line-delimited chunk.
///
/// Accepts a raw byte stream (as `String`) so callers can plug in either
/// `reqwest::Response::text()` for collected responses or a manually
/// assembled buffer. For live streaming use [`iter_sse_events`] on a
/// byte iterator.
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
        // Normalise CRLF.
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            flush(&mut buffer, &mut out);
            continue;
        }
        buffer.push(line.to_string());
    }
    // Flush any trailing buffer at EOF.
    flush(&mut buffer, &mut out);
    out
}

/// Reduce a sequence of already-decoded SSE events into a final response.
/// Mirrors `consume_sse` / `consume_sdk_stream` in Python.
pub fn consume_events(events: &[Value]) -> Result<(String, Vec<ToolCallRequest>, String), String> {
    let mut content = String::new();
    let mut tool_call_buffers: HashMap<String, (String, String, String)> = HashMap::new();
    // value = (item_id, name, arguments_accum)
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
                tool_call_buffers
                    .insert(call_id.to_string(), (id, name, arguments));
            }
            "response.output_text.delta" => {
                if let Some(delta) = event.get("delta").and_then(|v| v.as_str()) {
                    content.push_str(delta);
                }
            }
            "response.function_call_arguments.delta" => {
                let call_id = event.get("call_id").and_then(|v| v.as_str());
                let delta = event.get("delta").and_then(|v| v.as_str()).unwrap_or("");
                if let Some(id) = call_id {
                    if let Some(buf) = tool_call_buffers.get_mut(id) {
                        buf.2.push_str(delta);
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
                if let Some(id) = call_id {
                    if let Some(buf) = tool_call_buffers.get_mut(id) {
                        buf.2 = args;
                    }
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
                let (item_id, name, args_accum) = tool_call_buffers
                    .remove(call_id)
                    .unwrap_or_else(|| {
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
                let args_raw = if args_accum.is_empty() { "{}".into() } else { args_accum };
                let args = match serde_json::from_str::<Value>(&args_raw) {
                    Ok(Value::Object(m)) => m,
                    _ => {
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn map_finish_reason_basics() {
        assert_eq!(map_finish_reason(Some("completed")), "stop");
        assert_eq!(map_finish_reason(Some("incomplete")), "length");
        assert_eq!(map_finish_reason(Some("failed")), "error");
        assert_eq!(map_finish_reason(None), "stop");
    }

    #[test]
    fn split_ids() {
        assert_eq!(
            split_tool_call_id(Some("call_1|fc_2")),
            ("call_1".into(), Some("fc_2".into()))
        );
        assert_eq!(split_tool_call_id(Some("call_1")), ("call_1".into(), None));
        assert_eq!(split_tool_call_id(None), ("call_0".into(), None));
    }

    #[test]
    fn convert_messages_extracts_system_and_user() {
        let msgs = vec![
            json!({"role":"system","content":"sys"}),
            json!({"role":"user","content":"hi"}),
        ];
        let (sys, input) = convert_messages(&msgs);
        assert_eq!(sys, "sys");
        assert_eq!(input.len(), 1);
        assert_eq!(input[0]["content"][0]["type"], "input_text");
    }

    #[test]
    fn convert_messages_assistant_tool_call() {
        let msgs = vec![
            json!({"role":"user","content":"x"}),
            json!({
                "role":"assistant",
                "content":"",
                "tool_calls":[{
                    "id":"call_42|fc_99",
                    "type":"function",
                    "function":{"name":"foo","arguments":"{\"a\":1}"}
                }]
            }),
        ];
        let (_sys, input) = convert_messages(&msgs);
        assert_eq!(input.len(), 2);
        let fc = &input[1];
        assert_eq!(fc["type"], "function_call");
        assert_eq!(fc["call_id"], "call_42");
        assert_eq!(fc["id"], "fc_99");
        assert_eq!(fc["name"], "foo");
    }

    #[test]
    fn convert_tools_normalises_shape() {
        let tools = vec![json!({
            "type":"function",
            "function":{
                "name":"read_file",
                "description":"read",
                "parameters":{"type":"object"}
            }
        })];
        let out = convert_tools(&tools);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["name"], "read_file");
        assert_eq!(out[0]["description"], "read");
    }

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
