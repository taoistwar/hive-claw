//! AgentContextSyncHook: lifecycle hook for syncing turn state to AgentContext.
//!
//! This hook is called at key points in the agent loop to ensure that
//! transient turn state is synchronized to the persistent AgentContext.
//! Per FR-019, failures in this hook log errors but never interrupt the loop.

use std::sync::Arc;

use async_trait::async_trait;

use super::super::hook::{AgentHook, AgentHookContext};
use super::core::AgentContext;

/// Hook that synchronizes agent execution state to the AgentContext.
///
/// Called at key lifecycle points to ensure context stays up-to-date with
/// the current execution state. All methods are best-effort: errors are
/// logged but never propagated to avoid interrupting the agent loop.
pub struct AgentContextSyncHook {
    /// Reference to the agent context to sync
    pub context: Arc<AgentContext>,
}

impl AgentContextSyncHook {
    /// Create a new `AgentContextSyncHook` wrapping the given context.
    pub fn new(context: Arc<AgentContext>) -> Self {
        Self { context }
    }

    /// Called before an LLM call is made.
    ///
    /// Synchronizes the current iteration state and reasoning context
    /// to the AgentContext.
    pub fn before_llm_call(&self, iteration: usize) {
        let result = self.context.set_record(
            super::category::Category::ReasoningResults,
            "llm_call_iteration".into(),
            serde_json::json!({"iteration": iteration, "phase": "before_call"}),
            "AgentContextSyncHook".into(),
            iteration,
        );

        if let Err(e) = result {
            log::error!("AgentContextSyncHook: before_llm_call failed: {e}");
        }
    }

    /// Called after a tool execution completes.
    ///
    /// Synchronizes the tool result to the AgentContext and records
    /// the execution in the audit log.
    pub fn after_tool_execution(
        &self,
        tool_name: &str,
        arguments: &serde_json::Value,
        result: &serde_json::Value,
        success: bool,
        iteration: usize,
    ) {
        // Store the tool result in the ToolResults category
        let store_result = self.context.set_record(
            super::category::Category::ToolResults,
            tool_name.into(),
            result.clone(),
            tool_name.into(),
            iteration,
        );

        if let Err(e) = store_result {
            log::error!("AgentContextSyncHook: after_tool_execution set_record failed: {e}");
        }

        // Record in audit log
        let status = if success {
            super::category::ToolCallStatus::Success
        } else {
            super::category::ToolCallStatus::Failure
        };

        let audit_result = self.context.record_tool_call(
            tool_name.into(),
            arguments.clone(),
            Some(result.clone()),
            status,
            chrono::Utc::now(),
            Some(chrono::Utc::now()),
        );

        if let Err(e) = audit_result {
            log::error!("AgentContextSyncHook: after_tool_execution audit failed: {e}");
        }
    }

    /// Called at the end of each agent iteration.
    ///
    /// Synchronizes accumulated state and prepares for the next iteration.
    pub fn after_iteration(&self, iteration: usize, is_final: bool) {
        let state_data = serde_json::json!({
            "iteration": iteration,
            "is_final": is_final
        });

        let result = self.context.set_record(
            super::category::Category::StateChanges,
            "iteration_complete".into(),
            state_data,
            "AgentContextSyncHook".into(),
            iteration,
        );

        if let Err(e) = result {
            log::error!("AgentContextSyncHook: after_iteration failed: {e}");
        }

        if is_final {
            let state_result = self
                .context
                .set_lifecycle_state(super::core::LifecycleState::Completed);
            if let Err(e) = state_result {
                log::error!("AgentContextSyncHook: set_lifecycle_state failed: {e}");
            }
        }
    }

    /// Called before a sub-agent fork is created.
    ///
    /// Records the delegation intent in the audit log.
    pub fn before_subagent_fork(&self, subagent_id: &str, delegation_chain: &str) {
        log::info!(
            "AgentContextSyncHook: before_subagent_fork subagent_id={} delegation_chain={}",
            subagent_id,
            delegation_chain
        );
    }

    /// Called after a sub-agent context is merged back into the parent.
    ///
    /// Records the delegation result in the audit log.
    pub fn after_subagent_merge(&self, subagent_id: &str, _delegation_chain: &str, success: bool) {
        log::info!(
            "AgentContextSyncHook: after_subagent_merge subagent_id={} success={}",
            subagent_id,
            success
        );
    }
}

/// Implement the existing AgentHook trait so this hook integrates with the
/// agent lifecycle. The trait methods delegate to the native hook methods above.
#[async_trait]
impl AgentHook for AgentContextSyncHook {
    async fn before_iteration(&self, _ctx: &mut AgentHookContext) {
        // Called before each iteration — track iteration start
    }

    async fn after_iteration(&self, ctx: &mut AgentHookContext) {
        // Use the AgentHookContext iteration count
        let iteration = ctx.iteration;
        let is_final = ctx.stop_reason.is_some() || ctx.error.is_some();

        // Sync tool results from this iteration
        for tc in &ctx.tool_calls {
            // Find matching result
            let result = ctx
                .tool_results
                .iter()
                .find(|r| r.get("tool_call_id").and_then(|v| v.as_str()) == Some(&tc.id));

            if let Some(result) = result {
                self.after_tool_execution(
                    &tc.name,
                    &serde_json::Value::Object(tc.arguments.clone()),
                    result,
                    true,
                    iteration,
                );
            }
        }

        // Mark iteration complete
        self.after_iteration(iteration, is_final);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::config::ContextConfig;
    use crate::context::{AuditRecord, Category, LifecycleState};

    fn make_context() -> Arc<AgentContext> {
        use super::super::core::UserInput;
        use std::collections::HashMap;

        let user_input = UserInput {
            raw_text: "test".into(),
            session_id: Some("sess-1".into()),
            message_id: Some("msg-1".into()),
            timestamp: chrono::Utc::now(),
            metadata: HashMap::new(),
        };
        Arc::new(AgentContext::new(
            "ctx-1".into(),
            user_input,
            ContextConfig::default(),
        ))
    }

    #[test]
    fn hook_creation() {
        let ctx = make_context();
        let _hook = AgentContextSyncHook::new(ctx);
        // Verify it compiles as AgentHook
        fn _assert_hook<T: AgentHook>() {}
        _assert_hook::<AgentContextSyncHook>();
    }

    #[test]
    fn after_tool_execution_records() {
        let ctx = make_context();
        let hook = AgentContextSyncHook::new(Arc::clone(&ctx));

        hook.after_tool_execution(
            "test_tool",
            &serde_json::json!({}),
            &serde_json::json!("result"),
            true,
            1,
        );

        // Verify tool result was recorded
        let records = ctx.get_category(Category::ToolResults).unwrap();
        assert!(records.iter().any(|r| r.key == "test_tool"));

        // Verify audit log has the tool call
        let audit = ctx.get_audit_log();
        assert!(audit.iter().any(|r| matches!(r, AuditRecord::ToolCall(_))));
    }

    #[test]
    fn after_iteration_sets_final_state() {
        let ctx = make_context();
        let hook = AgentContextSyncHook::new(Arc::clone(&ctx));

        hook.after_iteration(1, true);

        assert_eq!(ctx.lifecycle_state(), LifecycleState::Completed);
    }
}
