//! Convert Chat Completions messages/tools to Responses API format.
//!
//! Port of `nanobot.providers.openai_responses.converters`.

use serde_json::{Value, json};

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
        let item = if b.is_empty() {
            None
        } else {
            Some(b.to_string())
        };
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
                let Some(obj) = item.as_object() else {
                    continue;
                };
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
                        let (call_id, item_id) =
                            split_tool_call_id(tc.get("id").and_then(|v| v.as_str()));
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
                let (call_id, _) =
                    split_tool_call_id(msg.get("tool_call_id").and_then(|v| v.as_str()));
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
