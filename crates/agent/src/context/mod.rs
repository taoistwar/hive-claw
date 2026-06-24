//! Agent Context module.
//!
//! This module contains two distinct concerns:
//!
//! 1. **ContextBuilder** (`builder.rs`) — the existing prompt assembly system
//!    for assembling agent system prompts and LLM messages.
//!
//! 2. **AgentContext** (`core.rs` and submodules) — the new unified state
//!    carrier for agent execution lifecycle, as specified in
//!    `specs/009-agent-context/`.
//!
//! The two are intentionally separate. ContextBuilder handles prompt
//! construction, while AgentContext manages execution state.

// Existing prompt assembly
pub mod builder;
pub use builder::{BOOTSTRAP_FILES, ContextBuilder, RUNTIME_CONTEXT_TAG};

// New Agent Context state management
pub mod audit;
pub mod category;
pub mod config;
pub mod core;
pub mod hook;
pub mod lock;
pub mod merge;
pub mod prompt;
pub mod read_view;
pub mod response;
pub mod serialize;

// Re-exports for convenience
pub use audit::{
    AuditLogger, AuditRecord, SkillExecutionRecord, SkillExecutionStatus, StateChangeLog,
    ToolCallRecord,
};
pub use category::{Category, RecordEntry, ToolCallStatus};
pub use config::ContextConfig;
pub use core::{AgentContext, ContextError, LifecycleState, UserInput};
pub use hook::AgentContextSyncHook;
pub use lock::CategoryLock;
pub use merge::SubagentContext;
pub use prompt::{PromptData, build_prompt_budget};
pub use read_view::ReadView;
pub use response::{ExtensionContent, ExtensionType, ObjectRef, ResponsePayload};
pub use serialize::ContextSnapshot;
