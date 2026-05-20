//! Agent tool traits + bundled built-in tools.
//! Port of `nanobot.agent.tools` package root.
//!
//! Notes on deferred items:
//! - `mcp.rs` — MCP protocol client (Python original ~625 lines) is
//!   deferred until the wider MCP crate lands; callers can register
//!   [`Tool`] impls directly.
//! - `self.rs` (MyTool) — Python-specific runtime-introspection tool.
//!   Rust needs a fundamentally different design (no reflection), so
//!   it's deferred behind `AgentLoop` accessors.

pub mod base;
pub mod context;
pub mod cron;
pub mod factory;
pub mod file_state;
pub mod filesystem;
pub mod message;
pub mod notebook;
pub mod path_utils;
pub mod registry;
pub mod runtime_state;
pub mod sandbox;
pub mod schema;
pub mod search;
pub mod shell;
pub mod spawn;
pub mod web;
pub mod web_search_ddg;

pub use base::{Tool, ToolExecError};
pub use context::{ContextAware, RequestContext, ToolContext};
pub use runtime_state::RuntimeState;
pub use cron::CronTool;
pub use factory::{BuiltinToolSet, ToolFactoryConfig, ToolFactoryDeps};
pub use filesystem::{EditFileTool, FsTool, ListDirTool, ReadFileTool, WriteFileTool};
pub use message::{MessageContext, MessageTool};
pub use notebook::NotebookEditTool;
pub use registry::{PrepareCallResult, ToolRegistry};
pub use sandbox::{PathError, resolve_path, wrap_command};
pub use schema::{fragment_of, resolve_json_schema_type, validate_json_schema_value};
pub use search::GrepTool;
pub use shell::ExecTool;
pub use spawn::{SpawnCallback, SpawnContext, SpawnRequest, SpawnTool};
pub use web::{
    UnavailableBackend as WebSearchUnavailable, WebFetchTool, WebSearchBackend, WebSearchItem,
    WebSearchTool,
};
pub use web_search_ddg::DuckDuckGoBackend;
pub use file_state::{FileStateStore, FileStates};
