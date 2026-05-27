//! Agent Runtime 模块（004 / plan §Project Structure）
//!
//! 子模块：
//! - `capability` — 11 个静态 capability 名 + CapabilityRegistry
//! - `pool` — Plugin Instance Pool（多 Plugin 共享 + 每 Plugin 子池）
//! - `invoker` — Plugin 调用入口（resolve → acquire → invoke → audit）
//! - `workflow` — DAG 执行器（拓扑 BFS + 并行）
//! - `agent` — Agent 编排（复用 crates/agent）
//! - `llm` — LLM preset 注册表（复用 crates/providers + FallbackProvider）

pub mod agent;
pub mod builtins;
pub mod capabilities;
pub mod capability;
pub mod invoker;
pub mod llm;
pub mod orchestrator;
pub mod pool;
pub mod wasm_exports;
pub mod workflow;

pub use capability::{CapabilityRegistry, CAPABILITIES};
pub use invoker::{HostInvocationCtx, Invoker, InvokerError};
pub use llm::{LlmAdapterError, LlmPresetName, LlmRegistry, PresetEntry};
pub use pool::{InstancePool, PerPluginMetrics, PluginPool, PoolConfig, PoolMetrics};
pub use workflow::{WorkflowError, WorkflowExecutor};

use std::sync::Arc;

/// 启动期注入到 `AppState.runtime_state` 的运行时句柄集合
#[derive(Clone)]
pub struct RuntimeState {
    pub capabilities: Arc<CapabilityRegistry>,
    pub pool: Arc<InstancePool>,
    pub invoker: Arc<Invoker>,
    pub workflows: Arc<WorkflowExecutor>,
    pub llm: Arc<LlmRegistry>,
}

impl std::fmt::Debug for RuntimeState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeState")
            .field("capabilities", &"<registry>")
            .field("pool", &"<pool>")
            .field("invoker", &"<invoker>")
            .field("workflows", &"<executor>")
            .field("llm", &"<llm>")
            .finish()
    }
}
