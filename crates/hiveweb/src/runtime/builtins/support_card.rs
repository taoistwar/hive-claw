//! support_card builtin — generate a "support" extension card carrying the
//! user's raw text input, and signal the orchestrator to break the agent loop.
//!
//! Flow:
//!   builtin returns JSON with `_agent_context_updates`
//!     → `runtime::hook::apply_agent_context_updates` reads it
//!     → `AgentContext::add_extension` inserts the card
//!     → `AgentContext::set_metadata("agent_loop_break", "true")` sets the flag
//!   the orchestrator then sees `agent_loop_break == "true"` and ends the hop loop.
//!
//! The handler is sync (no DB I/O) — all state comes from the injected
//! `BuiltinContext::agent_ctx`. Mirrors the structure of `query_balance`'s
//! return value (`extensions` array + `metadata.agent_loop_break`).

use serde_json::{Value, json};
use std::sync::Arc;

use crate::runtime::builtins::{
    BuiltinContext, BuiltinResult,
    rag_answer::{ask_llm_to_answer, extract_question, query_rag_chunks},
};
use crate::services::ragflow_config::RagflowConfig;

/// support_card handler — does not take LLM args; all state is read from
/// `ctx.agent_ctx.user_input()`.
pub fn support_card(_args: Value, ctx: &BuiltinContext) -> BuiltinResult {
    let question = extract_question(ctx.agent_ctx.as_ref());
    let llm = Arc::clone(&ctx.llm);
    let config = RagflowConfig::current();
    tokio::task::block_in_place(move || {
        tokio::runtime::Handle::current()
            .block_on(async move { support_card_async_impl(question, llm.as_ref(), config).await })
    })
}

async fn support_card_async_impl(
    question: String,
    llm: &crate::runtime::llm::LlmRegistry,
    config: RagflowConfig,
) -> BuiltinResult {
    let rag_chunks = query_rag_chunks(question.clone(), config).await;

    let has_knowledge = !rag_chunks.is_empty();

    let llm_answer = ask_llm_to_answer(&question, &rag_chunks, has_knowledge, llm).await;
    if !llm_answer.trim().is_empty() {
        let answer = llm_answer.trim().to_string();
        let support = "\n\n上面是AI智能回复，仅供参考。如果回答不满意，你可以通过下方「联系客服」继续反馈，我们会尽力协助处理。";
        let answer = format!("{}{}", answer, support);
        return Ok(json!({
            "_agent_context_updates": {
                "extensions": [{
                    "content_type": "card",
                    "payload": {
                        "type": "support"
                    },
                }],
                "metadata": {
                    "agent_loop_break": "true",
                    "agent_loop_reply": answer
                }
            }
        }));
    }

    Ok(json!({
        "_agent_context_updates": {
            "extensions": [{
                "content_type": "card",
                "payload": {
                    "type": "support"
                },
            }],
            "metadata": {
                "agent_loop_break": "true",
                "agent_loop_reply": "抱歉，我无法回答您的问题。你可以通过下方「联系客服」继续反馈，我们会尽力协助处理。"
            }
        }
    }))
}

pub const SUPPORT_CARD_INPUT_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
  }
}"#;

pub const SUPPORT_CARD_OUTPUT_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "message": {
      "type": "string",
      "description": "成功生成 support 卡片，会随 _agent_context_updates 注入 AgentContext 并立即结束 agent loop。"
    }
  }
}"#;
