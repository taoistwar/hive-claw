//! Agent tool traits + bundled built-in tools.
//! Port of `nanobot.agent.tools` package root.
//!
//! Notes on deferred items:
//! - `mcp.rs` — MCP protocol client: partial port with trait abstractions.
//!   Full implementation requires MCP SDK integration.
//! - `self.rs` (MyTool) — ported with RuntimeState field accessors; requires
//!   `RuntimeState` to expose `get_field`/`set_field`/`get_runtime_vars`/`set_runtime_var`.

pub mod base;
pub mod context;
pub mod cron;
pub mod file_state;
pub mod filesystem;
pub mod image_generation;
pub mod loader;
pub mod long_task;
pub mod mcp;
pub mod message;
pub mod notebook;
pub mod path_utils;
pub mod registry;
pub mod runtime_state;
pub mod sandbox;
pub mod schema;
pub mod search;
pub mod self_;
pub mod shell;
pub mod spawn;
pub mod web;

pub use base::{Tool, ToolExecError};
pub use context::{ContextAware, RequestContext, ToolContext};
pub use runtime_state::RuntimeState;
pub use cron::CronTool;
pub use filesystem::{EditFileTool, FsTool, ListDirTool, ReadFileTool, WriteFileTool};
pub use image_generation::{
    ImageGenerationTool, ImageGenerationToolConfig, ImageGenerationProvider,
    ImageGenerationProviderConfig, ImageGenerationResponse, ImageGenerationError,
    get_image_gen_provider, store_generated_image_artifact, generated_image_tool_result,
    detect_image_mime, get_media_dir, ArtifactError,
};
pub use loader::{ToolLoader, ToolConstructor};
pub use long_task::{
    LongTaskTool, CompleteGoalTool, GoalState, parse_goal_state, goal_state_raw,
    goal_state_ws_blob, discard_legacy_goal_state_key, Session,
};
pub use mcp::{
    MCPToolWrapper, MCPResourceWrapper, MCPPromptWrapper, McpServerConfig, McpSession,
    McpToolDefinition, McpResourceDefinition, McpPromptDefinition, McpPromptArgument,
    McpToolResult, McpResourceResult, McpPromptResult, McpContentBlock, McpResourceContent,
    McpPromptMessage, McpError, connect_mcp_servers, sanitize_name, normalize_schema_for_openai,
    probe_http_url, normalize_windows_stdio_command, McpServerHandle,
};
pub use message::{MessageContext, MessageTool};
pub use notebook::NotebookEditTool;
pub use registry::{PrepareCallResult, ToolRegistry};
pub use sandbox::{PathError, resolve_path, wrap_command};
pub use schema::{fragment_of, resolve_json_schema_type, validate_json_schema_value};
pub use search::GrepTool;
pub use self_::{MyTool, MyToolConfig, SubagentStatus, SubagentManager};
pub use shell::ExecTool;
pub use spawn::{SpawnCallback, SpawnContext, SpawnRequest, SpawnTool};
pub use web::{
    DuckDuckGoBackend, UnavailableBackend as WebSearchUnavailable, WebFetchTool,
    WebSearchBackend, WebSearchItem, WebSearchTool,
};
pub use file_state::{FileStateStore, FileStates};
