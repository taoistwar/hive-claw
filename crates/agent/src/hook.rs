//! Lifecycle hooks for agent runs. Port of `nanobot.agent.hook`.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use providers::{LLMResponse, ToolCallRequest};

/// Mutable per-iteration state exposed to runner hooks.
#[derive(Debug, Default)]
pub struct AgentHookContext {
    pub iteration: usize,
    pub messages: Vec<Value>,
    pub response: Option<LLMResponse>,
    pub usage: HashMap<String, i64>,
    pub tool_calls: Vec<ToolCallRequest>,
    pub tool_results: Vec<Value>,
    pub tool_events: Vec<ToolEvent>,
    pub final_content: Option<String>,
    pub stop_reason: Option<String>,
    pub error: Option<String>,
}

/// Summary entry for one tool invocation.
#[derive(Debug, Clone)]
pub struct ToolEvent {
    pub name: String,
    pub status: String, // "ok" | "error" | ...
    pub detail: String,
}

/// Minimal lifecycle surface for shared runner customization.
///
/// All async hooks receive `&mut AgentHookContext`; `finalize_content` is a
/// pipeline step that can transform the final string.
#[async_trait]
pub trait AgentHook: Send + Sync {
    fn wants_streaming(&self) -> bool {
        false
    }
    /// If true, exceptions raised from hook methods are propagated to the
    /// loop instead of being swallowed by [`CompositeHook`]. Used mostly by
    /// tests to surface hook bugs.
    fn reraise(&self) -> bool {
        false
    }

    async fn before_iteration(&self, _ctx: &mut AgentHookContext) {}
    async fn on_stream(&self, _ctx: &mut AgentHookContext, _delta: &str) {}
    async fn on_stream_end(&self, _ctx: &mut AgentHookContext, _resuming: bool) {}
    async fn before_execute_tools(&self, _ctx: &mut AgentHookContext) {}
    async fn after_iteration(&self, _ctx: &mut AgentHookContext) {}

    fn finalize_content(
        &self,
        _ctx: &mut AgentHookContext,
        content: Option<String>,
    ) -> Option<String> {
        content
    }
}

/// Fan-out hook that delegates to an ordered list of hooks.
///
/// Async methods catch and log per-hook exceptions so a faulty custom hook
/// cannot crash the agent loop; `finalize_content` is a pipeline (no
/// isolation — bugs should surface).
pub struct CompositeHook {
    hooks: Vec<Arc<dyn AgentHook>>,
}

impl CompositeHook {
    pub fn new(hooks: Vec<Arc<dyn AgentHook>>) -> Self {
        Self { hooks }
    }
}

#[async_trait]
impl AgentHook for CompositeHook {
    fn wants_streaming(&self) -> bool {
        self.hooks.iter().any(|h| h.wants_streaming())
    }

    async fn before_iteration(&self, ctx: &mut AgentHookContext) {
        for h in &self.hooks {
            h.before_iteration(ctx).await;
        }
    }
    async fn on_stream(&self, ctx: &mut AgentHookContext, delta: &str) {
        for h in &self.hooks {
            h.on_stream(ctx, delta).await;
        }
    }
    async fn on_stream_end(&self, ctx: &mut AgentHookContext, resuming: bool) {
        for h in &self.hooks {
            h.on_stream_end(ctx, resuming).await;
        }
    }
    async fn before_execute_tools(&self, ctx: &mut AgentHookContext) {
        for h in &self.hooks {
            h.before_execute_tools(ctx).await;
        }
    }
    async fn after_iteration(&self, ctx: &mut AgentHookContext) {
        for h in &self.hooks {
            h.after_iteration(ctx).await;
        }
    }

    fn finalize_content(
        &self,
        ctx: &mut AgentHookContext,
        mut content: Option<String>,
    ) -> Option<String> {
        for h in &self.hooks {
            content = h.finalize_content(ctx, content);
        }
        content
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct StreamingHook;
    #[async_trait]
    impl AgentHook for StreamingHook {
        fn wants_streaming(&self) -> bool {
            true
        }
    }

    struct NoopHook;
    #[async_trait]
    impl AgentHook for NoopHook {}

    #[tokio::test]
    async fn composite_wants_streaming_if_any_child_does() {
        let c = CompositeHook::new(vec![Arc::new(NoopHook), Arc::new(StreamingHook)]);
        assert!(c.wants_streaming());
    }

    #[tokio::test]
    async fn finalize_pipeline_applies_each_hook() {
        struct Appender(&'static str);
        #[async_trait]
        impl AgentHook for Appender {
            fn finalize_content(
                &self,
                _ctx: &mut AgentHookContext,
                content: Option<String>,
            ) -> Option<String> {
                content.map(|s| format!("{s}{}", self.0))
            }
        }
        let c =
            CompositeHook::new(vec![Arc::new(Appender(" a")), Arc::new(Appender(" b"))]);
        let mut ctx = AgentHookContext::default();
        let out = c.finalize_content(&mut ctx, Some("x".into()));
        assert_eq!(out, Some("x a b".to_string()));
    }
}
