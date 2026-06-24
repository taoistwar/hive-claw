//! Rust port of the `nanobot.agent` Python package.
//!
//! Organizes the agent runtime into focused modules:
//! - [`hook`]: lifecycle hooks around the LLM loop
//! - [`tools`]: the `Tool` trait, schema validation, and [`ToolRegistry`]
//! - [`skills`]: workspace + built-in skill discovery
//! - [`context`]: system-prompt / message assembly
//! - [`memory`]: MEMORY.md + history.jsonl I/O
//! - [`autocompact`]: proactive session compaction
//! - [`runner`]: the core model/tool step loop
//! - [`subagent`]: orchestration of background subagents
//! - [`loop_`]: simplified end-to-end orchestration
//! - [`model_presets`]: runtime model preset selection helpers

pub mod autocompact;
pub mod context;
pub mod hook;
pub mod loop_;
pub mod memory;
pub mod model_presets;
pub mod progress_hook;
pub mod runner;
pub mod skills;
pub mod subagent;
pub mod tools;

pub use autocompact::{AutoCompact, Consolidator as AutoCompactConsolidator};
pub use context::ContextBuilder;
pub use hook::{AgentHook, AgentHookContext, CompositeHook, ToolEvent};
pub use loop_::{
    AgentLoop, AgentRuntimeInfo, BuiltinPrefilter, BuiltinToolSet, CommandPrefilter, CommandRouter,
    LoopConfig, ProviderSnapshot, StateTraceEntry, ToolFactoryConfig, ToolFactoryDeps, TurnContext,
    TurnState, UNIFIED_SESSION_KEY, WebuiTurnCoordinator,
};
pub use memory::{Consolidator, Dream, DreamConfig, MemoryDream, MemoryStore, PromptSizeEstimate};
pub use model_presets::{
    ModelPresetConfig, PresetSnapshotLoader, build_runtime_preset_snapshot,
    build_static_preset_snapshot, configured_model_presets, default_selection_signature,
    make_preset_snapshot_loader, normalize_preset_name,
};
pub use progress_hook::{ProgressHook, ProgressPayload};
pub use runner::{AgentRunResult, AgentRunSpec, AgentRunner};
pub use skills::{SkillEntry, SkillSource, SkillsLoader};
pub use subagent::{SubagentConfig, SubagentManager, SubagentStatus};
pub use tools::{PrepareCallResult, Tool, ToolExecError, ToolRegistry};
