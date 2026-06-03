//! PromptData and prompt budget building for AgentContext.
//!
//! Implements the fixed-priority token budget algorithm for building
//! LLM prompts from context data:
//!
//! Priority order (always included first):
//! 1. user_input (100% — never truncated)
//! 2. recent_reasoning
//! 3. tool_results
//! 4. entities
//! 5. history_state

use serde::{Deserialize, Serialize};

use super::category::Category;
use super::core::{AgentContext, ContextError};

/// Data prepared for inclusion in an LLM prompt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptData {
    /// User input text (always included, never truncated)
    pub user_input: String,
    /// Most recent reasoning results (may be truncated)
    pub recent_reasoning: Option<String>,
    /// Tool execution results (may be truncated)
    pub tool_results: Option<String>,
    /// Entity recognition results (may be truncated)
    pub entities: Option<String>,
    /// Historical state summary (may be truncated)
    pub history_state: Option<String>,
    /// Estimated token count for the assembled prompt
    pub estimated_tokens: usize,
    /// Whether any section was truncated due to token budget
    pub truncated: bool,
}

/// Estimate token count from text using the configured estimation mode.
///
/// In "chars" mode, estimates ~4 characters per token (common approximation).
/// In "tiktoken" mode, would use the tiktoken library (falls back to chars).
fn estimate_tokens(text: &str, mode: &str) -> usize {
    match mode {
        "tiktoken" | "tiktoken_cl100k" => {
            // Fallback to character-based estimation since tiktoken is not
            // a direct dependency. In production, integrate tiktoken here.
            text.chars().count().div_ceil(4)
        }
        _ => {
            // Default: chars mode, ~4 chars per token
            text.chars().count().div_ceil(4)
        }
    }
}

/// Serialize category records into a string representation for prompt inclusion.
fn format_records(records: &[super::category::RecordEntry], max_chars: usize) -> (String, bool) {
    if records.is_empty() {
        return (String::new(), false);
    }

    // Serialize as compact JSON array
    let full = match serde_json::to_string(records) {
        Ok(s) => s,
        Err(_) => return (String::new(), false),
    };

    if full.chars().count() <= max_chars {
        return (full, false);
    }

    // Truncate: keep most recent records (end of the array)
    let mut truncated = String::new();
    let mut accumulated = String::new();
    let truncated_flag = true;

    // Add truncation marker
    truncated.push_str("[truncated] ");

    // Take records from the end until we fit
    for record in records.iter().rev() {
        let record_str = match serde_json::to_string(record) {
            Ok(s) => s,
            Err(_) => continue,
        };
        if accumulated.chars().count() + record_str.chars().count() + 3 > max_chars - 14 {
            break;
        }
        if !accumulated.is_empty() {
            accumulated.push_str(", ");
        }
        accumulated.push_str(&record_str);
    }

    truncated.push('[');
    truncated.push_str(&accumulated);
    truncated.push(']');

    (truncated, truncated_flag)
}

/// Build prompt data from the context within a token budget.
///
/// Uses a fixed-priority approach:
/// 1. `user_input` is always included (never counted against budget)
/// 2. Remaining budget is allocated to: recent_reasoning → tool_results → entities → history_state
///
/// Each section is filled in priority order until the budget is exhausted.
///
/// # Arguments
/// * `ctx` — The agent context to build from
/// * `max_tokens` — Maximum token budget for the assembled prompt
///
/// # Errors
/// Returns `ContextError::MergeFailed` if the context lock is poisoned.
pub fn build_prompt_budget(
    ctx: &AgentContext,
    max_tokens: usize,
) -> Result<PromptData, ContextError> {
    let user_input = ctx.user_input().raw_text.clone();
    let token_mode = &ctx.config.token_estimation_mode;

    // Count user input tokens (always included)
    let user_tokens = estimate_tokens(&user_input, token_mode);

    // Available budget for other sections
    let available = max_tokens.saturating_sub(user_tokens);

    // Priority list: (category, token allocation strategy)
    let priorities = [
        (Category::ReasoningResults, "recent_reasoning"),
        (Category::ToolResults, "tool_results"),
        (Category::Entities, "entities"),
        (Category::StateChanges, "history_state"),
    ];

    let mut remaining = available;
    let mut recent_reasoning: Option<String> = None;
    let mut tool_results: Option<String> = None;
    let mut entities: Option<String> = None;
    let mut history_state: Option<String> = None;
    let mut truncated = false;

    // Allocate budget greedily by priority
    for (category, _name) in &priorities {
        let records = ctx.get_category(*category)?;

        if records.is_empty() {
            continue;
        }

        // Allocate remaining budget to this section (or all remaining if it's the last)
        let section_budget = remaining;

        let (text, was_truncated) = format_records(&records, section_budget.saturating_mul(4)); // rough chars budget
        let text_tokens = estimate_tokens(&text, token_mode);

        match *category {
            Category::ReasoningResults => {
                recent_reasoning = if text.is_empty() { None } else { Some(text) }
            }
            Category::ToolResults => tool_results = if text.is_empty() { None } else { Some(text) },
            Category::Entities => entities = if text.is_empty() { None } else { Some(text) },
            Category::StateChanges => {
                history_state = if text.is_empty() { None } else { Some(text) }
            }
            _ => {}
        }

        if was_truncated {
            truncated = true;
        }

        remaining = remaining.saturating_sub(text_tokens);
    }

    let estimated_tokens = user_tokens
        + recent_reasoning
            .as_ref()
            .map(|s| estimate_tokens(s, token_mode))
            .unwrap_or(0)
        + tool_results
            .as_ref()
            .map(|s| estimate_tokens(s, token_mode))
            .unwrap_or(0)
        + entities
            .as_ref()
            .map(|s| estimate_tokens(s, token_mode))
            .unwrap_or(0)
        + history_state
            .as_ref()
            .map(|s| estimate_tokens(s, token_mode))
            .unwrap_or(0);

    Ok(PromptData {
        user_input,
        recent_reasoning,
        tool_results,
        entities,
        history_state,
        estimated_tokens,
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::context::config::ContextConfig;

    fn make_context() -> AgentContext {
        use crate::context::UserInput;

        let user_input = UserInput {
            raw_text: "What is the weather in Beijing?".into(),
            session_id: Some("sess-1".into()),
            message_id: Some("msg-1".into()),
            timestamp: chrono::Utc::now(),
            metadata: HashMap::new(),
        };
        AgentContext::new("ctx-1".into(), user_input, ContextConfig::default())
    }

    #[test]
    fn build_prompt_budget_user_input_always_included() {
        let ctx = make_context();
        let data = build_prompt_budget(&ctx, 1000).unwrap();

        assert_eq!(data.user_input, "What is the weather in Beijing?");
        assert!(!data.truncated);
    }

    #[test]
    fn build_prompt_budget_empty_context() {
        let ctx = make_context();
        let data = build_prompt_budget(&ctx, 1000).unwrap();

        assert!(data.recent_reasoning.is_none());
        assert!(data.tool_results.is_none());
        assert!(data.entities.is_none());
        assert!(data.history_state.is_none());
    }

    #[test]
    fn build_prompt_budget_with_tool_results() {
        let ctx = make_context();
        let _ = ctx.set_record(
            Category::ToolResults,
            "weather_api".into(),
            serde_json::json!({"temperature": 25, "condition": "sunny"}),
            "weather_tool".into(),
            1,
        );

        let data = build_prompt_budget(&ctx, 1000).unwrap();

        assert!(data.tool_results.is_some());
        assert!(!data.truncated);
    }

    #[test]
    fn estimate_tokens_chars_mode() {
        let text = "Hello, world!";
        // 13 chars / 4 = 4 tokens (ceiling)
        assert_eq!(estimate_tokens(text, "chars"), 4);
    }
}
