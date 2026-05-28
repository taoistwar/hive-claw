//! llm.invoke capability handler (T099 / US4 commit 4)
//!
//! Plugin 调用 LLM 走 Agent 当前 model_preset 解析的 primary provider。
//! 简化版：返回 single-shot chat completion（无 streaming，无 tool_calls）。
//!
//! 真实的 chat 路由 / tool-calling 循环在 orchestrator 已实现；本 capability
//! 是给 **Plugin 内部** 在 host_call 范围内做一次性 LLM 询问（如 summarize、
//! classify 等无 tool 的子任务）。

use providers::{ChatRequest, LLMProvider, RetryMode};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::MySqlPool;
use std::sync::Arc;

use crate::runtime::llm::LlmRegistry;

#[derive(Debug, Deserialize)]
pub struct LlmInvokeArgs {
    /// 简化：plugin 提供 messages 数组 (OpenAI-style) 或 prompt 字符串
    #[serde(default)]
    pub messages: Vec<Value>,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub temperature: Option<f32>,
}

#[derive(Debug, Serialize)]
pub struct LlmInvokeReply {
    pub content: Option<String>,
    pub finish_reason: String,
    pub usage: serde_json::Map<String, Value>,
}

/// `agent_id` 用于解析当前 agent 的 model_preset（沿用 dispatch ctx）
pub async fn llm_invoke(
    pool: &MySqlPool,
    llm: &Arc<LlmRegistry>,
    agent_id: i64,
    args: LlmInvokeArgs,
) -> Result<LlmInvokeReply, String> {
    if args.messages.is_empty() && args.prompt.is_none() {
        return Err("llm.invoke 至少需要 messages 或 prompt 之一".into());
    }

    // resolve agent.model_preset
    let preset: Option<(Option<String>,)> =
        sqlx::query_as("SELECT model_preset FROM agents WHERE id = ?")
            .bind(agent_id)
            .fetch_optional(pool)
            .await
            .map_err(|e| format!("agent fetch: {e}"))?;
    let preset_name = preset.and_then(|(p,)| p);

    let (provider, model) = llm
        .build_primary(preset_name.as_deref())
        .map_err(|e| format!("build_primary: {e}"))?;

    // 组装 messages
    let messages: Vec<Value> = if !args.messages.is_empty() {
        args.messages.clone()
    } else {
        vec![serde_json::json!({
            "role": "user",
            "content": args.prompt.clone().unwrap_or_default(),
        })]
    };

    let req = ChatRequest {
        model: Some(model),
        messages,
        max_tokens: args.max_tokens.unwrap_or(2048),
        temperature: args.temperature.unwrap_or(0.7),
        tools: None,
        tool_choice: None,
        reasoning_effort: None,
    };

    let resp = provider.chat_with_retry(req, RetryMode::Standard, None).await;
    if resp.is_error() {
        let msg = resp
            .content
            .clone()
            .or(resp.error_kind.clone())
            .unwrap_or_else(|| "LLM error".into());
        return Err(format!("LLM error: {msg}"));
    }

    let usage_value =
        serde_json::to_value(&resp.usage).unwrap_or(Value::Object(serde_json::Map::new()));
    let usage_map = usage_value
        .as_object()
        .cloned()
        .unwrap_or_default();

    Ok(LlmInvokeReply {
        content: resp.content.clone(),
        finish_reason: resp.finish_reason.to_string(),
        usage: usage_map,
    })
}
