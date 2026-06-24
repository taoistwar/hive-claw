//! Agent 编排（research §6 / FR-023..FR-026 / US5）
//!
//! 复用 `crates/agent::AgentRunner` + `SubagentManager`；hiveweb 仅提供：
//! - DB → Agent 配置加载
//! - permissions / model_preset 注入
//! - 5 跳路由上限 + 循环检测（spec FR-023）

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("agent depth exceeded (max=10)")]
    DepthExceeded,
    #[error("routing loop detected: {0:?}")]
    RoutingLoop(Vec<i64>),
    #[error("max hops {0} exceeded")]
    MaxHopsExceeded(u8),
    #[error("LLM error: {0}")]
    Llm(String),
}

#[derive(Debug)]
pub struct AgentOrchestrator;

impl AgentOrchestrator {
    pub fn new() -> Self {
        Self
    }
}

impl Default for AgentOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}
