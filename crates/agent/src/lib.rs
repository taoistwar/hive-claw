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

pub mod autocompact;
pub mod context;
pub mod dream;
pub mod hook;
pub mod loop_;
pub mod memory;
pub mod prefilter;
pub mod runner;
pub mod skills;
pub mod subagent;
pub mod tools;

pub use autocompact::{AutoCompact, Consolidator as AutoCompactConsolidator};
pub use context::ContextBuilder;
pub use hook::{AgentHook, AgentHookContext, CompositeHook, ToolEvent};
pub use loop_::{AgentLoop, CommandPrefilter, LoopConfig};
pub use prefilter::{AgentRuntimeInfo, BuiltinPrefilter};
pub use dream::{DreamConfig, MemoryDream};
pub use memory::{Consolidator, Dream, MemoryStore, PromptSizeEstimate};
pub use runner::{AgentRunResult, AgentRunSpec, AgentRunner};
pub use skills::{SkillEntry, SkillSource, SkillsLoader};
pub use subagent::{SubagentConfig, SubagentManager, SubagentStatus};
pub use tools::{PrepareCallResult, Tool, ToolExecError, ToolRegistry};
