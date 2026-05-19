//! Runtime-specific helpers and constants (port of `nanobot.utils.runtime`).

use std::collections::HashMap;

use serde_json::Value;

use crate::helpers::stringify_text_blocks;

const MAX_REPEAT_EXTERNAL_LOOKUPS: u32 = 2;

pub const EMPTY_FINAL_RESPONSE_MESSAGE: &str =
    "I completed the tool steps but couldn't produce a final answer. \
     Please try again or narrow the task.";

pub const FINALIZATION_RETRY_PROMPT: &str =
    "Please provide your response to the user based on the conversation above.";

pub const LENGTH_RECOVERY_PROMPT: &str =
    "Output limit reached. Continue exactly where you left off \
     — no recap, no apology. Break remaining work into smaller steps if needed.";

/// Short prompt-safe marker for tools that completed without visible output.
pub fn empty_tool_result_message(tool_name: &str) -> String {
    format!("({tool_name} completed with no output)")
}

/// Replace semantically empty tool results with a short marker string.
pub fn ensure_nonempty_tool_result(tool_name: &str, content: Value) -> Value {
    match &content {
        Value::Null => Value::String(empty_tool_result_message(tool_name)),
        Value::String(s) if s.trim().is_empty() => {
            Value::String(empty_tool_result_message(tool_name))
        }
        Value::Array(arr) => {
            if arr.is_empty() {
                return Value::String(empty_tool_result_message(tool_name));
            }
            if let Some(text) = stringify_text_blocks(arr) {
                if text.trim().is_empty() {
                    return Value::String(empty_tool_result_message(tool_name));
                }
            }
            content
        }
        _ => content,
    }
}

/// `true` when *content* is missing or only whitespace.
pub fn is_blank_text(content: Option<&str>) -> bool {
    match content {
        None => true,
        Some(s) => s.trim().is_empty(),
    }
}

fn role_message(content: &str) -> serde_json::Value {
    serde_json::json!({ "role": "user", "content": content })
}

/// A short no-tools-allowed prompt for final-answer recovery.
pub fn build_finalization_retry_message() -> serde_json::Value {
    role_message(FINALIZATION_RETRY_PROMPT)
}

/// Prompt the model to continue after hitting an output-token limit.
pub fn build_length_recovery_message() -> serde_json::Value {
    role_message(LENGTH_RECOVERY_PROMPT)
}

/// Stable signature for repeated external lookups we want to throttle.
pub fn external_lookup_signature(tool_name: &str, arguments: &Value) -> Option<String> {
    let get_str = |key: &str| -> String {
        arguments
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string()
    };
    match tool_name {
        "web_fetch" => {
            let url = get_str("url");
            if url.is_empty() {
                None
            } else {
                Some(format!("web_fetch:{}", url.to_lowercase()))
            }
        }
        "web_search" => {
            let query = {
                let q = get_str("query");
                if q.is_empty() { get_str("search_term") } else { q }
            };
            if query.is_empty() {
                None
            } else {
                Some(format!("web_search:{}", query.to_lowercase()))
            }
        }
        _ => None,
    }
}

/// Block repeated external lookups after a small retry budget.
pub fn repeated_external_lookup_error(
    tool_name: &str,
    arguments: &Value,
    seen_counts: &mut HashMap<String, u32>,
) -> Option<String> {
    let sig = external_lookup_signature(tool_name, arguments)?;
    let count = seen_counts.entry(sig.clone()).or_insert(0);
    *count += 1;
    if *count <= MAX_REPEAT_EXTERNAL_LOOKUPS {
        return None;
    }
    log::warn!(
        "Blocking repeated external lookup {} on attempt {}",
        &sig.chars().take(160).collect::<String>(),
        count,
    );
    Some(
        "Error: repeated external lookup blocked. \
         Use the results you already have to answer, or try a meaningfully different source."
            .into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn marker_on_empty_string() {
        let v = ensure_nonempty_tool_result("exec", json!(""));
        assert_eq!(v.as_str().unwrap(), "(exec completed with no output)");
    }

    #[test]
    fn marker_on_empty_array() {
        let v = ensure_nonempty_tool_result("exec", json!([]));
        assert_eq!(v.as_str().unwrap(), "(exec completed with no output)");
    }

    #[test]
    fn keeps_non_empty_string() {
        let v = ensure_nonempty_tool_result("exec", json!("hi"));
        assert_eq!(v.as_str().unwrap(), "hi");
    }

    #[test]
    fn lookup_throttle() {
        let mut counts = HashMap::new();
        let args = json!({"url": "https://EX.com/foo"});
        assert!(repeated_external_lookup_error("web_fetch", &args, &mut counts).is_none());
        assert!(repeated_external_lookup_error("web_fetch", &args, &mut counts).is_none());
        assert!(repeated_external_lookup_error("web_fetch", &args, &mut counts).is_some());
    }
}
