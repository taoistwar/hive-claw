//! Message sanitization utilities (role alternation, empty-content fixup,
//! image stripping). Port of the static helpers in `nanobot.providers.base`.

use serde_json::{json, Map, Value};

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
        .map(|msg| sanitize_one(msg.clone()))
        .collect()
}

fn sanitize_one(mut msg: Value) -> Value {
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn merges_consecutive_users() {
        let msgs = vec![
            json!({"role":"user","content":"a"}),
            json!({"role":"user","content":"b"}),
        ];
        let out = enforce_role_alternation(&msgs);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["content"], "a\n\nb");
    }

    #[test]
    fn drops_trailing_assistant() {
        let msgs = vec![
            json!({"role":"user","content":"q"}),
            json!({"role":"assistant","content":"a"}),
        ];
        let out = enforce_role_alternation(&msgs);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["role"], "user");
    }

    #[test]
    fn recovers_system_only() {
        let msgs = vec![
            json!({"role":"system","content":"sys"}),
            json!({"role":"assistant","content":"hello"}),
        ];
        let out = enforce_role_alternation(&msgs);
        assert_eq!(out.len(), 2);
        assert_eq!(out[1]["role"], "user");
    }

    #[test]
    fn fixes_empty_string_content() {
        let msgs = vec![json!({"role":"user","content":""})];
        let out = sanitize_empty_content(&msgs);
        assert_eq!(out[0]["content"], "(empty)");
    }

    #[test]
    fn inplace_strip_images() {
        let mut msgs = vec![json!({
            "role":"user",
            "content":[
                {"type":"text","text":"hello"},
                {"type":"image_url","image_url":{"url":"http://example.com/img.png"},"_meta":{"path":"test.png"}}
            ]
        })];
        let found = strip_image_content_inplace(&mut msgs);
        assert!(found);
        assert_eq!(msgs[0]["content"][1]["text"], "[image: test.png]");
    }
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
