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

use crate::runtime::builtins::{BuiltinContext, BuiltinError, BuiltinResult};

/// support_card handler — does not take LLM args; all state is read from
/// `ctx.agent_ctx.user_input()`.
pub fn support_card(_args: Value, ctx: &BuiltinContext) -> BuiltinResult {
    let agent_ctx = ctx
        .agent_ctx
        .as_ref()
        .ok_or_else(|| BuiltinError::Exec("AgentContext 未配置".into()))?;

    Ok(json!({
        "_agent_context_updates": {
            "extensions": [{
                "content_type": "card",
                "payload": {
                    "type": "support"
                },
            }],
            "metadata": {
                "agent_loop_break": "true"
            }
        }
    }))
}

pub const SUPPORT_CARD_INPUT_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {}
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
