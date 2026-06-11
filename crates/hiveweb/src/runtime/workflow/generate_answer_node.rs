//! Workflow DAG 执行器（research §5 / FR-013 / US3 / T110）
//!
//! 拓扑序 BFS 分层 + `tokio::join_all` 并行同层节点。环检测在 service 层
//! `services::workflow::put_graph` 保存时已禁止（spec FR-014），运行时不再校验。
//!
//! mapping 解析：每个 edge 的 `mapping = {"dst.input.<field>": "<src_node_key>.output.<path>"}`
//! 路径 `<path>` 支持简单 dot-path (e.g. `temp_c` 或 `nested.field`)。

use serde_json::Value;
use std::sync::Arc;

use super::{ExecutorDeps, WorkflowError};
use agent::context::AgentContext;

/// Replace `{var_name}` template variables in a string with values from input JSON.
pub fn resolve_template_vars(template: &str, input: &Value) -> String {
    let mut result = template.to_string();
    if let Value::Object(map) = input {
        for (key, val) in map {
            let placeholder = format!("{{{key}}}");
            let replacement = match val {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            result = result.replace(&placeholder, &replacement);
        }
    }
    // Replace remaining unresolved placeholders with empty string.
    // Supports optional whitespace: { var } or {var}
    let re = regex::Regex::new(r"\{\s*[a-zA-Z0-9_]+\s*\}")
        .unwrap_or_else(|_| regex::Regex::new(r"\{[^}]+\}").unwrap());
    re.replace_all(&result, "").to_string()
}

/// Build conversation history context from AgentContext messages.
///
/// When `limit > 0`, returns the most recent `limit` messages from the
/// conversation history stored in AgentContext, formatted as role:content pairs.
pub fn build_history_context(agent_ctx: &AgentContext, limit: usize) -> String {
    let messages = match agent_ctx.get_messages() {
        Ok(msgs) => msgs.iter().skip(1).collect(),//first is system prompt, skip it
        Err(_) => return String::new(),
    };

    if messages.is_empty() {
        return String::new();
    }

    // Take the most recent `limit` messages when limit > 0
    let recent: Vec<&serde_json::Value> = if limit > 0 && messages.len() > limit {
        let skip = messages.len() - limit;
        messages.iter().skip(skip).collect()
    } else {
        messages.iter().collect()
    };

    let mut lines: Vec<String> = Vec::with_capacity(recent.len() + 1);
    lines.push("对话历史:".to_string());
    for msg in &recent {
        let role = msg
            .get("role")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let content = msg
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if !content.is_empty() {
            lines.push(format!("{role}: {content}"));
        }
    }

    lines.join("\n")
}

/// Extract the set of keys referenced by `{var}` placeholders in a template.
/// Supports optional whitespace: `{ var }` and `{var}` both match.
fn referenced_template_keys(template: &str) -> std::collections::HashSet<String> {
    let re = regex::Regex::new(r"\{\s*([a-zA-Z0-9_]+)\s*\}")
        .unwrap_or_else(|_| regex::Regex::new(r"\{[^}]+\}").unwrap());
    re.captures_iter(template)
        .filter_map(|c| c.get(1).map(|m| m.as_str().to_string()))
        .collect()
}


/// Build a human-readable user message from the resolved node input.
///
/// The LLM chat pattern expects:
///   - `system`: instructions + context (with `{var}` placeholders resolved)
///   - `user`: the user's actual question / request
///
/// The user's question is conventionally a single string field (often named
/// `query`, `text`, `input`, etc.). We prefer that field as the user message
/// so the LLM sees the actual question rather than a `"key: value"` dump.
///
/// If no "user question" field is found we fall back to the first string
/// field; if there are no string fields we dump remaining fields as
/// `"key: value"` (skipping internal `_`-prefixed keys). Keys that were
/// injected into the system_prompt via `{var}` are skipped in the fallback
/// path to avoid duplicating context data.
pub fn build_user_message(input: &Value, system_prompt: &str) -> String {
    // Common names for the field that holds the user's actual question.
    const USER_QUESTION_KEYS: &[&str] = &[
        "query",
        "question",
        "text",
        "input",
        "message",
        "prompt",
        "user_input",
        "raw_text",
        "user_message",
        "ask",
    ];

    if let Value::Object(map) = input {
        // Priority 1: a well-known "user question" field
        for key in USER_QUESTION_KEYS {
            if !key.starts_with('_') {
                if let Some(Value::String(s)) = map.get(*key) {
                    return s.clone();
                }
            }
        }
        // Priority 2: the first string field (alphabetical order from BTreeMap)
        for (k, v) in map {
            if !k.starts_with('_') {
                if let Value::String(s) = v {
                    return s.clone();
                }
            }
        }
        // Priority 3: dump unreferenced non-internal fields as "key: value"
        let referenced = referenced_template_keys(system_prompt);
        let lines: Vec<String> = map
            .iter()
            .filter(|(k, _)| !referenced.contains(*k) && !k.starts_with('_'))
            .map(|(k, v)| {
                let val = serde_json::to_string(v).unwrap_or_default();
                format!("{k}: {val}")
            })
            .collect();
        if !lines.is_empty() {
            return lines.join("\n");
        }
    }
    // Last resort: original "all fields" behavior
    match input {
        Value::Object(map) if !map.is_empty() => {
            let lines: Vec<String> = map
                .iter()
                .filter(|(k, _)| !k.starts_with('_'))
                .map(|(k, v)| {
                    let val = match v {
                        Value::String(s) => s.clone(),
                        other => serde_json::to_string(other).unwrap_or_default(),
                    };
                    format!("{k}: {val}")
                })
                .collect();
            lines.join("\n")
        }
        Value::String(s) => s.clone(),
        _ => serde_json::to_string(input).unwrap_or_default(),
    }
}



/// Execute a generate-answer node: resolves template variables from upstream outputs
/// and calls LLM to generate a response.
pub async fn execute_answer_node(
    deps: &ExecutorDeps,
    node_key: &str,
    mut input: Value,
    node_config: Option<Value>,
    invoking_agent_id: i64,
    agent_ctx: Arc<AgentContext>,
) -> Result<(String, Value), WorkflowError> {
    let config = node_config.unwrap_or_default();
    let system_prompt = config
        .get("system_prompt")
        .and_then(|v| v.as_str())
        .unwrap_or("You are a helpful assistant.");
    let model_preset = config
        .get("model_preset")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty());
    let history_window = config
        .get("history_window")
        .and_then(|v| v.as_i64())
        .unwrap_or(0)
        .max(0) as usize;

    // Strip _agent_context from input — it's runtime machinery, not user data
    if let Value::Object(ref mut map) = input {
        map.remove("_agent_context");
    }

    // Resolve template variables in system_prompt (e.g., "{query}" → actual value)
    let resolved_prompt = resolve_template_vars(system_prompt, &input);

    // Build the user message from the input. We prefer the "user's question"
    // field (e.g. `query`) so the LLM sees the actual question, not a
    // `"key: value"` dump. Fields injected into the system_prompt via `{var}`
    // are excluded from the fallback dump to avoid duplicating context data.
    let mut user_message = build_user_message(&input, system_prompt);

    // When history_window > 0, fetch recent messages from AgentContext
    // and prepend them as conversation history to the user message.
    if history_window > 0 {
        let history_context = build_history_context(&agent_ctx, history_window);
        if !history_context.is_empty() {
            user_message = format!("{history_context}\n\n---\n当前输入:\n{user_message}");
        }
    }

    tracing::info!(
        node_key,
        system_prompt_len = system_prompt.len(),
        resolved_prompt_len = resolved_prompt.len(),
        user_message_len = user_message.len(),
        input_keys = ?input.as_object().map(|m| m.keys().collect::<Vec<_>>()),
        query_value = ?input.get("query").and_then(|v| v.as_str()),
        "execute_answer_node: invoking LLM"
    );

    // Try LLM invocation; fall back to direct response if no LLM available
    let answer = match deps.llm.build_primary(model_preset) {
        Ok((provider, model)) => {
            use providers::ChatRequest;
            let req = ChatRequest {
                messages: vec![
                    serde_json::json!({"role": "system", "content": resolved_prompt}),
                    serde_json::json!({"role": "user", "content": user_message}),
                ],
                model: Some(model.clone()),
                max_tokens: 2048,
                temperature: 0.7,
                tools: None,
                tool_choice: None,
                reasoning_effort: None,
            };
            let resp = provider.chat(req).await;
            if resp.is_error() {
                let err_msg = resp
                    .content
                    .unwrap_or_else(|| "unknown LLM error".to_string());
                tracing::warn!(node_key, error = %err_msg, "answer node LLM failed, using fallback");
                format!("[LLM 调用失败: {err_msg}] 输入: {user_message}")
            } else {
                resp.content.unwrap_or_default()
            }
        }
        Err(e) => {
            tracing::warn!(node_key, error = %e, "answer node no LLM provider, using fallback");
            format!("[无可用模型] 系统提示: {resolved_prompt}\n输入: {user_message}")
        }
    };

    let _ = invoking_agent_id;
    let _ = history_window;

    Ok((
        node_key.to_string(),
        serde_json::json!({
            "answer": answer,
            "model_preset": model_preset,
        }),
    ))
}
