use async_trait::async_trait;
use log::{debug, error, info, warn};
use regex::Regex;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{RwLock, mpsc};

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

    async fn list_tools(&self) -> Result<Vec<McpToolDefinition>, McpError>;

    async fn list_resources(&self) -> Result<Vec<McpResourceDefinition>, McpError>;

    async fn list_prompts(&self) -> Result<Vec<McpPromptDefinition>, McpError>;
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

pub async fn probe_http_url(url: &str, timeout_secs: f64) -> bool {
    let parsed = match reqwest::Url::parse(url) {
        Ok(u) => u,
        Err(_) => return false,
    };

    let host = parsed.host_str().unwrap_or("127.0.0.1");
    let port = parsed
        .port()
        .unwrap_or_else(|| if parsed.scheme() == "https" { 443 } else { 80 });

    let timeout = std::time::Duration::from_secs_f64(timeout_secs);

    match tokio::time::timeout(timeout, tokio::net::TcpStream::connect((host, port))).await {
        Ok(Ok(_stream)) => true,
        Ok(Err(_)) => false,
        Err(_) => false,
    }
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
        let non_null: Vec<&Value> = raw_type
            .iter()
            .filter(|i| i.as_str() != Some("null"))
            .collect();
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
        let description = tool_def
            .description
            .unwrap_or_else(|| tool_def.name.clone());
        let raw_schema = tool_def
            .input_schema
            .unwrap_or_else(|| json!({"type": "object", "properties": {}}));
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
        let name = sanitize_name(&format!(
            "mcp_{}_resource_{}",
            server_name, resource_def.name
        ));
        let desc = resource_def
            .description
            .unwrap_or_else(|| resource_def.name.clone());
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
                            McpResourceContent::Blob(size) => {
                                format!("[Binary resource: {} bytes]", size)
                            }
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
        let desc = prompt_def
            .description
            .unwrap_or_else(|| prompt_def.name.clone());
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

/// Handle that owns the transport channels for a connected MCP server.
/// Dropping the handle (or calling [`shutdown`]) closes the transport
/// channels so the spawned I/O tasks can exit cleanly.
pub struct McpServerHandle {
    pub name: String,
    pub session: Arc<dyn McpSession>,
    /// Dropping this sender closes the transport command channel,
    /// signalling the background I/O task to exit.
    _cmd_tx: Option<mpsc::Sender<String>>,
}

impl McpServerHandle {
    pub fn new(name: String, session: Arc<dyn McpSession>, cmd_tx: mpsc::Sender<String>) -> Self {
        Self {
            name,
            session,
            _cmd_tx: Some(cmd_tx),
        }
    }

    /// Signal the transport to shut down and consume the handle.
    pub fn shutdown(mut self) {
        self._cmd_tx.take();
    }
}

pub async fn connect_mcp_servers(
    mcp_servers: &HashMap<String, McpServerConfig>,
    registry: &ToolRegistry,
) -> HashMap<String, McpServerHandle> {
    let mut handles: HashMap<String, McpServerHandle> = HashMap::new();

    for (name, cfg) in mcp_servers {
        match connect_single_server(name, cfg, registry).await {
            Ok((session, cmd_tx)) => {
                handles.insert(
                    name.clone(),
                    McpServerHandle::new(name.clone(), session, cmd_tx),
                );
            }
            Err(e) => {
                error!("MCP server '{}': failed to connect: {}", name, e);
            }
        }
    }

    handles
}

// ---------- McpSessionImpl: concrete JSON-RPC MCP session ----------

static MSG_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

struct PendingRequest {
    tx: tokio::sync::oneshot::Sender<Result<Value, String>>,
}

struct McpSessionImpl {
    pending: Arc<RwLock<HashMap<u64, PendingRequest>>>,
    shutdown_tx: mpsc::Sender<()>,
}

impl McpSessionImpl {
    fn new(shutdown_tx: mpsc::Sender<()>) -> Self {
        Self {
            pending: Arc::new(RwLock::new(HashMap::new())),
            shutdown_tx,
        }
    }

    fn next_id() -> u64 {
        MSG_ID_COUNTER.fetch_add(1, Ordering::Relaxed)
    }

    async fn send_request(
        &self,
        method: &str,
        params: Value,
        timeout_secs: u64,
    ) -> Result<Value, McpError> {
        let id = Self::next_id();
        let message = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });

        let (tx, rx) = tokio::sync::oneshot::channel();
        {
            let mut pending = self.pending.write().await;
            pending.insert(id, PendingRequest { tx });
        }

        let _serialized = serde_json::to_string(&message).map_err(|e| McpError {
            code: None,
            message: format!("Failed to serialize request: {}", e),
            is_transient: false,
        })?;

        if self.shutdown_tx.is_closed() {
            let mut pending = self.pending.write().await;
            pending.remove(&id);
            return Err(McpError {
                code: None,
                message: "Session is closed".into(),
                is_transient: true,
            });
        }

        let timeout = std::time::Duration::from_secs(timeout_secs);
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(result)) => result.map_err(|e| {
                let is_transient = is_transient(&e);
                McpError {
                    code: None,
                    message: e,
                    is_transient,
                }
            }),
            Ok(Err(_)) => Err(McpError {
                code: None,
                message: "Channel closed".into(),
                is_transient: true,
            }),
            Err(_) => {
                let mut pending = self.pending.write().await;
                pending.remove(&id);
                Err(McpError {
                    code: None,
                    message: format!("Request timed out after {}s", timeout_secs),
                    is_transient: false,
                })
            }
        }
    }

    fn spawn_stdio_handler(
        pending: Arc<RwLock<HashMap<u64, PendingRequest>>>,
        mut child: tokio::process::Child,
    ) -> (mpsc::Sender<String>, mpsc::Receiver<()>) {
        let (tx_cmd, mut rx_cmd) = mpsc::channel::<String>(32);
        let (done_tx, done_rx) = mpsc::channel::<()>(1);

        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let mut stdin_writer = tokio::io::BufWriter::new(stdin);
        let mut reader = BufReader::new(stdout);

        tokio::spawn(async move {
            let mut read_task = tokio::spawn(async move {
                let mut line = String::new();
                loop {
                    line.clear();
                    match reader.read_line(&mut line).await {
                        Ok(0) => break,
                        Ok(_) => {
                            let trimmed = line.trim();
                            if trimmed.is_empty() {
                                continue;
                            }
                            if let Ok(response) = serde_json::from_str::<Value>(trimmed) {
                                if let Some(id) = response.get("id").and_then(|v| v.as_u64()) {
                                    let mut pending_map = pending.write().await;
                                    if let Some(pending_req) = pending_map.remove(&id) {
                                        if let Some(error) = response.get("error") {
                                            let _code = error
                                                .get("code")
                                                .and_then(|c| c.as_i64())
                                                .map(|c| c as i32);
                                            let message = error
                                                .get("message")
                                                .and_then(|m| m.as_str())
                                                .unwrap_or("Unknown error")
                                                .to_string();
                                            let _ = pending_req.tx.send(Err(message));
                                        } else if let Some(result) = response.get("result") {
                                            let _ = pending_req.tx.send(Ok(result.clone()));
                                        }
                                    }
                                }
                            }
                        }
                        Err(_) => break,
                    }
                }
            });

            loop {
                tokio::select! {
                    msg = rx_cmd.recv() => {
                        match msg {
                            Some(line) => {
                                let _ = stdin_writer.write_all(line.as_bytes()).await;
                                let _ = stdin_writer.write_all(b"\n").await;
                                let _ = stdin_writer.flush().await;
                            }
                            None => break,
                        }
                    }
                    result = &mut read_task => {
                        match result {
                            Ok(_) => {},
                            Err(e) => {
                                warn!("Stdio reader task failed: {}", e);
                            }
                        }
                        break;
                    }
                }
            }

            let _ = done_tx.send(()).await;
        });

        (tx_cmd, done_rx)
    }
}

#[async_trait]
impl McpSession for McpSessionImpl {
    async fn call_tool(
        &self,
        name: String,
        arguments: HashMap<String, Value>,
        timeout_secs: u64,
    ) -> Result<McpToolResult, McpError> {
        let args_map: serde_json::Map<String, Value> = arguments.into_iter().collect();
        let params = json!({
            "name": name,
            "arguments": args_map,
        });
        let result = self
            .send_request("tools/call", params, timeout_secs)
            .await?;

        let content = result
            .get("content")
            .and_then(|c| c.as_array())
            .map(|arr| {
                arr.iter()
                    .map(|block| {
                        if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                            McpContentBlock::Text(text.to_string())
                        } else {
                            McpContentBlock::Other(block.to_string())
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(McpToolResult { content })
    }

    async fn read_resource(
        &self,
        uri: String,
        timeout_secs: u64,
    ) -> Result<McpResourceResult, McpError> {
        let params = json!({
            "uri": uri,
        });
        let result = self
            .send_request("resources/read", params, timeout_secs)
            .await?;

        let contents = result
            .get("contents")
            .and_then(|c| c.as_array())
            .map(|arr| {
                arr.iter()
                    .map(|block| {
                        if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                            McpResourceContent::Text(text.to_string())
                        } else if let Some(blob) = block.get("blob").and_then(|b| b.as_str()) {
                            McpResourceContent::Blob(blob.len())
                        } else {
                            McpResourceContent::Other(block.to_string())
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(McpResourceResult { contents })
    }

    async fn get_prompt(
        &self,
        name: String,
        arguments: HashMap<String, Value>,
        timeout_secs: u64,
    ) -> Result<McpPromptResult, McpError> {
        let args_map: serde_json::Map<String, Value> = arguments.into_iter().collect();
        let params = json!({
            "name": name,
            "arguments": args_map,
        });
        let result = self
            .send_request("prompts/get", params, timeout_secs)
            .await?;

        let messages = result
            .get("messages")
            .and_then(|m| m.as_array())
            .map(|arr| {
                arr.iter()
                    .map(|msg| {
                        if let Some(text) = msg
                            .get("content")
                            .and_then(|c| c.get("text"))
                            .and_then(|t| t.as_str())
                        {
                            McpPromptMessage::Text(text.to_string())
                        } else {
                            McpPromptMessage::Other(msg.to_string())
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(McpPromptResult { messages })
    }

    async fn list_tools(&self) -> Result<Vec<McpToolDefinition>, McpError> {
        let result = self.send_request("tools/list", json!({}), 30).await?;

        let tools = result
            .get("tools")
            .and_then(|t| t.as_array())
            .map(|arr| {
                arr.iter()
                    .map(|tool| McpToolDefinition {
                        name: tool
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        description: tool
                            .get("description")
                            .and_then(|v| v.as_str())
                            .map(String::from),
                        input_schema: tool.get("inputSchema").cloned(),
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(tools)
    }

    async fn list_resources(&self) -> Result<Vec<McpResourceDefinition>, McpError> {
        let result = self.send_request("resources/list", json!({}), 30).await?;

        let resources = result
            .get("resources")
            .and_then(|r| r.as_array())
            .map(|arr| {
                arr.iter()
                    .map(|res| McpResourceDefinition {
                        uri: res
                            .get("uri")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        name: res
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        description: res
                            .get("description")
                            .and_then(|v| v.as_str())
                            .map(String::from),
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(resources)
    }

    async fn list_prompts(&self) -> Result<Vec<McpPromptDefinition>, McpError> {
        let result = self.send_request("prompts/list", json!({}), 30).await?;

        let prompts = result
            .get("prompts")
            .and_then(|p| p.as_array())
            .map(|arr| {
                arr.iter()
                    .map(|prompt| {
                        let args = prompt
                            .get("arguments")
                            .and_then(|a| a.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .map(|arg| McpPromptArgument {
                                        name: arg
                                            .get("name")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("")
                                            .to_string(),
                                        description: arg
                                            .get("description")
                                            .and_then(|v| v.as_str())
                                            .map(String::from),
                                        required: arg
                                            .get("required")
                                            .and_then(|v| v.as_bool())
                                            .unwrap_or(false),
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();

                        McpPromptDefinition {
                            name: prompt
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
                            description: prompt
                                .get("description")
                                .and_then(|v| v.as_str())
                                .map(String::from),
                            arguments: args,
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(prompts)
    }
}

// ---------- HTTP-based transports (SSE & Streamable HTTP) ----------

async fn connect_sse_transport(
    url: &str,
    headers: Option<&HashMap<String, String>>,
) -> Result<(mpsc::Sender<String>, mpsc::Receiver<()>), String> {
    let client = build_http_client(headers);
    let headers_owned = headers.map(|h| h.clone());

    let pending: Arc<RwLock<HashMap<u64, PendingRequest>>> = Arc::new(RwLock::new(HashMap::new()));
    let (done_tx, done_rx) = mpsc::channel::<()>(1);

    let sse_url = if url.ends_with("/sse") {
        url.to_string()
    } else {
        format!("{}/sse", url.trim_end_matches('/'))
    };

    let pending_clone = pending.clone();
    let done_tx_sse = done_tx.clone();
    tokio::spawn(async move {
        let request = client.get(&sse_url);
        let response = match request.send().await {
            Ok(r) => r,
            Err(e) => {
                warn!("SSE connection failed: {}", e);
                let _ = done_tx_sse.send(()).await;
                return;
            }
        };

        let mut event_stream = response.bytes_stream();
        let mut event_data = String::new();
        let mut event_type = String::new();
        let mut message_id = String::new();
        let mut retry = std::time::Duration::from_secs(3);

        use futures::StreamExt;
        while let Some(chunk_result) = event_stream.next().await {
            match chunk_result {
                Ok(chunk) => {
                    let text = String::from_utf8_lossy(&chunk);
                    for line in text.lines() {
                        if line.is_empty() {
                            if !event_type.is_empty() || !event_data.is_empty() {
                                process_sse_event(
                                    &event_type,
                                    &event_data,
                                    &message_id,
                                    &pending_clone,
                                )
                                .await;
                                event_type.clear();
                                event_data.clear();
                                message_id.clear();
                            }
                            continue;
                        }

                        if let Some(colon_pos) = line.find(':') {
                            let field = &line[..colon_pos];
                            let value = line[colon_pos + 1..].trim();
                            match field {
                                "event" => event_type = value.to_string(),
                                "data" => event_data = value.to_string(),
                                "id" => message_id = value.to_string(),
                                "retry" => {
                                    if let Ok(ms) = value.parse::<u64>() {
                                        retry = std::time::Duration::from_millis(ms);
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
                Err(e) => {
                    warn!("SSE stream error: {}", e);
                    tokio::time::sleep(retry).await;
                    break;
                }
            }
        }

        let _ = done_tx_sse.send(()).await;
    });

    let (cmd_tx, mut cmd_rx) = mpsc::channel::<String>(32);
    let pending_clone2 = pending.clone();
    let client_clone = build_http_client(headers_owned.as_ref());
    let base_url = url.to_string();
    tokio::spawn(async move {
        while let Some(request_str) = cmd_rx.recv().await {
            if let Ok(_req) = serde_json::from_str::<Value>(&request_str) {
                let post_url = base_url.clone();
                let response = client_clone
                    .post(&post_url)
                    .header("Content-Type", "application/json")
                    .body(request_str)
                    .send()
                    .await;

                match response {
                    Ok(resp) => {
                        if let Ok(body) = resp.text().await {
                            if let Ok(response_val) = serde_json::from_str::<Value>(&body) {
                                if let Some(id) = response_val.get("id").and_then(|v| v.as_u64()) {
                                    let mut pending_map = pending_clone2.write().await;
                                    if let Some(pending_req) = pending_map.remove(&id) {
                                        if let Some(error) = response_val.get("error") {
                                            let message = error
                                                .get("message")
                                                .and_then(|m| m.as_str())
                                                .unwrap_or("Unknown error")
                                                .to_string();
                                            let _ = pending_req.tx.send(Err(message));
                                        } else if let Some(result) = response_val.get("result") {
                                            let _ = pending_req.tx.send(Ok(result.clone()));
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        warn!("HTTP POST failed: {}", e);
                    }
                }
            }
        }
        let _ = done_tx.send(()).await;
    });

    Ok((cmd_tx, done_rx))
}

async fn connect_streamable_http_transport(
    url: &str,
    headers: Option<&HashMap<String, String>>,
) -> Result<(mpsc::Sender<String>, mpsc::Receiver<()>), String> {
    let client = build_http_client(headers);
    let headers_owned = headers.map(|h| h.clone());
    let pending: Arc<RwLock<HashMap<u64, PendingRequest>>> = Arc::new(RwLock::new(HashMap::new()));
    let (done_tx, done_rx) = mpsc::channel::<()>(1);
    let (cmd_tx, mut cmd_rx) = mpsc::channel::<String>(32);

    let pending_clone = pending.clone();
    let client_clone = client.clone();
    let base_url = url.to_string();
    tokio::spawn(async move {
        let mut session_id: Option<String> = None;

        while let Some(request_str) = cmd_rx.recv().await {
            let mut req_builder = client_clone
                .post(&base_url)
                .header("Content-Type", "application/json")
                .header("Accept", "application/json, text/event-stream");

            if let Some(ref sid) = session_id {
                req_builder = req_builder.header("Mcp-Session-Id", sid);
            }

            if let Some(h) = &headers_owned {
                for (k, v) in h {
                    req_builder = req_builder.header(k, v);
                }
            }

            let response = match req_builder.body(request_str.clone()).send().await {
                Ok(r) => r,
                Err(e) => {
                    warn!("Streamable HTTP request failed: {}", e);
                    continue;
                }
            };

            if let Some(sid) = response.headers().get("Mcp-Session-Id") {
                if let Ok(sid_str) = sid.to_str() {
                    session_id = Some(sid_str.to_string());
                }
            }

            let content_type = response
                .headers()
                .get("Content-Type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_string();

            if content_type.contains("text/event-stream") {
                let mut event_stream = response.bytes_stream();
                let mut event_data = String::new();
                let mut event_type = String::new();
                let mut message_id = String::new();

                use futures::StreamExt;
                while let Some(chunk_result) = event_stream.next().await {
                    match chunk_result {
                        Ok(chunk) => {
                            let text = String::from_utf8_lossy(&chunk);
                            for line in text.lines() {
                                if line.is_empty() {
                                    if !event_type.is_empty() || !event_data.is_empty() {
                                        process_sse_event(
                                            &event_type,
                                            &event_data,
                                            &message_id,
                                            &pending_clone,
                                        )
                                        .await;
                                        event_type.clear();
                                        event_data.clear();
                                        message_id.clear();
                                    }
                                    continue;
                                }

                                if let Some(colon_pos) = line.find(':') {
                                    let field = &line[..colon_pos];
                                    let value = line[colon_pos + 1..].trim();
                                    match field {
                                        "event" => event_type = value.to_string(),
                                        "data" => event_data = value.to_string(),
                                        "id" => message_id = value.to_string(),
                                        _ => {}
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            warn!("SSE stream error: {}", e);
                            break;
                        }
                    }
                }
            } else {
                let body = match response.text().await {
                    Ok(b) => b,
                    Err(e) => {
                        warn!("Failed to read response body: {}", e);
                        continue;
                    }
                };

                if let Ok(response_val) = serde_json::from_str::<Value>(&body) {
                    if let Some(id) = response_val.get("id").and_then(|v| v.as_u64()) {
                        let mut pending_map = pending_clone.write().await;
                        if let Some(pending_req) = pending_map.remove(&id) {
                            if let Some(error) = response_val.get("error") {
                                let message = error
                                    .get("message")
                                    .and_then(|m| m.as_str())
                                    .unwrap_or("Unknown error")
                                    .to_string();
                                let _ = pending_req.tx.send(Err(message));
                            } else if let Some(result) = response_val.get("result") {
                                let _ = pending_req.tx.send(Ok(result.clone()));
                            }
                        }
                    }
                }
            }
        }

        if let Some(sid) = &session_id {
            let _ = client_clone
                .delete(&base_url)
                .header("Mcp-Session-Id", sid)
                .send()
                .await;
        }

        let _ = done_tx.send(()).await;
    });

    Ok((cmd_tx, done_rx))
}

fn build_http_client(headers: Option<&HashMap<String, String>>) -> reqwest::Client {
    let mut builder = reqwest::Client::builder().redirect(reqwest::redirect::Policy::limited(5));

    if let Some(h) = headers {
        let mut default_headers = reqwest::header::HeaderMap::new();
        for (k, v) in h {
            if let Ok(key) = reqwest::header::HeaderName::from_bytes(k.as_bytes()) {
                if let Ok(val) = reqwest::header::HeaderValue::from_str(v) {
                    default_headers.insert(key, val);
                }
            }
        }
        builder = builder.default_headers(default_headers);
    }

    builder.build().expect("Failed to build HTTP client")
}

async fn process_sse_event(
    _event_type: &str,
    event_data: &str,
    _message_id: &str,
    pending: &RwLock<HashMap<u64, PendingRequest>>,
) {
    if event_data.is_empty() {
        return;
    }

    if let Ok(response) = serde_json::from_str::<Value>(event_data) {
        if let Some(id) = response.get("id").and_then(|v| v.as_u64()) {
            let mut pending_map = pending.write().await;
            if let Some(pending_req) = pending_map.remove(&id) {
                if let Some(error) = response.get("error") {
                    let message = error
                        .get("message")
                        .and_then(|m| m.as_str())
                        .unwrap_or("Unknown error")
                        .to_string();
                    let _ = pending_req.tx.send(Err(message));
                } else if let Some(result) = response.get("result") {
                    let _ = pending_req.tx.send(Ok(result.clone()));
                }
            }
        }
    }
}

async fn connect_single_server(
    name: &str,
    cfg: &McpServerConfig,
    registry: &ToolRegistry,
) -> Result<(Arc<dyn McpSession>, mpsc::Sender<String>), String> {
    let transport_type = cfg.transport_type.clone().unwrap_or_else(|| {
        if cfg.command.is_some() {
            "stdio".into()
        } else if let Some(ref url) = cfg.url {
            if url.trim_end_matches('/').ends_with("/sse") {
                "sse".into()
            } else {
                "streamableHttp".into()
            }
        } else {
            "stdio".into()
        }
    });

    if transport_type != "stdio" && transport_type != "sse" && transport_type != "streamableHttp" {
        if cfg.command.is_none() && cfg.url.is_none() {
            warn!(
                "MCP server '{}': no command or url configured, skipping",
                name
            );
            return Err("No command or url configured".to_string());
        }
    }

    let (cmd_tx, _done_rx, session) = match transport_type.as_str() {
        "stdio" => {
            let command = cfg
                .command
                .as_ref()
                .ok_or("stdio transport requires command")?;
            let args = cfg.args.as_deref().unwrap_or(&[]);
            let env = cfg.env.as_ref();
            let (cmd, normalized_args, normalized_env) =
                normalize_windows_stdio_command(command, Some(args), env);

            let mut child_cmd = tokio::process::Command::new(&cmd);
            child_cmd.args(&normalized_args);
            child_cmd.stdin(std::process::Stdio::piped());
            child_cmd.stdout(std::process::Stdio::piped());
            child_cmd.stderr(std::process::Stdio::inherit());

            if let Some(ref e) = normalized_env {
                for (k, v) in e {
                    child_cmd.env(k, v);
                }
            }

            let child = child_cmd
                .spawn()
                .map_err(|e| format!("Failed to spawn stdio process '{}': {}", cmd, e))?;

            let session = McpSessionImpl::new(mpsc::channel(1).0);
            let (tx, _done) = McpSessionImpl::spawn_stdio_handler(session.pending.clone(), child);

            let msg = json!({
                "jsonrpc": "2.0",
                "id": McpSessionImpl::next_id(),
                "method": "initialize",
                "params": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": {
                        "name": "hive-claw",
                        "version": "0.1.0"
                    }
                }
            });

            let init_str = serde_json::to_string(&msg).unwrap();
            tx.send(init_str)
                .await
                .map_err(|e| format!("Failed to send initialize: {}", e))?;
            let (init_tx, init_rx) = tokio::sync::oneshot::channel();
            {
                let mut pending = session.pending.write().await;
                let id = MSG_ID_COUNTER.load(Ordering::Relaxed) - 1;
                pending.insert(id, PendingRequest { tx: init_tx });
            }

            match tokio::time::timeout(std::time::Duration::from_secs(30), init_rx).await {
                Ok(Ok(Ok(_))) => {}
                Ok(Ok(Err(e))) => return Err(format!("Initialize failed: {}", e)),
                Ok(Err(_)) => return Err("Initialize channel closed".into()),
                Err(_) => return Err("Initialize timed out".into()),
            }

            let notify_msg = json!({
                "jsonrpc": "2.0",
                "method": "notifications/initialized"
            });
            let notify_str = serde_json::to_string(&notify_msg).unwrap();
            tx.send(notify_str)
                .await
                .map_err(|e| format!("Failed to send initialized notification: {}", e))?;

            (tx, _done, Arc::new(session))
        }

        "sse" => {
            let url = cfg.url.as_ref().ok_or("sse transport requires url")?;

            if !probe_http_url(url, 3.0).await {
                warn!("MCP server '{}': {} unreachable, skipping", name, url);
                return Err(format!("URL {} unreachable", url));
            }

            let (cmd_tx, _done_rx) = connect_sse_transport(url, cfg.headers.as_ref()).await?;

            let pending: Arc<RwLock<HashMap<u64, PendingRequest>>> =
                Arc::new(RwLock::new(HashMap::new()));
            let shutdown_tx = mpsc::channel(1).0;
            let session = Arc::new(McpSessionImpl {
                pending: pending.clone(),
                shutdown_tx,
            });

            let result = session
                .send_request(
                    "initialize",
                    json!({
                        "protocolVersion": "2024-11-05",
                        "capabilities": {},
                        "clientInfo": {
                            "name": "hive-claw",
                            "version": "0.1.0"
                        }
                    }),
                    30,
                )
                .await;

            match result {
                Ok(_) => {}
                Err(e) => return Err(format!("Initialize failed: {}", e.message)),
            }

            let _ = session
                .send_request("notifications/initialized", json!({}), 5)
                .await;

            (cmd_tx, _done_rx, session)
        }

        "streamableHttp" => {
            let url = cfg
                .url
                .as_ref()
                .ok_or("streamableHttp transport requires url")?;

            if !probe_http_url(url, 3.0).await {
                warn!("MCP server '{}': {} unreachable, skipping", name, url);
                return Err(format!("URL {} unreachable", url));
            }

            let (cmd_tx, _done_rx) =
                connect_streamable_http_transport(url, cfg.headers.as_ref()).await?;

            let pending: Arc<RwLock<HashMap<u64, PendingRequest>>> =
                Arc::new(RwLock::new(HashMap::new()));
            let shutdown_tx = mpsc::channel(1).0;
            let session = Arc::new(McpSessionImpl {
                pending: pending.clone(),
                shutdown_tx,
            });

            let result = session
                .send_request(
                    "initialize",
                    json!({
                        "protocolVersion": "2024-11-05",
                        "capabilities": {},
                        "clientInfo": {
                            "name": "hive-claw",
                            "version": "0.1.0"
                        }
                    }),
                    30,
                )
                .await;

            match result {
                Ok(_) => {}
                Err(e) => return Err(format!("Initialize failed: {}", e.message)),
            }

            let _ = session
                .send_request("notifications/initialized", json!({}), 5)
                .await;

            (cmd_tx, _done_rx, session)
        }

        other => {
            warn!("MCP server '{}': unknown transport type '{}'", name, other);
            return Err(format!("Unknown transport type: {}", other));
        }
    };

    let tools = session
        .list_tools()
        .await
        .map_err(|e| format!("Failed to list tools: {}", e.message))?;

    let enabled_tools: std::collections::HashSet<&String> = cfg.enabled_tools.iter().collect();
    let allow_all_tools = enabled_tools.contains(&"*".to_string());
    let mut registered_count = 0;

    let available_raw_names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    let available_wrapped_names: Vec<String> = tools
        .iter()
        .map(|t| sanitize_name(&format!("mcp_{}_{}", name, t.name)))
        .collect();

    for tool_def in &tools {
        let wrapped_name = sanitize_name(&format!("mcp_{}_{}", name, tool_def.name));
        if !allow_all_tools
            && !enabled_tools.contains(&tool_def.name)
            && !enabled_tools.contains(&wrapped_name)
        {
            debug!(
                "MCP: skipping tool '{}' from server '{}' (not in enabledTools)",
                wrapped_name, name
            );
            continue;
        }

        let wrapper = MCPToolWrapper::new(
            session.clone(),
            name,
            McpToolDefinition {
                name: tool_def.name.clone(),
                description: tool_def.description.clone(),
                input_schema: tool_def.input_schema.clone(),
            },
            cfg.tool_timeout,
        );
        let wrapper_name = wrapper.name().to_string();
        registry.register(Arc::new(wrapper)).await;
        debug!(
            "MCP: registered tool '{}' from server '{}'",
            wrapper_name, name
        );
        registered_count += 1;
    }

    if !allow_all_tools && !cfg.enabled_tools.is_empty() {
        let matched: std::collections::HashSet<String> = tools
            .iter()
            .flat_map(|t| {
                let wrapped = sanitize_name(&format!("mcp_{}_{}", name, t.name));
                vec![t.name.clone(), wrapped]
            })
            .filter(|n| enabled_tools.contains(n))
            .collect();

        let unmatched: Vec<&String> = cfg
            .enabled_tools
            .iter()
            .filter(|t| !matched.contains(*t))
            .collect();
        if !unmatched.is_empty() {
            warn!(
                "MCP server '{}': enabledTools entries not found: {}. Available raw names: {}. Available wrapped names: {}",
                name,
                unmatched
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                available_raw_names
                    .join(", ")
                    .is_empty()
                    .then(|| "(none)")
                    .unwrap_or(&available_raw_names.join(", ")),
                available_wrapped_names
                    .join(", ")
                    .is_empty()
                    .then(|| "(none)")
                    .unwrap_or(&available_wrapped_names.join(", ")),
            );
        }
    }

    match session.list_resources().await {
        Ok(resources) => {
            for res_def in resources {
                let wrapper =
                    MCPResourceWrapper::new(session.clone(), name, res_def, cfg.tool_timeout);
                let wrapper_name = wrapper.name().to_string();
                registry.register(Arc::new(wrapper)).await;
                registered_count += 1;
                debug!(
                    "MCP: registered resource '{}' from server '{}'",
                    wrapper_name, name
                );
            }
        }
        Err(e) => {
            debug!(
                "MCP server '{}': resources not supported or failed: {}",
                name, e.message
            );
        }
    }

    match session.list_prompts().await {
        Ok(prompts) => {
            for prompt_def in prompts {
                let wrapper =
                    MCPPromptWrapper::new(session.clone(), name, prompt_def, cfg.tool_timeout);
                let wrapper_name = wrapper.name().to_string();
                registry.register(Arc::new(wrapper)).await;
                registered_count += 1;
                debug!(
                    "MCP: registered prompt '{}' from server '{}'",
                    wrapper_name, name
                );
            }
        }
        Err(e) => {
            debug!(
                "MCP server '{}': prompts not supported or failed: {}",
                name, e.message
            );
        }
    }

    info!(
        "MCP server '{}': connected, {} capabilities registered",
        name, registered_count
    );

    Ok((session, cmd_tx))
}
