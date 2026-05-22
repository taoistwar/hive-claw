use async_trait::async_trait;
use log::{debug, error, warn};
use regex::Regex;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::LazyLock;

use super::base::{Tool, ToolExecError};
use super::registry::ToolRegistry;

const TRANSIENT_EXC_NAMES: &[&str] = &[
    "ClosedResourceError",
    "BrokenResourceError",
    "EndOfStream",
    "BrokenPipeError",
    "ConnectionResetError",
    "ConnectionRefusedError",
    "ConnectionAbortedError",
    "ConnectionError",
];

const WINDOWS_SHELL_LAUNCHERS: &[&str] = &["npx", "npm", "pnpm", "yarn", "bunx"];

static SANITIZE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"_+").unwrap());

pub struct McpServerConfig {
    pub transport_type: Option<String>,
    pub command: Option<String>,
    pub args: Option<Vec<String>>,
    pub env: Option<HashMap<String, String>>,
    pub url: Option<String>,
    pub headers: Option<HashMap<String, String>>,
    pub enabled_tools: Vec<String>,
    pub tool_timeout: u64,
}

#[async_trait]
pub trait McpSession: Send + Sync {
    async fn call_tool(
        &self,
        name: String,
        arguments: HashMap<String, Value>,
        timeout_secs: u64,
    ) -> Result<McpToolResult, McpError>;

    async fn read_resource(
        &self,
        uri: String,
        timeout_secs: u64,
    ) -> Result<McpResourceResult, McpError>;

    async fn get_prompt(
        &self,
        name: String,
        arguments: HashMap<String, Value>,
        timeout_secs: u64,
    ) -> Result<McpPromptResult, McpError>;

    async fn list_tools(
        &self,
    ) -> Result<Vec<McpToolDefinition>, McpError>;

    async fn list_resources(
        &self,
    ) -> Result<Vec<McpResourceDefinition>, McpError>;

    async fn list_prompts(
        &self,
    ) -> Result<Vec<McpPromptDefinition>, McpError>;
}

pub struct McpToolDefinition {
    pub name: String,
    pub description: Option<String>,
    pub input_schema: Option<Value>,
}

pub struct McpResourceDefinition {
    pub uri: String,
    pub name: String,
    pub description: Option<String>,
}

pub struct McpPromptDefinition {
    pub name: String,
    pub description: Option<String>,
    pub arguments: Vec<McpPromptArgument>,
}

pub struct McpPromptArgument {
    pub name: String,
    pub description: Option<String>,
    pub required: bool,
}

pub struct McpToolResult {
    pub content: Vec<McpContentBlock>,
}

pub struct McpResourceResult {
    pub contents: Vec<McpResourceContent>,
}

pub struct McpPromptResult {
    pub messages: Vec<McpPromptMessage>,
}

pub enum McpContentBlock {
    Text(String),
    Other(String),
}

pub enum McpResourceContent {
    Text(String),
    Blob(usize),
    Other(String),
}

pub enum McpPromptMessage {
    Text(String),
    Other(String),
}

pub struct McpError {
    pub code: Option<i32>,
    pub message: String,
    pub is_transient: bool,
}

impl std::fmt::Display for McpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::fmt::Debug for McpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "McpError: {}", self.message)
    }
}

impl std::error::Error for McpError {}

pub fn sanitize_name(name: &str) -> String {
    let replaced = Regex::new(r"[^a-zA-Z0-9_-]")
        .unwrap()
        .replace_all(name, "_");
    SANITIZE_RE.replace_all(&replaced, "_").to_string()
}

fn is_transient(exc_type_name: &str) -> bool {
    TRANSIENT_EXC_NAMES.contains(&exc_type_name)
}

pub async fn probe_http_url(_url: &str, _timeout_secs: f64) -> bool {
    todo!("TODO: implement TCP probe to check if HTTP MCP server is reachable")
}

fn windows_command_basename(command: &str) -> String {
    command
        .replace("\\", "/")
        .rsplit_once("/")
        .map(|(_, s)| s.to_lowercase())
        .unwrap_or_else(|| command.to_lowercase())
}

pub fn normalize_windows_stdio_command(
    command: &str,
    args: Option<&[String]>,
    env: Option<&HashMap<String, String>>,
) -> (String, Vec<String>, Option<HashMap<String, String>>) {
    #[cfg(not(windows))]
    {
        return (
            command.to_string(),
            args.unwrap_or(&[]).to_vec(),
            env.cloned(),
        );
    }

    #[cfg(windows)]
    {
        let normalized_args = args.unwrap_or(&[]).to_vec();
        let basename = windows_command_basename(command);

        if matches!(
            basename.as_str(),
            "cmd" | "cmd.exe" | "powershell" | "powershell.exe" | "pwsh" | "pwsh.exe"
        ) {
            return (command.to_string(), normalized_args, env.cloned());
        }

        if basename.ends_with(".exe") || basename.ends_with(".com") {
            return (command.to_string(), normalized_args, env.cloned());
        }

        let resolved = command.to_string();
        let resolved_basename = windows_command_basename(&resolved);

        let should_wrap = basename.ends_with(".cmd")
            || basename.ends_with(".bat")
            || WINDOWS_SHELL_LAUNCHERS.contains(&basename.as_str())
            || resolved_basename.ends_with(".cmd")
            || resolved_basename.ends_with(".bat");

        if !should_wrap {
            return (command.to_string(), normalized_args, env.cloned());
        }

        let comspec = env
            .and_then(|e| e.get("COMSPEC"))
            .cloned()
            .or_else(|| std::env::var("COMSPEC").ok())
            .unwrap_or_else(|| "cmd.exe".to_string());

        let mut new_args = vec!["/d".to_string(), "/c".to_string(), command.to_string()];
        new_args.extend(normalized_args);

        (comspec, new_args, env.cloned())
    }
}

fn extract_nullable_branch(options: &Value) -> Option<(Value, bool)> {
    let arr = match options.as_array() {
        Some(a) => a,
        None => return None,
    };

    let mut non_null: Vec<Value> = Vec::new();
    let mut saw_null = false;

    for option in arr {
        let obj = match option.as_object() {
            Some(o) => o,
            None => return None,
        };
        if obj.get("type").and_then(|v| v.as_str()) == Some("null") {
            saw_null = true;
            continue;
        }
        non_null.push(option.clone());
    }

    if saw_null && non_null.len() == 1 {
        Some((non_null.remove(0), true))
    } else {
        None
    }
}

pub fn normalize_schema_for_openai(schema: &Value) -> Value {
    let obj = match schema.as_object() {
        Some(o) => o.clone(),
        None => return json!({"type": "object", "properties": {}}),
    };

    let mut normalized = obj;

    if let Some(raw_type) = normalized.get("type").and_then(|v| v.as_array()) {
        let non_null: Vec<&Value> = raw_type.iter().filter(|i| i.as_str() != Some("null")).collect();
        if raw_type.iter().any(|i| i.as_str() == Some("null")) && non_null.len() == 1 {
            normalized.insert("type".into(), non_null[0].clone());
            normalized.insert("nullable".into(), Value::Bool(true));
        }
    }

    for key in &["oneOf", "anyOf"] {
        if let Some(nullable_branch) = normalized.get(*key).and_then(extract_nullable_branch) {
            let (branch, _) = nullable_branch;
            if let Some(branch_obj) = branch.as_object() {
                let mut merged: serde_json::Map<String, Value> = normalized
                    .iter()
                    .filter(|(k, _)| k.as_str() != *key)
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                for (k, v) in branch_obj {
                    merged.insert(k.clone(), v.clone());
                }
                merged.insert("nullable".into(), Value::Bool(true));
                normalized = merged;
            }
            break;
        }
    }

    if let Some(props) = normalized.get("properties").and_then(|v| v.as_object()) {
        let new_props: serde_json::Map<String, Value> = props
            .iter()
            .map(|(name, prop)| {
                let normalized_prop = if prop.is_object() {
                    normalize_schema_for_openai(prop)
                } else {
                    prop.clone()
                };
                (name.clone(), normalized_prop)
            })
            .collect();
        normalized.insert("properties".into(), Value::Object(new_props));
    }

    if let Some(items) = normalized.get("items").and_then(|v| v.as_object()) {
        let normalized_items = normalize_schema_for_openai(&Value::Object(items.clone()));
        normalized.insert("items".into(), normalized_items);
    }

    if normalized.get("type").and_then(|v| v.as_str()) != Some("object") {
        return Value::Object(normalized);
    }

    normalized.insert("properties".to_string(), json!({}));
    normalized.insert("required".to_string(), json!([]));

    Value::Object(normalized)
}

use serde_json::json;

pub struct MCPToolWrapper {
    session: Arc<dyn McpSession>,
    original_name: String,
    name: String,
    description: String,
    parameters: Value,
    tool_timeout: u64,
}

impl MCPToolWrapper {
    pub fn new(
        session: Arc<dyn McpSession>,
        server_name: &str,
        tool_def: McpToolDefinition,
        tool_timeout: u64,
    ) -> Self {
        let original_name = tool_def.name.clone();
        let name = sanitize_name(&format!("mcp_{}_{}", server_name, tool_def.name));
        let description = tool_def.description.unwrap_or_else(|| tool_def.name.clone());
        let raw_schema = tool_def.input_schema.unwrap_or_else(|| json!({"type": "object", "properties": {}}));
        let parameters = normalize_schema_for_openai(&raw_schema);

        Self {
            session,
            original_name,
            name,
            description,
            parameters,
            tool_timeout,
        }
    }
}

#[async_trait]
impl Tool for MCPToolWrapper {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> String {
        self.description.clone()
    }

    fn parameters(&self) -> Value {
        self.parameters.clone()
    }

    fn plugin_discoverable(&self) -> bool {
        false
    }

    async fn execute(&self, params: Value) -> Result<Value, ToolExecError> {
        let args: HashMap<String, Value> = params
            .as_object()
            .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default();

        for attempt in 0..2 {
            let result = self
                .session
                .call_tool(self.original_name.clone(), args.clone(), self.tool_timeout)
                .await;

            match result {
                Ok(res) => {
                    let parts: Vec<String> = res
                        .content
                        .iter()
                        .map(|block| match block {
                            McpContentBlock::Text(t) => t.clone(),
                            McpContentBlock::Other(s) => s.clone(),
                        })
                        .collect();
                    let output = if parts.is_empty() {
                        "(no output)".into()
                    } else {
                        parts.join("\n")
                    };
                    return Ok(Value::String(output));
                }
                Err(e) => {
                    if e.is_transient && attempt == 0 {
                        warn!(
                            "MCP tool '{}' hit transient error ({}), retrying once...",
                            self.name, e.message
                        );
                        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                        continue;
                    }
                    error!("MCP tool '{}' failed: {}", self.name, e.message);
                    if e.is_transient {
                        return Ok(Value::String(format!(
                            "(MCP tool call failed after retry: {})",
                            e.message
                        )));
                    }
                    return Ok(Value::String(format!(
                        "(MCP tool call failed: {})",
                        e.message
                    )));
                }
            }
        }

        Ok(Value::String("(MCP tool call failed)".into()))
    }
}

pub struct MCPResourceWrapper {
    session: Arc<dyn McpSession>,
    uri: String,
    name: String,
    description: String,
    parameters: Value,
    resource_timeout: u64,
}

impl MCPResourceWrapper {
    pub fn new(
        session: Arc<dyn McpSession>,
        server_name: &str,
        resource_def: McpResourceDefinition,
        resource_timeout: u64,
    ) -> Self {
        let uri = resource_def.uri.clone();
        let name = sanitize_name(&format!("mcp_{}_resource_{}", server_name, resource_def.name));
        let desc = resource_def.description.unwrap_or_else(|| resource_def.name.clone());
        let description = format!("[MCP Resource] {}\nURI: {}", desc, uri);

        Self {
            session,
            uri,
            name,
            description,
            parameters: json!({"type": "object", "properties": {}, "required": []}),
            resource_timeout,
        }
    }
}

#[async_trait]
impl Tool for MCPResourceWrapper {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> String {
        self.description.clone()
    }

    fn parameters(&self) -> Value {
        self.parameters.clone()
    }

    fn plugin_discoverable(&self) -> bool {
        false
    }

    fn read_only(&self) -> bool {
        true
    }

    async fn execute(&self, _params: Value) -> Result<Value, ToolExecError> {
        for attempt in 0..2 {
            let result = self
                .session
                .read_resource(self.uri.clone(), self.resource_timeout)
                .await;

            match result {
                Ok(res) => {
                    let parts: Vec<String> = res
                        .contents
                        .iter()
                        .map(|block| match block {
                            McpResourceContent::Text(t) => t.clone(),
                            McpResourceContent::Blob(size) => format!("[Binary resource: {} bytes]", size),
                            McpResourceContent::Other(s) => s.clone(),
                        })
                        .collect();
                    let output = if parts.is_empty() {
                        "(no output)".into()
                    } else {
                        parts.join("\n")
                    };
                    return Ok(Value::String(output));
                }
                Err(e) => {
                    if e.is_transient && attempt == 0 {
                        warn!(
                            "MCP resource '{}' hit transient error ({}), retrying once...",
                            self.name, e.message
                        );
                        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                        continue;
                    }
                    error!("MCP resource '{}' failed: {}", self.name, e.message);
                    if e.is_transient {
                        return Ok(Value::String(format!(
                            "(MCP resource read failed after retry: {})",
                            e.message
                        )));
                    }
                    return Ok(Value::String(format!(
                        "(MCP resource read failed: {})",
                        e.message
                    )));
                }
            }
        }

        Ok(Value::String("(MCP resource read failed)".into()))
    }
}

pub struct MCPPromptWrapper {
    session: Arc<dyn McpSession>,
    prompt_name: String,
    name: String,
    description: String,
    parameters: Value,
    prompt_timeout: u64,
}

impl MCPPromptWrapper {
    pub fn new(
        session: Arc<dyn McpSession>,
        server_name: &str,
        prompt_def: McpPromptDefinition,
        prompt_timeout: u64,
    ) -> Self {
        let prompt_name = prompt_def.name.clone();
        let name = sanitize_name(&format!("mcp_{}_prompt_{}", server_name, prompt_def.name));
        let desc = prompt_def.description.unwrap_or_else(|| prompt_def.name.clone());
        let description = format!(
            "[MCP Prompt] {}\nReturns a filled prompt template that can be used as a workflow guide.",
            desc
        );

        let mut properties = serde_json::Map::new();
        let mut required: Vec<String> = Vec::new();

        for arg in &prompt_def.arguments {
            let mut prop = serde_json::Map::new();
            prop.insert("type".into(), Value::String("string".into()));
            if let Some(ref d) = arg.description {
                prop.insert("description".into(), Value::String(d.clone()));
            }
            properties.insert(arg.name.clone(), Value::Object(prop));
            if arg.required {
                required.push(arg.name.clone());
            }
        }

        let parameters = json!({
            "type": "object",
            "properties": properties,
            "required": required
        });

        Self {
            session,
            prompt_name,
            name,
            description,
            parameters,
            prompt_timeout,
        }
    }
}

#[async_trait]
impl Tool for MCPPromptWrapper {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> String {
        self.description.clone()
    }

    fn parameters(&self) -> Value {
        self.parameters.clone()
    }

    fn plugin_discoverable(&self) -> bool {
        false
    }

    fn read_only(&self) -> bool {
        true
    }

    async fn execute(&self, params: Value) -> Result<Value, ToolExecError> {
        let args: HashMap<String, Value> = params
            .as_object()
            .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default();

        for attempt in 0..2 {
            let result = self
                .session
                .get_prompt(self.prompt_name.clone(), args.clone(), self.prompt_timeout)
                .await;

            match result {
                Ok(res) => {
                    let parts: Vec<String> = res
                        .messages
                        .iter()
                        .flat_map(|msg| match msg {
                            McpPromptMessage::Text(t) => vec![t.clone()],
                            McpPromptMessage::Other(s) => vec![s.clone()],
                        })
                        .collect();
                    let output = if parts.is_empty() {
                        "(no output)".into()
                    } else {
                        parts.join("\n")
                    };
                    return Ok(Value::String(output));
                }
                Err(e) => {
                    if let Some(code) = e.code {
                        error!(
                            "MCP prompt '{}' failed: code={} message={}",
                            self.name, code, e.message
                        );
                        return Ok(Value::String(format!(
                            "(MCP prompt call failed: {} [code {}])",
                            e.message, code
                        )));
                    }
                    if e.is_transient && attempt == 0 {
                        warn!(
                            "MCP prompt '{}' hit transient error ({}), retrying once...",
                            self.name, e.message
                        );
                        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                        continue;
                    }
                    error!("MCP prompt '{}' failed: {}", self.name, e.message);
                    if e.is_transient {
                        return Ok(Value::String(format!(
                            "(MCP prompt call failed after retry: {})",
                            e.message
                        )));
                    }
                    return Ok(Value::String(format!(
                        "(MCP prompt call failed: {})",
                        e.message
                    )));
                }
            }
        }

        Ok(Value::String("(MCP prompt call failed)".into()))
    }
}

pub struct McpServerStack {
    pub name: String,
    pub session: Arc<dyn McpSession>,
}

pub async fn connect_mcp_servers(
    mcp_servers: &HashMap<String, McpServerConfig>,
    registry: &ToolRegistry,
) -> HashMap<String, Arc<dyn McpSession>> {
    let mut server_sessions: HashMap<String, Arc<dyn McpSession>> = HashMap::new();

    for (name, cfg) in mcp_servers {
        match connect_single_server(name, cfg, registry).await {
            Ok(session) => {
                server_sessions.insert(name.clone(), session);
            }
            Err(e) => {
                error!("MCP server '{}': failed to connect: {}", name, e);
            }
        }
    }

    server_sessions
}

async fn connect_single_server(
    name: &str,
    cfg: &McpServerConfig,
    registry: &ToolRegistry,
) -> Result<Arc<dyn McpSession>, String> {
    todo!("TODO: implement MCP server connection — connect via stdio, SSE, or streamable HTTP using the MCP SDK, then create a ClientSession and register tools/resources/prompts")
}
