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
    let messages: Vec<serde_json::Value> = match agent_ctx.get_messages() {
        Ok(msgs) if !msgs.is_empty() => msgs,
        Ok(_) | Err(_) => return format!("用户A:{}", agent_ctx.user_input().raw_text),
    };

    // Take the most recent `limit` messages when limit > 0
    let recent: Vec<&serde_json::Value> = if limit > 0 && messages.len() > limit {
        let skip = messages.len() - limit;
        messages.iter().skip(skip).collect()
    } else {
        messages.iter().collect()
    };

    let mut lines: Vec<String> = Vec::with_capacity(recent.len() + 1);
    for msg in &recent {
        let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("");
        let content = msg.get("content").and_then(|v| v.as_str()).unwrap_or("");
        if !content.is_empty() {
            let user = if role == "user" {
                "用户A"
            } else if role == "assistant" {
                "用户B"
            } else {
                role
            };
            lines.push(format!("{user}: {content}"));
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

    // When history_window > 0, build conversation history from AgentContext
    // messages and inject it as {context} for system_prompt template replacement.
    if history_window > 0 {
        let history_context = build_history_context(&agent_ctx, history_window);
        if !history_context.is_empty() {
            if let Value::Object(ref mut map) = input {
                map.insert("context".to_string(), Value::String(history_context));
            }
        }
    }

    // Resolve template variables in system_prompt (e.g., "{query}" → actual value,
    // "{context}" → conversation history).
    let resolved_prompt = resolve_template_vars(system_prompt, &input);

    tracing::info!(
        node_key,
        system_prompt_len = system_prompt.len(),
        resolved_prompt_len = resolved_prompt.len(),
        input_keys = ?input.as_object().map(|m| m.keys().collect::<Vec<_>>()),
        query_value = ?input.get("query").and_then(|v| v.as_str()),
        "execute_answer_node: invoking LLM"
    );

    // Try LLM invocation; fall back to direct response if no LLM available
    let (max_tokens, temperature) = deps.llm.resolve_config(model_preset);
    let answer = match deps.llm.build_primary(model_preset) {
        Ok((provider, model)) => {
            use providers::ChatRequest;
            let req = ChatRequest {
                messages: vec![serde_json::json!({"role": "system", "content": resolved_prompt})],
                model: Some(model.clone()),
                max_tokens,
                temperature,
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
                format!("[LLM 调用失败: {err_msg}]")
            } else {
                resp.content.unwrap_or_default()
            }
        }
        Err(e) => {
            tracing::warn!(node_key, error = %e, "answer node no LLM provider, using fallback");
            format!("[无可用模型] 系统提示: {resolved_prompt}\n")
        }
    };

    let _ = invoking_agent_id;

    Ok((
        node_key.to_string(),
        serde_json::json!({
            "answer": answer,
            "model_preset": model_preset,
        }),
    ))
}

/// Build a human-readable user message from the resolved node input.
///
/// Priority: well-known question key (query/text/etc) → first string field → "key: value" dump.
/// Keys referenced by `{var}` in system_prompt are excluded from the dump to avoid duplication.
fn build_user_message(input: &Value, system_prompt: &str) -> String {
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
        for key in USER_QUESTION_KEYS {
            if let Some(Value::String(s)) = map.get(*key) {
                return s.clone();
            }
        }
        for (k, v) in map {
            if !k.starts_with('_') {
                if let Value::String(s) = v {
                    return s.clone();
                }
            }
        }
        let re = regex::Regex::new(r"\{\s*([a-zA-Z0-9_]+)\s*\}")
            .unwrap_or_else(|_| regex::Regex::new(r"\{[^}]+\}").unwrap());
        let referenced: std::collections::HashSet<String> = re
            .captures_iter(system_prompt)
            .filter_map(|c| c.get(1).map(|m| m.as_str().to_string()))
            .collect();
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
    serde_json::to_string(input).unwrap_or_default()
}
