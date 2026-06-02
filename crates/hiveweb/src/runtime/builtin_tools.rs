//! Builtin tool definitions — metadata-only wrappers for agent tools.
//!
//! These correspond to the `Tool` implementations in `crates/agent/src/tools/`.
//! Each entry captures: identifier, name, description, input_schema, output_schema.
//! Execution is delegated to the agent process (via `invoke_function` with
//! `function_id` pointing to a kind=2 custom function backed by a plugin, or
//! via future host-native implementations).

// ---------------------------------------------------------------------------
// Helper — schema constants as &'static str so they can be CAST('...' AS JSON)
// ---------------------------------------------------------------------------

pub const READ_FILE_INPUT_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "path": { "type": "string", "description": "The file path to read" },
    "offset": { "type": "integer", "description": "Line number to start reading from (1-indexed, default 1)", "minimum": 1, "default": 1 },
    "limit": { "type": "integer", "description": "Maximum number of lines to read (default 2000)", "minimum": 1 },
    "pages": { "type": "string", "description": "Page range for PDF files, e.g. '1-5' (default: all, max 20 pages)" }
  },
  "required": ["path"]
}"#;

pub const READ_FILE_OUTPUT_SCHEMA: &str = r#"{ "type": "string", "description": "File content (LINE_NUM|CONTENT format for text, or visual content for images)" }"#;

pub const WRITE_FILE_INPUT_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "path": { "type": "string", "description": "The file path to write to" },
    "content": { "type": "string", "description": "The content to write" }
  },
  "required": ["path", "content"]
}"#;

pub const WRITE_FILE_OUTPUT_SCHEMA: &str =
    r#"{ "type": "string", "description": "Confirmation or error message" }"#;

pub const EDIT_FILE_INPUT_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "path": { "type": "string", "description": "The file path to edit" },
    "old_text": { "type": "string", "description": "Exact text to find (empty to create a new file)" },
    "new_text": { "type": "string", "description": "Replacement text (empty to delete)" },
    "replace_all": { "type": "boolean", "description": "Replace all occurrences (default false)", "default": false }
  },
  "required": ["path", "old_text", "new_text"]
}"#;

pub const EDIT_FILE_OUTPUT_SCHEMA: &str =
    r#"{ "type": "string", "description": "Confirmation or error message" }"#;

pub const LIST_DIR_INPUT_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "path": { "type": "string", "description": "The directory path to list" },
    "recursive": { "type": "boolean", "description": "Recursively list all files (default false)", "default": false },
    "max_entries": { "type": "integer", "description": "Maximum entries to return (default 200)", "minimum": 1, "default": 200 }
  },
  "required": ["path"]
}"#;

pub const LIST_DIR_OUTPUT_SCHEMA: &str = r#"{ "type": "string", "description": "Directory listing (file/directory names with metadata)" }"#;

pub const WEB_FETCH_INPUT_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "url": { "type": "string", "description": "URL to fetch" },
    "extractMode": { "type": "string", "enum": ["markdown", "text"], "default": "text" },
    "maxChars": { "type": "integer", "minimum": 100 }
  },
  "required": ["url"]
}"#;

pub const WEB_FETCH_OUTPUT_SCHEMA: &str =
    r#"{ "type": "string", "description": "Extracted text content from the page" }"#;

pub const WEB_SEARCH_INPUT_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "query": { "type": "string", "description": "Search query" },
    "count": { "type": "integer", "minimum": 1, "maximum": 10, "description": "Number of results (1-10)" }
  },
  "required": ["query"]
}"#;

pub const WEB_SEARCH_OUTPUT_SCHEMA: &str = r#"{ "type": "array", "items": { "type": "object" }, "description": "Search results (titles, URLs, snippets)" }"#;

pub const EXEC_INPUT_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "command": { "type": "string", "description": "Shell command to execute" },
    "timeout": { "type": "integer", "minimum": 1, "description": "Timeout in seconds (default 60, max 600)", "default": 60 },
    "working_dir": { "type": "string", "description": "Working directory for the command" }
  },
  "required": ["command"]
}"#;

pub const EXEC_OUTPUT_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "exit_code": { "type": "integer" },
    "stdout": { "type": "string" },
    "stderr": { "type": "string" }
  }
}"#;

pub const GREP_INPUT_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "pattern": { "type": "string", "description": "Regex pattern to search for (Rust regex syntax)" },
    "path": { "type": "string", "description": "File or directory to search in (default: workspace root)" },
    "include": { "type": "string", "description": "Glob pattern to filter files, e.g. '*.rs'" },
    "exclude": { "type": "string", "description": "Glob pattern to exclude files" },
    "case_sensitive": { "type": "boolean", "description": "Case-sensitive search (default false)", "default": false },
    "head_limit": { "type": "integer", "minimum": 1, "description": "Maximum matches to return (default 250)", "default": 250 }
  },
  "required": ["pattern"]
}"#;

pub const GREP_OUTPUT_SCHEMA: &str =
    r#"{ "type": "string", "description": "Match lines in FILE:LINE:CONTENT format" }"#;

pub const MY_INPUT_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "action": { "type": "string", "enum": ["list", "status", "kill", "cancel", "update_prompt", "get_field", "set_field", "list_vars", "set_var", "get_var"], "description": "Action to perform on subagents or agent state" },
    "subagent_id": { "type": "string", "description": "Target subagent ID for status/kill/cancel" },
    "prompt": { "type": "string", "description": "New system prompt for update_prompt" },
    "field_name": { "type": "string", "description": "Field name for get_field/set_field" },
    "field_value": {},
    "var_name": { "type": "string", "description": "Variable name for get_var/set_var/list_vars" },
    "var_value": {},
    "message": { "type": "string", "description": "Message to send to subagent" }
  },
  "required": ["action"]
}"#;

pub const MY_OUTPUT_SCHEMA: &str =
    r#"{ "type": "object", "description": "Action result (varies by action)" }"#;

pub const NOTEBOOK_EDIT_INPUT_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "notebook": { "type": "string", "description": "Path to the .ipynb notebook file" },
    "cell_index": { "type": "integer", "description": "Index of cell to edit (0-based). -1 to append" },
    "cell_type": { "type": "string", "enum": ["code", "markdown"], "description": "Type of cell (required for new cells)" },
    "source": { "type": "string", "description": "New cell source code" },
    "edit_type": { "type": "string", "enum": ["replace", "insert", "delete"], "description": "Type of edit operation", "default": "replace" }
  },
  "required": ["notebook", "source"]
}"#;

pub const NOTEBOOK_EDIT_OUTPUT_SCHEMA: &str =
    r#"{ "type": "string", "description": "Confirmation of notebook edit" }"#;

pub const SPAWN_INPUT_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "task": { "type": "string", "description": "Task description for the subagent" },
    "label": { "type": "string", "description": "Human-readable label for this spawn" }
  },
  "required": ["task"]
}"#;

pub const SPAWN_OUTPUT_SCHEMA: &str =
    r#"{ "type": "string", "description": "Spawn status/result message" }"#;

pub const CRON_INPUT_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "action": { "type": "string", "enum": ["list", "add", "remove"], "description": "Action to perform on cron jobs" },
    "cron_expression": { "type": "string", "description": "Cron schedule expression (e.g. '0 9 * * *')" },
    "task": { "type": "string", "description": "Task description to execute on schedule" },
    "job_id": { "type": "string", "description": "Job ID for remove action" }
  },
  "required": ["action"]
}"#;

pub const CRON_OUTPUT_SCHEMA: &str = r#"{ "type": "array", "items": { "type": "object" }, "description": "Cron job list or status" }"#;

pub const GENERATE_IMAGE_INPUT_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "prompt": { "type": "string", "description": "Text prompt describing the image to generate" },
    "model": { "type": "string", "description": "Image generation model to use" },
    "size": { "type": "string", "description": "Output image size (e.g. '1024x1024')" },
    "n": { "type": "integer", "minimum": 1, "maximum": 10, "description": "Number of images to generate", "default": 1 }
  },
  "required": ["prompt"]
}"#;

pub const GENERATE_IMAGE_OUTPUT_SCHEMA: &str =
    r#"{ "type": "object", "description": "Generated image URL(s) or local path(s)" }"#;

pub const MESSAGE_INPUT_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "content": { "type": "string", "description": "Message content to send" },
    "channel": { "type": "string", "description": "Channel to send message to (e.g. 'webui', 'discord')" },
    "reply_to": { "type": "string", "description": "Message ID to reply to" }
  },
  "required": ["content"]
}"#;

pub const MESSAGE_OUTPUT_SCHEMA: &str =
    r#"{ "type": "object", "description": "Message send status" }"#;

pub const LONG_TASK_INPUT_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "action": { "type": "string", "enum": ["start", "status", "list", "done"], "description": "Long task action" },
    "task_id": { "type": "string", "description": "Task ID for status/done actions" },
    "description": { "type": "string", "description": "Description for start action" }
  },
  "required": ["action"]
}"#;

pub const LONG_TASK_OUTPUT_SCHEMA: &str =
    r#"{ "type": "object", "description": "Long task status or result" }"#;

pub const COMPLETE_GOAL_INPUT_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "goal": { "type": "string", "description": "The goal that has been completed" },
    "status": { "type": "string", "enum": ["success", "failure", "partial"], "description": "Completion status", "default": "success" },
    "summary": { "type": "string", "description": "Summary of what was accomplished" }
  },
  "required": ["goal"]
}"#;

pub const COMPLETE_GOAL_OUTPUT_SCHEMA: &str =
    r#"{ "type": "string", "description": "Goal completion confirmation" }"#;

// ---------------------------------------------------------------------------
// Definition struct and registry
// ---------------------------------------------------------------------------

use crate::runtime::capability;

pub struct BuiltinToolDef {
    pub identifier: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: &'static str,
    pub output_schema: &'static str,
    /// Capabilities required at runtime.
    pub required_capabilities: &'static [&'static str],
    /// Whether this tool is always available to all agents (is_always flag).
    pub is_always: bool,
}

pub const BUILTIN_TOOLS: &[BuiltinToolDef] = &[
    // -- File system --
    BuiltinToolDef {
        identifier: "read_file",
        name: "Read File",
        description: "Read a file (text, image, or document). Text output format: LINE_NUM|CONTENT. Images return visual content for analysis. Supports PDF, DOCX, XLSX, PPTX documents. Use offset and limit for large text files.",
        input_schema: READ_FILE_INPUT_SCHEMA,
        output_schema: READ_FILE_OUTPUT_SCHEMA,
        required_capabilities: &[capability::FS_READ],
        is_always: false,
    },
    BuiltinToolDef {
        identifier: "write_file",
        name: "Write File",
        description: "Write content to a file. Overwrites if the file already exists; creates parent directories as needed. For partial edits, prefer edit_file instead.",
        input_schema: WRITE_FILE_INPUT_SCHEMA,
        output_schema: WRITE_FILE_OUTPUT_SCHEMA,
        required_capabilities: &[capability::FS_WRITE],
        is_always: false,
    },
    BuiltinToolDef {
        identifier: "edit_file",
        name: "Edit File",
        description: "Edit a file by replacing old_text with new_text. Use read_file first to verify content. Set replace_all=true to replace all occurrences.",
        input_schema: EDIT_FILE_INPUT_SCHEMA,
        output_schema: EDIT_FILE_OUTPUT_SCHEMA,
        required_capabilities: &[capability::FS_READ, capability::FS_WRITE],
        is_always: false,
    },
    BuiltinToolDef {
        identifier: "list_dir",
        name: "List Directory",
        description: "List the contents of a directory. Set recursive=true to explore nested structure. Common noise dirs (.git, node_modules, ...) are auto-ignored.",
        input_schema: LIST_DIR_INPUT_SCHEMA,
        output_schema: LIST_DIR_OUTPUT_SCHEMA,
        required_capabilities: &[capability::FS_READ],
        is_always: false,
    },
    // -- Web --
    BuiltinToolDef {
        identifier: "web_fetch",
        name: "Web Fetch",
        description: "Fetch a URL and extract readable text content. Output is capped at maxChars (default 50 000). May fail on login-walled or JS-heavy sites.",
        input_schema: WEB_FETCH_INPUT_SCHEMA,
        output_schema: WEB_FETCH_OUTPUT_SCHEMA,
        required_capabilities: &[capability::NETWORK_HTTP],
        is_always: false,
    },
    BuiltinToolDef {
        identifier: "web_search",
        name: "Web Search",
        description: "Search the web. Returns titles, URLs, and snippets. count defaults to 5 (max 10). Use web_fetch to read a specific page in full.",
        input_schema: WEB_SEARCH_INPUT_SCHEMA,
        output_schema: WEB_SEARCH_OUTPUT_SCHEMA,
        required_capabilities: &[capability::NETWORK_HTTP],
        is_always: false,
    },
    // -- Shell --
    BuiltinToolDef {
        identifier: "exec",
        name: "Execute Command",
        description: "Execute a shell command with configurable timeout and working directory. Returns exit code, stdout, and stderr. Hard policy boundary: commands must not escape the workspace.",
        input_schema: EXEC_INPUT_SCHEMA,
        output_schema: EXEC_OUTPUT_SCHEMA,
        required_capabilities: &[capability::EXEC_RUN],
        is_always: false,
    },
    // -- Search --
    BuiltinToolDef {
        identifier: "grep",
        name: "Grep",
        description: "Search file contents using Rust-syntax regex. Supports file/directory targeting, glob include/exclude filters, case sensitivity toggle, and head_limit.",
        input_schema: GREP_INPUT_SCHEMA,
        output_schema: GREP_OUTPUT_SCHEMA,
        required_capabilities: &[capability::FS_READ],
        is_always: false,
    },
    // -- Agent Self-Management --
    BuiltinToolDef {
        identifier: "my",
        name: "My (Agent Self-Management)",
        description: "Manage subagents and agent runtime state. Actions: list subagents, check status, kill/cancel subagents, update system prompt, get/set fields and runtime variables.",
        input_schema: MY_INPUT_SCHEMA,
        output_schema: MY_OUTPUT_SCHEMA,
        required_capabilities: &[capability::LOG_EMIT],
        is_always: false,
    },
    // -- Notebook --
    BuiltinToolDef {
        identifier: "notebook_edit",
        name: "Notebook Edit",
        description: "Edit Jupyter .ipynb notebooks. Supports replacing, inserting, and deleting cells of code or markdown type.",
        input_schema: NOTEBOOK_EDIT_INPUT_SCHEMA,
        output_schema: NOTEBOOK_EDIT_OUTPUT_SCHEMA,
        required_capabilities: &[capability::FS_READ, capability::FS_WRITE],
        is_always: false,
    },
    // -- Spawn --
    BuiltinToolDef {
        identifier: "spawn",
        name: "Spawn Subagent",
        description: "Launch a background subagent to handle a complex or long-running task independently.",
        input_schema: SPAWN_INPUT_SCHEMA,
        output_schema: SPAWN_OUTPUT_SCHEMA,
        required_capabilities: &[capability::AGENT_SPAWN],
        is_always: false,
    },
    // -- Cron --
    BuiltinToolDef {
        identifier: "cron",
        name: "Cron Jobs",
        description: "Manage scheduled cron jobs. Actions: list existing jobs, add new jobs with cron expression, or remove jobs by ID.",
        input_schema: CRON_INPUT_SCHEMA,
        output_schema: CRON_OUTPUT_SCHEMA,
        required_capabilities: &[capability::CRON_MANAGE],
        is_always: false,
    },
    // -- Image Generation --
    BuiltinToolDef {
        identifier: "generate_image",
        name: "Generate Image",
        description: "Generate images from text prompts using configured image generation providers.",
        input_schema: GENERATE_IMAGE_INPUT_SCHEMA,
        output_schema: GENERATE_IMAGE_OUTPUT_SCHEMA,
        required_capabilities: &[capability::LLM_INVOKE],
        is_always: false,
    },
    // -- Messaging --
    BuiltinToolDef {
        identifier: "message",
        name: "Send Message",
        description: "Send a message to a specific channel (e.g. webui, discord). Supports reply-to threading.",
        input_schema: MESSAGE_INPUT_SCHEMA,
        output_schema: MESSAGE_OUTPUT_SCHEMA,
        required_capabilities: &[],
        is_always: false,
    },
    // -- Long Task --
    BuiltinToolDef {
        identifier: "long_task",
        name: "Long Task",
        description: "Manage long-running tasks. Actions: start a new task, check status, list active tasks, or mark done.",
        input_schema: LONG_TASK_INPUT_SCHEMA,
        output_schema: LONG_TASK_OUTPUT_SCHEMA,
        required_capabilities: &[],
        is_always: false,
    },
    BuiltinToolDef {
        identifier: "complete_goal",
        name: "Complete Goal",
        description: "Mark a goal as completed with status (success/failure/partial) and a summary of accomplishments.",
        input_schema: COMPLETE_GOAL_INPUT_SCHEMA,
        output_schema: COMPLETE_GOAL_OUTPUT_SCHEMA,
        required_capabilities: &[],
        is_always: false,
    },
];

/// Startup idempotent upsert — registers all builtin tools as functions (kind=1)
/// and corresponding tools (source='builtin', kind=1).
pub async fn ensure_registered(pool: &sqlx::MySqlPool) -> Result<(), sqlx::Error> {
    for bt in BUILTIN_TOOLS {
        let caps_json =
            serde_json::to_string(bt.required_capabilities).unwrap_or_else(|_| "[]".to_string());

        // 1. Upsert function (kind=1 = builtin, no plugin needed)
        sqlx::query(
            r#"INSERT INTO functions
               (identifier, name, description, kind, input_schema, output_schema,
                required_capabilities, plugin_id, plugin_export)
               VALUES (?, ?, ?, 1, CAST(? AS JSON), CAST(? AS JSON), CAST(? AS JSON), NULL, NULL)
               ON DUPLICATE KEY UPDATE
                 name = VALUES(name),
                 description = VALUES(description),
                 input_schema = VALUES(input_schema),
                 output_schema = VALUES(output_schema),
                 required_capabilities = VALUES(required_capabilities)"#,
        )
        .bind(bt.identifier)
        .bind(bt.name)
        .bind(bt.description)
        .bind(bt.input_schema)
        .bind(bt.output_schema)
        .bind(&caps_json)
        .execute(pool)
        .await?;

        // 2. Upsert corresponding builtin tool (source='builtin', kind=1, wraps the function)
        sqlx::query(
            r#"INSERT INTO tools
               (identifier, name, description, kind, source, is_always,
                function_id, workflow_id, input_schema, output_schema, required_capabilities)
               VALUES (?, ?, ?, 1, 'builtin', ?,
                       (SELECT id FROM functions WHERE identifier = ? AND kind = 1 LIMIT 1),
                       NULL, CAST(? AS JSON), CAST(? AS JSON), CAST(? AS JSON))
               ON DUPLICATE KEY UPDATE
                 name = VALUES(name),
                 description = VALUES(description),
                 is_always = VALUES(is_always),
                 function_id = (SELECT id FROM functions WHERE identifier = ? AND kind = 1 LIMIT 1),
                 input_schema = VALUES(input_schema),
                 output_schema = VALUES(output_schema),
                 required_capabilities = VALUES(required_capabilities)"#,
        )
        .bind(bt.identifier)
        .bind(bt.name)
        .bind(bt.description)
        .bind(bt.is_always)
        .bind(bt.identifier)
        .bind(bt.input_schema)
        .bind(bt.output_schema)
        .bind(&caps_json)
        .bind(bt.identifier)
        .execute(pool)
        .await?;
    }

    tracing::info!(
        count = BUILTIN_TOOLS.len(),
        "builtin agent tools (functions + tool wrappers) upserted"
    );
    Ok(())
}
