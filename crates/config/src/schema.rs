//! Configuration schema (translated from nanobot/config/schema.py).
//!
//! All fields serialize/deserialize in `camelCase` to match the JSON config
//! files produced by the Python code base (which used
//! `alias_generator=to_camel` + `by_alias=True`). Where the Python version
//! also accepted alternate keys via `AliasChoices`, a `#[serde(alias = ...)]`
//! is added so existing configs keep loading.

use std::fs;
use std::path::PathBuf;
use std::{collections::HashMap, path::Path};

use crate::loader::{resolve_value, try_load};
use crate::{ConfigError, get_config_path};
use serde::{Deserialize, Serialize};

/// Expands a path that starts with `~` (tilde) to the user's home directory.
/// If the path doesn't start with `~`, it is returned as-is.
///
/// 将以 `~`（波浪号）开头的路径展开为用户的主目录。
/// 如果路径不以 `~` 开头，则原样返回。
///
/// # Arguments / 参数
/// * `path`
///     - A path string that may start with `~` or `~/`
///     - 可能以 `~` 或 `~/` 开头的路径字符串
///
/// # Returns / 返回值
/// A `PathBuf` with the tilde expanded to the home directory
/// 展开波浪号后的 PathBuf
fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    } else if path == "~" {
        if let Some(home) = dirs::home_dir() {
            return home;
        }
    }
    PathBuf::from(path)
}

// ---------------------------------------------------------------------------
// Channels
// ---------------------------------------------------------------------------

/// Configuration for chat channels.
///
/// Built-in and plugin channel configs are captured as free-form extras so
/// each channel can parse its own sub-config.
///
/// 聊天频道的配置。
///
/// 内置和插件频道配置以自由形式的额外字段捕获，以便每个频道可以解析自己的子配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelsConfig {
    /// Stream agent text progress to the channel.
    /// / 将 Agent 文本进度流式传输到频道。
    #[serde(default = "default_true")]
    pub send_progress: bool,

    /// Stream tool-call hints (e.g. `read_file("…")`).
    /// / 流式传输工具调用提示（例如 `read_file("…")`）。
    #[serde(default)]
    pub send_tool_hints: bool,

    /// Max delivery attempts (initial send included). 0..=10.
    /// / 最大投递尝试次数（包含初始发送）。0..=10。
    #[serde(default = "default_send_max_retries")]
    pub send_max_retries: u32,

    /// Voice transcription backend: "groq" or "openai".
    /// / 语音转录后端："groq" 或 "openai"。
    #[serde(default = "default_transcription_provider")]
    pub transcription_provider: String,

    /// Optional ISO-639-1/2/3 hint for audio transcription.
    /// / 音频转录的可选 ISO-639-1/2/3 提示。
    #[serde(default)]
    pub transcription_language: Option<String>,

    /// Arbitrary per-channel configuration (extras).
    /// / 任意每频道配置（额外字段）。
    #[serde(flatten)]
    pub extras: HashMap<String, serde_json::Value>,
}

fn default_true() -> bool {
    true
}
fn default_send_max_retries() -> u32 {
    3
}
fn default_transcription_provider() -> String {
    "groq".to_string()
}

impl Default for ChannelsConfig {
    fn default() -> Self {
        Self {
            send_progress: true,
            send_tool_hints: false,
            send_max_retries: 3,
            transcription_provider: "groq".to_string(),
            transcription_language: None,
            extras: HashMap::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Dream
// ---------------------------------------------------------------------------

const HOUR_MS: u64 = 3_600_000;

/// Dream memory-consolidation configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DreamConfig {
    /// Run every N hours (default 2).
    #[serde(default = "default_interval_h")]
    pub interval_h: u32,

    /// Legacy compatibility override; never serialized.
    #[serde(default, skip_serializing)]
    pub cron: Option<String>,

    /// Optional Dream-specific model override.
    #[serde(default, alias = "model", alias = "model_override")]
    pub model_override: Option<String>,

    /// Max history entries per run (>=1).
    #[serde(default = "default_max_batch_size")]
    pub max_batch_size: u32,

    /// Max tool calls per Phase 2 (>=1).
    #[serde(default = "default_max_iterations")]
    pub max_iterations: u32,

    /// Per-line git-blame age annotation in Phase 1 prompt.
    #[serde(default = "default_true")]
    pub annotate_line_ages: bool,
}

fn default_interval_h() -> u32 {
    2
}
fn default_max_batch_size() -> u32 {
    20
}
fn default_max_iterations() -> u32 {
    15
}

impl Default for DreamConfig {
    fn default() -> Self {
        Self {
            interval_h: 2,
            cron: None,
            model_override: None,
            max_batch_size: 20,
            max_iterations: 15,
            annotate_line_ages: true,
        }
    }
}

impl DreamConfig {
    /// Human-readable schedule summary (matches `describe_schedule`).
    pub fn describe_schedule(&self) -> String {
        if let Some(expr) = &self.cron {
            format!("cron {} (legacy)", expr)
        } else {
            format!("every {}h", self.interval_h)
        }
    }

    /// Interval in milliseconds when using the default hourly schedule.
    pub fn interval_ms(&self) -> u64 {
        self.interval_h as u64 * HOUR_MS
    }
}

// ---------------------------------------------------------------------------
// Agents
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ProviderRetryMode {
    Standard,
    Persistent,
}

impl Default for ProviderRetryMode {
    fn default() -> Self {
        ProviderRetryMode::Standard
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentDefaults {
    #[serde(default = "default_workspace")]
    pub workspace: String,

    #[serde(default = "default_model")]
    pub model: String,

    /// Provider name (e.g. "anthropic", "openrouter") or "auto".
    #[serde(default = "default_provider")]
    pub provider: String,

    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,

    #[serde(default = "default_context_window_tokens")]
    pub context_window_tokens: u32,

    #[serde(default)]
    pub context_block_limit: Option<u32>,

    #[serde(default = "default_temperature")]
    pub temperature: f32,

    #[serde(default = "default_max_tool_iterations")]
    pub max_tool_iterations: u32,

    #[serde(default = "default_max_tool_result_chars")]
    pub max_tool_result_chars: u32,

    #[serde(default)]
    pub provider_retry_mode: ProviderRetryMode,

    /// low / medium / high / adaptive — enables LLM thinking mode.
    #[serde(default)]
    pub reasoning_effort: Option<String>,

    /// IANA timezone, e.g. "Asia/Shanghai".
    #[serde(default = "default_timezone")]
    pub timezone: String,

    /// Share one session across all channels (single-user multi-device).
    #[serde(default)]
    pub unified_session: bool,

    /// Skill names to exclude from loading.
    #[serde(default)]
    pub disabled_skills: Vec<String>,

    /// Auto-compact idle threshold in minutes (0 = disabled).
    /// Serializes as `idleCompactAfterMinutes` and also accepts
    /// `sessionTtlMinutes` / `session_ttl_minutes`.
    #[serde(
        default,
        rename = "idleCompactAfterMinutes",
        alias = "sessionTtlMinutes",
        alias = "session_ttl_minutes"
    )]
    pub session_ttl_minutes: u32,

    #[serde(default)]
    pub dream: DreamConfig,
}

fn default_workspace() -> String {
    "~/.nanobot/workspace".to_string()
}
fn default_model() -> String {
    "anthropic/claude-opus-4-5".to_string()
}
fn default_provider() -> String {
    "auto".to_string()
}
fn default_max_tokens() -> u32 {
    8192
}
fn default_context_window_tokens() -> u32 {
    65_536
}
fn default_temperature() -> f32 {
    0.1
}
fn default_max_tool_iterations() -> u32 {
    200
}
fn default_max_tool_result_chars() -> u32 {
    16_000
}
fn default_timezone() -> String {
    "UTC".to_string()
}

impl Default for AgentDefaults {
    fn default() -> Self {
        Self {
            workspace: default_workspace(),
            model: default_model(),
            provider: default_provider(),
            max_tokens: default_max_tokens(),
            context_window_tokens: default_context_window_tokens(),
            context_block_limit: None,
            temperature: default_temperature(),
            max_tool_iterations: default_max_tool_iterations(),
            max_tool_result_chars: default_max_tool_result_chars(),
            provider_retry_mode: ProviderRetryMode::Standard,
            reasoning_effort: None,
            timezone: default_timezone(),
            unified_session: false,
            disabled_skills: Vec::new(),
            session_ttl_minutes: 0,
            dream: DreamConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentsConfig {
    #[serde(default)]
    pub defaults: AgentDefaults,
}

// ---------------------------------------------------------------------------
// Providers
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfig {
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub api_base: Option<String>,
    /// Custom headers (e.g. `APP-Code` for AiHubMix).
    #[serde(default)]
    pub extra_headers: Option<HashMap<String, String>>,
}

/// Configuration for LLM providers. Mirrors the Python `ProvidersConfig`
/// (field order preserved).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProvidersConfig {
    /// Any OpenAI-compatible endpoint.
    #[serde(default)]
    pub custom: ProviderConfig,
    /// Azure OpenAI (model = deployment name).
    #[serde(default)]
    pub azure_openai: ProviderConfig,
    #[serde(default)]
    pub anthropic: ProviderConfig,
    #[serde(default)]
    pub openai: ProviderConfig,
    #[serde(default)]
    pub openrouter: ProviderConfig,
    #[serde(default)]
    pub deepseek: ProviderConfig,
    #[serde(default)]
    pub groq: ProviderConfig,
    #[serde(default)]
    pub zhipu: ProviderConfig,
    #[serde(default)]
    pub dashscope: ProviderConfig,
    #[serde(default)]
    pub vllm: ProviderConfig,
    /// Ollama local models.
    #[serde(default)]
    pub ollama: ProviderConfig,
    /// LM Studio local models.
    #[serde(default)]
    pub lm_studio: ProviderConfig,
    /// OpenVINO Model Server (OVMS).
    #[serde(default)]
    pub ovms: ProviderConfig,
    #[serde(default)]
    pub gemini: ProviderConfig,
    #[serde(default)]
    pub moonshot: ProviderConfig,
    #[serde(default)]
    pub minimax: ProviderConfig,
    /// MiniMax Anthropic endpoint (thinking).
    #[serde(default)]
    pub minimax_anthropic: ProviderConfig,
    #[serde(default)]
    pub mistral: ProviderConfig,
    /// Step Fun (阶跃星辰).
    #[serde(default)]
    pub stepfun: ProviderConfig,
    /// Xiaomi MIMO (小米).
    #[serde(default)]
    pub xiaomi_mimo: ProviderConfig,
    /// AiHubMix API gateway.
    #[serde(default)]
    pub aihubmix: ProviderConfig,
    /// SiliconFlow (硅基流动).
    #[serde(default)]
    pub siliconflow: ProviderConfig,
    /// VolcEngine (火山引擎).
    #[serde(default)]
    pub volcengine: ProviderConfig,
    /// VolcEngine Coding Plan.
    #[serde(default)]
    pub volcengine_coding_plan: ProviderConfig,
    /// BytePlus (VolcEngine international).
    #[serde(default)]
    pub byteplus: ProviderConfig,
    /// BytePlus Coding Plan.
    #[serde(default)]
    pub byteplus_coding_plan: ProviderConfig,
    /// OpenAI Codex (OAuth) — never serialized.
    #[serde(default, skip_serializing)]
    pub openai_codex: ProviderConfig,
    /// Github Copilot (OAuth) — never serialized.
    #[serde(default, skip_serializing)]
    pub github_copilot: ProviderConfig,
    /// Qianfan (百度千帆).
    #[serde(default)]
    pub qianfan: ProviderConfig,
}

// ---------------------------------------------------------------------------
// API / Gateway
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HeartbeatConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 30 minutes.
    #[serde(default = "default_heartbeat_interval_s")]
    pub interval_s: u32,
    #[serde(default = "default_keep_recent_messages")]
    pub keep_recent_messages: u32,
}

fn default_heartbeat_interval_s() -> u32 {
    30 * 60
}
fn default_keep_recent_messages() -> u32 {
    8
}

impl Default for HeartbeatConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            interval_s: 30 * 60,
            keep_recent_messages: 8,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiConfig {
    /// Safer default: local-only bind.
    #[serde(default = "default_localhost")]
    pub host: String,
    #[serde(default = "default_api_port")]
    pub port: u16,
    /// Per-request timeout in seconds.
    #[serde(default = "default_api_timeout")]
    pub timeout: f32,
}

fn default_localhost() -> String {
    "127.0.0.1".to_string()
}
fn default_api_port() -> u16 {
    8900
}
fn default_api_timeout() -> f32 {
    120.0
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            host: default_localhost(),
            port: default_api_port(),
            timeout: default_api_timeout(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayConfig {
    #[serde(default = "default_localhost")]
    pub host: String,
    #[serde(default = "default_gateway_port")]
    pub port: u16,
    #[serde(default)]
    pub heartbeat: HeartbeatConfig,
}

fn default_gateway_port() -> u16 {
    18790
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            host: default_localhost(),
            port: default_gateway_port(),
            heartbeat: HeartbeatConfig::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// Tools
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebSearchConfig {
    /// brave, tavily, duckduckgo, searxng, jina, kagi.
    #[serde(default = "default_web_search_provider")]
    pub provider: String,
    #[serde(default)]
    pub api_key: String,
    /// SearXNG base URL.
    #[serde(default)]
    pub base_url: String,
    #[serde(default = "default_web_search_max_results")]
    pub max_results: u32,
    /// Wall-clock timeout (seconds) for search operations.
    #[serde(default = "default_web_search_timeout")]
    pub timeout: u32,
}

fn default_web_search_provider() -> String {
    "duckduckgo".to_string()
}
fn default_web_search_max_results() -> u32 {
    5
}
fn default_web_search_timeout() -> u32 {
    30
}

impl Default for WebSearchConfig {
    fn default() -> Self {
        Self {
            provider: default_web_search_provider(),
            api_key: String::new(),
            base_url: String::new(),
            max_results: 5,
            timeout: 30,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebToolsConfig {
    #[serde(default = "default_true")]
    pub enable: bool,
    /// HTTP/SOCKS5 proxy URL.
    #[serde(default)]
    pub proxy: Option<String>,
    #[serde(default)]
    pub search: WebSearchConfig,
}

impl Default for WebToolsConfig {
    fn default() -> Self {
        Self {
            enable: true,
            proxy: None,
            search: WebSearchConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecToolConfig {
    #[serde(default = "default_true")]
    pub enable: bool,
    #[serde(default = "default_exec_timeout")]
    pub timeout: u32,
    #[serde(default)]
    pub path_append: String,
    /// Sandbox backend: "" (none) or "bwrap".
    #[serde(default)]
    pub sandbox: String,
    /// Env var names to pass through to subprocess.
    #[serde(default)]
    pub allowed_env_keys: Vec<String>,
}

fn default_exec_timeout() -> u32 {
    60
}

impl Default for ExecToolConfig {
    fn default() -> Self {
        Self {
            enable: true,
            timeout: 60,
            path_append: String::new(),
            sandbox: String::new(),
            allowed_env_keys: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum McpTransport {
    Stdio,
    Sse,
    StreamableHttp,
}

/// MCP server connection configuration (stdio or HTTP).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerConfig {
    /// Auto-detected if omitted.
    #[serde(default, rename = "type")]
    pub transport: Option<McpTransport>,
    /// Stdio: command to run (e.g. "npx").
    #[serde(default)]
    pub command: String,
    /// Stdio: command arguments.
    #[serde(default)]
    pub args: Vec<String>,
    /// Stdio: extra env vars.
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// HTTP/SSE: endpoint URL.
    #[serde(default)]
    pub url: String,
    /// HTTP/SSE: custom headers.
    #[serde(default)]
    pub headers: HashMap<String, String>,
    /// Seconds before a tool call is cancelled.
    #[serde(default = "default_tool_timeout")]
    pub tool_timeout: u32,
    /// Only register these tools; `["*"]` = all tools; `[]` = none.
    #[serde(default = "default_enabled_tools")]
    pub enabled_tools: Vec<String>,
}

fn default_tool_timeout() -> u32 {
    30
}
fn default_enabled_tools() -> Vec<String> {
    vec!["*".to_string()]
}

impl Default for McpServerConfig {
    fn default() -> Self {
        Self {
            transport: None,
            command: String::new(),
            args: Vec::new(),
            env: HashMap::new(),
            url: String::new(),
            headers: HashMap::new(),
            tool_timeout: default_tool_timeout(),
            enabled_tools: default_enabled_tools(),
        }
    }
}

/// Self-inspection tool configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MyToolConfig {
    /// Register the `my` tool.
    #[serde(default = "default_true")]
    pub enable: bool,
    /// Let `my` modify loop state (read-only if `false`).
    #[serde(default)]
    pub allow_set: bool,
}

impl Default for MyToolConfig {
    fn default() -> Self {
        Self {
            enable: true,
            allow_set: false,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolsConfig {
    #[serde(default)]
    pub web: WebToolsConfig,
    #[serde(default)]
    pub exec: ExecToolConfig,
    #[serde(default)]
    pub my: MyToolConfig,
    /// Restrict all tool access to workspace directory.
    #[serde(default)]
    pub restrict_to_workspace: bool,
    #[serde(default)]
    pub mcp_servers: HashMap<String, McpServerConfig>,
    /// CIDR ranges to exempt from SSRF blocking.
    #[serde(default)]
    pub ssrf_whitelist: Vec<String>,
}

// ---------------------------------------------------------------------------
// Root config
// ---------------------------------------------------------------------------

/// Root configuration for nanobot.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    #[serde(default)]
    pub agents: AgentsConfig,
    #[serde(default)]
    pub channels: ChannelsConfig,
    #[serde(default)]
    pub providers: ProvidersConfig,
    #[serde(default)]
    pub api: ApiConfig,
    #[serde(default)]
    pub gateway: GatewayConfig,
    #[serde(default)]
    pub tools: ToolsConfig,
}

impl Config {
    /// Expanded workspace path (Python `workspace_path` property).
    pub fn workspace_path(&self) -> PathBuf {
        expand_tilde(&self.agents.defaults.workspace)
    }

    /// Save configuration to `config_path` (or the default) as pretty JSON.
    pub fn save_config(&self, config_path: Option<&Path>) -> Result<(), ConfigError> {
        let path = match config_path {
            Some(p) => p.to_path_buf(),
            None => get_config_path(),
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let data = serde_json::to_string_pretty(self)?;
        fs::write(path, data)?;
        Ok(())
    }

    pub fn json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap()
    }

    /// Return a copy of `config` with any `${VAR}` env-var references resolved.
    ///
    /// Errors if a referenced variable is not set. Fields skipped from
    /// serialization (e.g. [`crate::config::schema::DreamConfig::cron`]) do not
    /// round-trip through JSON and are preserved from the input.
    pub fn resolve_env_vars(&self) -> Result<Config, ConfigError> {
        let mut value = serde_json::to_value(self)?;
        resolve_value(&mut value)?;
        let mut resolved: Config = serde_json::from_value(value)?;

        // Preserve fields that are `skip_serializing` and thus don't round-trip.
        resolved.agents.defaults.dream.cron = self.agents.defaults.dream.cron.clone();
        resolved.providers.openai_codex = self.providers.openai_codex.clone();
        resolved.providers.github_copilot = self.providers.github_copilot.clone();

        Ok(resolved)
    }

    // ---------------------------------------------------------------------------
    // Load / Save
    // ---------------------------------------------------------------------------

    /// Load configuration from `config_path` (or the default) and return a
    /// [`Config`]. A corrupt/unparseable file is logged and the default config
    /// is returned, matching the Python `load_config` behavior.
    pub fn from_config(config_path: Option<&Path>) -> Config {
        let path = match config_path {
            Some(p) => p.to_path_buf(),
            None => get_config_path(),
        };

        if !path.exists() {
            return Config::default();
        }

        match try_load(&path) {
            Ok(cfg) => cfg,
            Err(err) => {
                log::warn!("Failed to load config from {:?}: {}", path, err);
                log::warn!("Using default configuration.");
                Config::default()
            }
        }
    }
}
