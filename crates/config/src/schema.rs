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
use cron::types::{CronSchedule, ScheduleKind};
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

    /// Surface model reasoning when channel implements it.
    /// / 当频道实现时展示模型推理。
    #[serde(default = "default_true")]
    pub show_reasoning: bool,

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
            show_reasoning: true,
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
    #[serde(default, alias = "model", alias = "model_override", alias = "modelOverride")]
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

    /// Build the runtime schedule, preferring the legacy cron override if present.
    pub fn build_schedule(&self, timezone: &str) -> CronSchedule {
        if let Some(expr) = &self.cron {
            CronSchedule {
                kind: ScheduleKind::Cron,
                expr: Some(expr.clone()),
                tz: Some(timezone.to_string()),
                at_ms: None,
                every_ms: None,
            }
        } else {
            CronSchedule {
                kind: ScheduleKind::Every,
                every_ms: Some(self.interval_h as i64 * HOUR_MS as i64),
                at_ms: None,
                expr: None,
                tz: None,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Agents
// ---------------------------------------------------------------------------

/// One inline fallback model configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InlineFallbackConfig {
    pub model: String,
    pub provider: String,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub context_window_tokens: Option<u32>,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub reasoning_effort: Option<String>,
}

/// A fallback candidate: either a preset name (String) or an inline config.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FallbackCandidate {
    PresetName(String),
    Inline(InlineFallbackConfig),
}

/// A named set of model + generation parameters for quick switching.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelPresetConfig {
    pub model: String,
    #[serde(default = "default_provider")]
    pub provider: String,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    #[serde(default = "default_context_window_tokens")]
    pub context_window_tokens: u32,
    #[serde(default = "default_temperature")]
    pub temperature: f32,
    #[serde(default)]
    pub reasoning_effort: Option<String>,
}

impl Default for ModelPresetConfig {
    fn default() -> Self {
        Self {
            model: "anthropic/claude-opus-4-5".to_string(),
            provider: "auto".to_string(),
            max_tokens: 8192,
            context_window_tokens: 65_536,
            temperature: 0.1,
            reasoning_effort: None,
        }
    }
}

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

    /// Active preset name — takes precedence over fields below.
    #[serde(default)]
    pub model_preset: Option<String>,

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

    #[serde(default)]
    pub fallback_models: Vec<FallbackCandidate>,

    #[serde(default = "default_max_tool_iterations")]
    pub max_tool_iterations: u32,

    #[serde(default = "default_max_concurrent_subagents")]
    pub max_concurrent_subagents: u32,

    #[serde(default = "default_max_tool_result_chars")]
    pub max_tool_result_chars: u32,

    #[serde(default)]
    pub provider_retry_mode: ProviderRetryMode,

    /// Max characters for tool hint display. 20..=500.
    #[serde(
        default = "default_tool_hint_max_length",
        rename = "toolHintMaxLength",
        alias = "tool_hint_max_length"
    )]
    pub tool_hint_max_length: u32,

    /// low / medium / high / adaptive / none — enables LLM thinking mode.
    #[serde(default)]
    pub reasoning_effort: Option<String>,

    /// IANA timezone, e.g. "Asia/Shanghai".
    #[serde(default = "default_timezone")]
    pub timezone: String,

    /// Display name shown in CLI prompts.
    #[serde(default = "default_bot_name")]
    pub bot_name: String,

    /// Short icon (emoji or text) shown next to the bot name in CLI.
    #[serde(default = "default_bot_icon")]
    pub bot_icon: String,

    /// Max messages to replay from session history (0 = use default 120).
    #[serde(default = "default_max_messages")]
    pub max_messages: u32,

    /// Consolidation target ratio (0.5 = 50% of budget retained). 0.1..=0.95.
    #[serde(
        default = "default_consolidation_ratio",
        rename = "consolidationRatio",
        alias = "consolidation_ratio"
    )]
    pub consolidation_ratio: f32,

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
fn default_max_concurrent_subagents() -> u32 {
    1
}
fn default_max_tool_result_chars() -> u32 {
    16_000
}
fn default_tool_hint_max_length() -> u32 {
    40
}
fn default_timezone() -> String {
    "UTC".to_string()
}
fn default_bot_name() -> String {
    "nanobot".to_string()
}
fn default_bot_icon() -> String {
    "🐈".to_string()
}
fn default_max_messages() -> u32 {
    120
}
fn default_consolidation_ratio() -> f32 {
    0.5
}

impl Default for AgentDefaults {
    fn default() -> Self {
        Self {
            workspace: default_workspace(),
            model_preset: None,
            model: default_model(),
            provider: default_provider(),
            max_tokens: default_max_tokens(),
            context_window_tokens: default_context_window_tokens(),
            context_block_limit: None,
            temperature: default_temperature(),
            fallback_models: Vec::new(),
            max_tool_iterations: default_max_tool_iterations(),
            max_concurrent_subagents: default_max_concurrent_subagents(),
            max_tool_result_chars: default_max_tool_result_chars(),
            provider_retry_mode: ProviderRetryMode::Standard,
            tool_hint_max_length: default_tool_hint_max_length(),
            reasoning_effort: None,
            timezone: default_timezone(),
            bot_name: default_bot_name(),
            bot_icon: default_bot_icon(),
            max_messages: default_max_messages(),
            consolidation_ratio: default_consolidation_ratio(),
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
    /// Extra fields merged into every request body.
    #[serde(default)]
    pub extra_body: Option<serde_json::Value>,
}

/// AWS Bedrock Runtime provider configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BedrockProviderConfig {
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub api_base: Option<String>,
    #[serde(default)]
    pub extra_headers: Option<HashMap<String, String>>,
    #[serde(default)]
    pub extra_body: Option<serde_json::Value>,
    /// AWS region, falls back to AWS_REGION/AWS_DEFAULT_REGION/profile.
    #[serde(default)]
    pub region: Option<String>,
    /// Optional AWS shared config profile.
    #[serde(default)]
    pub profile: Option<String>,
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
    /// AWS Bedrock Converse.
    #[serde(default)]
    pub bedrock: BedrockProviderConfig,
    #[serde(default)]
    pub anthropic: ProviderConfig,
    #[serde(default)]
    pub openai: ProviderConfig,
    #[serde(default)]
    pub openrouter: ProviderConfig,
    #[serde(default)]
    pub huggingface: ProviderConfig,
    /// Skywork / APIFree API gateway.
    #[serde(default)]
    pub skywork: ProviderConfig,
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
    /// Atomic Chat local models.
    #[serde(default)]
    pub atomic_chat: ProviderConfig,
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
    /// LongCat.
    #[serde(default)]
    pub longcat: ProviderConfig,
    /// Ant Ling.
    #[serde(default)]
    pub ant_ling: ProviderConfig,
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
    /// NVIDIA NIM (nvapi- keys).
    #[serde(default)]
    pub nvidia: ProviderConfig,
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

/// Image generation tool configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageGenerationToolConfig {
    /// Register the image generation tool.
    #[serde(default = "default_true")]
    pub enable: bool,
    /// Image generation provider (e.g. "openai", "stability").
    #[serde(default)]
    pub provider: String,
    /// Default image size (e.g. "1024x1024").
    #[serde(default)]
    pub size: Option<String>,
    /// Default image quality.
    #[serde(default)]
    pub quality: Option<String>,
}

impl Default for ImageGenerationToolConfig {
    fn default() -> Self {
        Self {
            enable: true,
            provider: String::new(),
            size: None,
            quality: None,
        }
    }
}

/// Balance query tool configuration — connects to an external database.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BalanceToolConfig {
    /// Database connection URL for the external balance database.
    /// e.g. "mysql://user:password@host:3306/database"
    #[serde(default)]
    pub db_url: Option<String>,
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
    #[serde(default)]
    pub image_generation: ImageGenerationToolConfig,
    #[serde(default)]
    pub balance: BalanceToolConfig,
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
    #[serde(
        default,
        rename = "modelPresets",
        alias = "model_presets"
    )]
    pub model_presets: HashMap<String, ModelPresetConfig>,
}

/// Provider spec for matching (simplified version of Python PROVIDERS registry).
struct ProviderSpec {
    name: &'static str,
    keywords: &'static [&'static str],
    is_oauth: bool,
    is_local: bool,
    is_direct: bool,
    default_api_base: Option<&'static str>,
    detect_by_base_keyword: Option<&'static str>,
}

const PROVIDERS: &[ProviderSpec] = &[
    ProviderSpec { name: "anthropic", keywords: &["claude", "anthropic"], is_oauth: false, is_local: false, is_direct: true, default_api_base: Some("https://api.anthropic.com/v1"), detect_by_base_keyword: None },
    ProviderSpec { name: "openai", keywords: &["gpt", "o1", "o3", "openai"], is_oauth: false, is_local: false, is_direct: true, default_api_base: Some("https://api.openai.com/v1"), detect_by_base_keyword: None },
    ProviderSpec { name: "openrouter", keywords: &["openrouter"], is_oauth: false, is_local: false, is_direct: true, default_api_base: Some("https://openrouter.ai/api/v1"), detect_by_base_keyword: None },
    ProviderSpec { name: "deepseek", keywords: &["deepseek"], is_oauth: false, is_local: false, is_direct: true, default_api_base: Some("https://api.deepseek.com"), detect_by_base_keyword: None },
    ProviderSpec { name: "groq", keywords: &["groq"], is_oauth: false, is_local: false, is_direct: true, default_api_base: Some("https://api.groq.com/openai/v1"), detect_by_base_keyword: None },
    ProviderSpec { name: "zhipu", keywords: &["zhipu", "glm"], is_oauth: false, is_local: false, is_direct: true, default_api_base: None, detect_by_base_keyword: None },
    ProviderSpec { name: "dashscope", keywords: &["dashscope", "qwen"], is_oauth: false, is_local: false, is_direct: true, default_api_base: None, detect_by_base_keyword: None },
    ProviderSpec { name: "huggingface", keywords: &["huggingface", "hugging", "hf"], is_oauth: false, is_local: false, is_direct: true, default_api_base: None, detect_by_base_keyword: None },
    ProviderSpec { name: "skywork", keywords: &["skywork"], is_oauth: false, is_local: false, is_direct: true, default_api_base: None, detect_by_base_keyword: None },
    ProviderSpec { name: "moonshot", keywords: &["moonshot"], is_oauth: false, is_local: false, is_direct: true, default_api_base: None, detect_by_base_keyword: None },
    ProviderSpec { name: "minimax", keywords: &["minimax"], is_oauth: false, is_local: false, is_direct: true, default_api_base: None, detect_by_base_keyword: None },
    ProviderSpec { name: "minimax_anthropic", keywords: &["minimax-anthropic"], is_oauth: false, is_local: false, is_direct: true, default_api_base: None, detect_by_base_keyword: None },
    ProviderSpec { name: "mistral", keywords: &["mistral"], is_oauth: false, is_local: false, is_direct: true, default_api_base: Some("https://api.mistral.ai/v1"), detect_by_base_keyword: None },
    ProviderSpec { name: "stepfun", keywords: &["stepfun"], is_oauth: false, is_local: false, is_direct: true, default_api_base: None, detect_by_base_keyword: None },
    ProviderSpec { name: "xiaomi_mimo", keywords: &["mimo", "xiaomi"], is_oauth: false, is_local: false, is_direct: true, default_api_base: None, detect_by_base_keyword: None },
    ProviderSpec { name: "longcat", keywords: &["longcat"], is_oauth: false, is_local: false, is_direct: true, default_api_base: None, detect_by_base_keyword: None },
    ProviderSpec { name: "ant_ling", keywords: &["ant_ling", "antling"], is_oauth: false, is_local: false, is_direct: true, default_api_base: None, detect_by_base_keyword: None },
    ProviderSpec { name: "aihubmix", keywords: &["aihubmix"], is_oauth: false, is_local: false, is_direct: true, default_api_base: None, detect_by_base_keyword: None },
    ProviderSpec { name: "siliconflow", keywords: &["siliconflow"], is_oauth: false, is_local: false, is_direct: true, default_api_base: None, detect_by_base_keyword: None },
    ProviderSpec { name: "volcengine", keywords: &["volcengine"], is_oauth: false, is_local: false, is_direct: true, default_api_base: None, detect_by_base_keyword: None },
    ProviderSpec { name: "volcengine_coding_plan", keywords: &["volcengine-coding-plan"], is_oauth: false, is_local: false, is_direct: true, default_api_base: None, detect_by_base_keyword: None },
    ProviderSpec { name: "byteplus", keywords: &["byteplus"], is_oauth: false, is_local: false, is_direct: true, default_api_base: None, detect_by_base_keyword: None },
    ProviderSpec { name: "byteplus_coding_plan", keywords: &["byteplus-coding-plan"], is_oauth: false, is_local: false, is_direct: true, default_api_base: None, detect_by_base_keyword: None },
    ProviderSpec { name: "qianfan", keywords: &["qianfan", "baidu"], is_oauth: false, is_local: false, is_direct: true, default_api_base: None, detect_by_base_keyword: None },
    ProviderSpec { name: "nvidia", keywords: &["nvidia", "nvapi"], is_oauth: false, is_local: false, is_direct: true, default_api_base: Some("https://integrate.api.nvidia.com/v1"), detect_by_base_keyword: None },
    ProviderSpec { name: "gemini", keywords: &["gemini", "google"], is_oauth: false, is_local: false, is_direct: true, default_api_base: None, detect_by_base_keyword: None },
    ProviderSpec { name: "ollama", keywords: &["ollama"], is_oauth: false, is_local: true, is_direct: false, default_api_base: Some("http://localhost:11434/v1"), detect_by_base_keyword: Some("11434") },
    ProviderSpec { name: "lm_studio", keywords: &["lm-studio"], is_oauth: false, is_local: true, is_direct: false, default_api_base: Some("http://localhost:1234/v1"), detect_by_base_keyword: Some("1234") },
    ProviderSpec { name: "atomic_chat", keywords: &["atomic-chat"], is_oauth: false, is_local: true, is_direct: false, default_api_base: None, detect_by_base_keyword: None },
    ProviderSpec { name: "vllm", keywords: &["vllm"], is_oauth: false, is_local: true, is_direct: false, default_api_base: Some("http://localhost:8000/v1"), detect_by_base_keyword: Some("8000") },
    ProviderSpec { name: "ovms", keywords: &["ovms", "openvino"], is_oauth: false, is_local: true, is_direct: false, default_api_base: None, detect_by_base_keyword: None },
    ProviderSpec { name: "openai_codex", keywords: &["codex"], is_oauth: true, is_local: false, is_direct: false, default_api_base: None, detect_by_base_keyword: None },
    ProviderSpec { name: "github_copilot", keywords: &["github-copilot", "copilot"], is_oauth: true, is_local: false, is_direct: false, default_api_base: None, detect_by_base_keyword: None },
    ProviderSpec { name: "azure_openai", keywords: &["azure"], is_oauth: false, is_local: false, is_direct: false, default_api_base: None, detect_by_base_keyword: None },
    ProviderSpec { name: "bedrock", keywords: &["bedrock"], is_oauth: false, is_local: false, is_direct: true, default_api_base: None, detect_by_base_keyword: None },
    ProviderSpec { name: "custom", keywords: &[], is_oauth: false, is_local: false, is_direct: false, default_api_base: None, detect_by_base_keyword: None },
];

fn find_provider_spec(name: &str) -> Option<&'static ProviderSpec> {
    PROVIDERS.iter().find(|s| s.name == name)
}

impl Config {
    /// Expanded workspace path (Python `workspace_path` property).
    pub fn workspace_path(&self) -> PathBuf {
        expand_tilde(&self.agents.defaults.workspace)
    }

    /// Return the implicit `default` preset from agents.defaults fields.
    pub fn resolve_default_preset(&self) -> ModelPresetConfig {
        let d = &self.agents.defaults;
        ModelPresetConfig {
            model: d.model.clone(),
            provider: d.provider.clone(),
            max_tokens: d.max_tokens,
            context_window_tokens: d.context_window_tokens,
            temperature: d.temperature,
            reasoning_effort: d.reasoning_effort.clone(),
        }
    }

    /// Return effective model params from a named preset or the implicit default.
    pub fn resolve_preset(&self, name: Option<&str>) -> Result<ModelPresetConfig, ConfigError> {
        let name = match name {
            Some(n) => n,
            None => self.agents.defaults.model_preset.as_deref().unwrap_or("default"),
        };
        if name.is_empty() || name == "default" {
            return Ok(self.resolve_default_preset());
        }
        self.model_presets
            .get(name)
            .cloned()
            .ok_or_else(|| ConfigError::PresetNotFound(name.to_string()))
    }

    /// Match provider config and its registry name. Returns (config, spec_name).
    fn match_provider(
        &self,
        model: Option<&str>,
        preset: Option<&ModelPresetConfig>,
    ) -> (Option<ProviderConfig>, Option<String>) {
        let resolved = preset.map(|p| p.clone()).unwrap_or_else(|| self.resolve_preset(None).unwrap_or_else(|_| self.resolve_default_preset()));
        let forced = &resolved.provider;

        if forced != "auto" {
            if let Some(spec) = find_provider_spec(forced) {
                return self.get_provider_by_spec(spec);
            }
            return (None, None);
        }

        let model_lower = model.unwrap_or(&resolved.model).to_lowercase();
        let model_normalized = model_lower.replace('-', "_");
        let model_prefix = if model_lower.contains('/') {
            model_lower.split('/').next().unwrap_or("").to_string()
        } else {
            String::new()
        };
        let normalized_prefix = model_prefix.replace('-', "_");

        fn kw_matches(kw: &str, model_lower: &str, model_normalized: &str) -> bool {
            let kw_lower = kw.to_lowercase();
            model_lower.contains(&kw_lower) || model_normalized.contains(&kw_lower.replace('-', "_"))
        }

        // Explicit provider prefix wins
        for spec in PROVIDERS {
            let p = self.get_provider_by_spec(spec);
            if p.0.is_some() && !model_prefix.is_empty() && normalized_prefix == spec.name {
                if spec.is_oauth || spec.is_local || spec.is_direct || self.has_api_key(spec.name) {
                    return p;
                }
            }
        }

        // Match by keyword
        for spec in PROVIDERS {
            let p = self.get_provider_by_spec(spec);
            if p.0.is_some() && spec.keywords.iter().any(|kw| kw_matches(kw, &model_lower, &model_normalized)) {
                if spec.is_oauth || spec.is_local || spec.is_direct || self.has_api_key(spec.name) {
                    return p;
                }
            }
        }

        // Fallback: configured local providers
        let mut local_fallback: Option<(ProviderConfig, String)> = None;
        for spec in PROVIDERS {
            if !spec.is_local {
                continue;
            }
            let p = self.get_provider_by_spec(spec);
            if let (Some(provider), _) = &p {
                if let Some(api_base) = &provider.api_base {
                    if let Some(detect_kw) = spec.detect_by_base_keyword {
                        if api_base.contains(detect_kw) {
                            return p;
                        }
                    }
                    if local_fallback.is_none() {
                        local_fallback = Some((provider.clone(), spec.name.to_string()));
                    }
                }
            }
        }
        if let Some((cfg, name)) = local_fallback {
            return (Some(cfg), Some(name));
        }

        // Fallback: gateways first, then others (OAuth excluded)
        for spec in PROVIDERS {
            if spec.is_oauth {
                continue;
            }
            if self.has_api_key(spec.name) {
                return self.get_provider_by_spec(spec);
            }
        }

        (None, None)
    }

    fn get_provider_by_spec(&self, spec: &ProviderSpec) -> (Option<ProviderConfig>, Option<String>) {
        let has_config = match spec.name {
            "anthropic" => self.providers.anthropic.api_key.is_some() || self.providers.anthropic.api_base.is_some(),
            "openai" => self.providers.openai.api_key.is_some() || self.providers.openai.api_base.is_some(),
            "openrouter" => self.providers.openrouter.api_key.is_some() || self.providers.openrouter.api_base.is_some(),
            "deepseek" => self.providers.deepseek.api_key.is_some() || self.providers.deepseek.api_base.is_some(),
            "groq" => self.providers.groq.api_key.is_some() || self.providers.groq.api_base.is_some(),
            "zhipu" => self.providers.zhipu.api_key.is_some() || self.providers.zhipu.api_base.is_some(),
            "dashscope" => self.providers.dashscope.api_key.is_some() || self.providers.dashscope.api_base.is_some(),
            "huggingface" => self.providers.huggingface.api_key.is_some() || self.providers.huggingface.api_base.is_some(),
            "skywork" => self.providers.skywork.api_key.is_some() || self.providers.skywork.api_base.is_some(),
            "moonshot" => self.providers.moonshot.api_key.is_some() || self.providers.moonshot.api_base.is_some(),
            "minimax" => self.providers.minimax.api_key.is_some() || self.providers.minimax.api_base.is_some(),
            "minimax_anthropic" => self.providers.minimax_anthropic.api_key.is_some() || self.providers.minimax_anthropic.api_base.is_some(),
            "mistral" => self.providers.mistral.api_key.is_some() || self.providers.mistral.api_base.is_some(),
            "stepfun" => self.providers.stepfun.api_key.is_some() || self.providers.stepfun.api_base.is_some(),
            "xiaomi_mimo" => self.providers.xiaomi_mimo.api_key.is_some() || self.providers.xiaomi_mimo.api_base.is_some(),
            "longcat" => self.providers.longcat.api_key.is_some() || self.providers.longcat.api_base.is_some(),
            "ant_ling" => self.providers.ant_ling.api_key.is_some() || self.providers.ant_ling.api_base.is_some(),
            "aihubmix" => self.providers.aihubmix.api_key.is_some() || self.providers.aihubmix.api_base.is_some(),
            "siliconflow" => self.providers.siliconflow.api_key.is_some() || self.providers.siliconflow.api_base.is_some(),
            "volcengine" => self.providers.volcengine.api_key.is_some() || self.providers.volcengine.api_base.is_some(),
            "volcengine_coding_plan" => self.providers.volcengine_coding_plan.api_key.is_some() || self.providers.volcengine_coding_plan.api_base.is_some(),
            "byteplus" => self.providers.byteplus.api_key.is_some() || self.providers.byteplus.api_base.is_some(),
            "byteplus_coding_plan" => self.providers.byteplus_coding_plan.api_key.is_some() || self.providers.byteplus_coding_plan.api_base.is_some(),
            "qianfan" => self.providers.qianfan.api_key.is_some() || self.providers.qianfan.api_base.is_some(),
            "nvidia" => self.providers.nvidia.api_key.is_some() || self.providers.nvidia.api_base.is_some(),
            "gemini" => self.providers.gemini.api_key.is_some() || self.providers.gemini.api_base.is_some(),
            "ollama" => self.providers.ollama.api_key.is_some() || self.providers.ollama.api_base.is_some(),
            "lm_studio" => self.providers.lm_studio.api_key.is_some() || self.providers.lm_studio.api_base.is_some(),
            "atomic_chat" => self.providers.atomic_chat.api_key.is_some() || self.providers.atomic_chat.api_base.is_some(),
            "vllm" => self.providers.vllm.api_key.is_some() || self.providers.vllm.api_base.is_some(),
            "ovms" => self.providers.ovms.api_key.is_some() || self.providers.ovms.api_base.is_some(),
            "openai_codex" => self.providers.openai_codex.api_key.is_some() || self.providers.openai_codex.api_base.is_some(),
            "github_copilot" => self.providers.github_copilot.api_key.is_some() || self.providers.github_copilot.api_base.is_some(),
            "azure_openai" => self.providers.azure_openai.api_key.is_some() || self.providers.azure_openai.api_base.is_some(),
            "bedrock" => self.providers.bedrock.api_key.is_some() || self.providers.bedrock.api_base.is_some(),
            "custom" => self.providers.custom.api_key.is_some() || self.providers.custom.api_base.is_some(),
            _ => false,
        };

        if !has_config {
            return (None, None);
        }

        let config = match spec.name {
            "anthropic" => Some(self.providers.anthropic.clone()),
            "openai" => Some(self.providers.openai.clone()),
            "openrouter" => Some(self.providers.openrouter.clone()),
            "deepseek" => Some(self.providers.deepseek.clone()),
            "groq" => Some(self.providers.groq.clone()),
            "zhipu" => Some(self.providers.zhipu.clone()),
            "dashscope" => Some(self.providers.dashscope.clone()),
            "huggingface" => Some(self.providers.huggingface.clone()),
            "skywork" => Some(self.providers.skywork.clone()),
            "moonshot" => Some(self.providers.moonshot.clone()),
            "minimax" => Some(self.providers.minimax.clone()),
            "minimax_anthropic" => Some(self.providers.minimax_anthropic.clone()),
            "mistral" => Some(self.providers.mistral.clone()),
            "stepfun" => Some(self.providers.stepfun.clone()),
            "xiaomi_mimo" => Some(self.providers.xiaomi_mimo.clone()),
            "longcat" => Some(self.providers.longcat.clone()),
            "ant_ling" => Some(self.providers.ant_ling.clone()),
            "aihubmix" => Some(self.providers.aihubmix.clone()),
            "siliconflow" => Some(self.providers.siliconflow.clone()),
            "volcengine" => Some(self.providers.volcengine.clone()),
            "volcengine_coding_plan" => Some(self.providers.volcengine_coding_plan.clone()),
            "byteplus" => Some(self.providers.byteplus.clone()),
            "byteplus_coding_plan" => Some(self.providers.byteplus_coding_plan.clone()),
            "qianfan" => Some(self.providers.qianfan.clone()),
            "nvidia" => Some(self.providers.nvidia.clone()),
            "gemini" => Some(self.providers.gemini.clone()),
            "ollama" => Some(self.providers.ollama.clone()),
            "lm_studio" => Some(self.providers.lm_studio.clone()),
            "atomic_chat" => Some(self.providers.atomic_chat.clone()),
            "vllm" => Some(self.providers.vllm.clone()),
            "ovms" => Some(self.providers.ovms.clone()),
            "openai_codex" => Some(self.providers.openai_codex.clone()),
            "github_copilot" => Some(self.providers.github_copilot.clone()),
            "azure_openai" => Some(self.providers.azure_openai.clone()),
            "bedrock" => Some(ProviderConfig {
                api_key: self.providers.bedrock.api_key.clone(),
                api_base: self.providers.bedrock.api_base.clone(),
                extra_headers: self.providers.bedrock.extra_headers.clone(),
                extra_body: self.providers.bedrock.extra_body.clone(),
            }),
            "custom" => Some(self.providers.custom.clone()),
            _ => None,
        };

        (config, Some(spec.name.to_string()))
    }

    fn has_api_key(&self, name: &str) -> bool {
        match name {
            "anthropic" => self.providers.anthropic.api_key.is_some(),
            "openai" => self.providers.openai.api_key.is_some(),
            "openrouter" => self.providers.openrouter.api_key.is_some(),
            "deepseek" => self.providers.deepseek.api_key.is_some(),
            "groq" => self.providers.groq.api_key.is_some(),
            "zhipu" => self.providers.zhipu.api_key.is_some(),
            "dashscope" => self.providers.dashscope.api_key.is_some(),
            "huggingface" => self.providers.huggingface.api_key.is_some(),
            "skywork" => self.providers.skywork.api_key.is_some(),
            "moonshot" => self.providers.moonshot.api_key.is_some(),
            "minimax" => self.providers.minimax.api_key.is_some(),
            "minimax_anthropic" => self.providers.minimax_anthropic.api_key.is_some(),
            "mistral" => self.providers.mistral.api_key.is_some(),
            "stepfun" => self.providers.stepfun.api_key.is_some(),
            "xiaomi_mimo" => self.providers.xiaomi_mimo.api_key.is_some(),
            "longcat" => self.providers.longcat.api_key.is_some(),
            "ant_ling" => self.providers.ant_ling.api_key.is_some(),
            "aihubmix" => self.providers.aihubmix.api_key.is_some(),
            "siliconflow" => self.providers.siliconflow.api_key.is_some(),
            "volcengine" => self.providers.volcengine.api_key.is_some(),
            "volcengine_coding_plan" => self.providers.volcengine_coding_plan.api_key.is_some(),
            "byteplus" => self.providers.byteplus.api_key.is_some(),
            "byteplus_coding_plan" => self.providers.byteplus_coding_plan.api_key.is_some(),
            "qianfan" => self.providers.qianfan.api_key.is_some(),
            "nvidia" => self.providers.nvidia.api_key.is_some(),
            "gemini" => self.providers.gemini.api_key.is_some(),
            "ollama" => self.providers.ollama.api_key.is_some(),
            "lm_studio" => self.providers.lm_studio.api_key.is_some(),
            "atomic_chat" => self.providers.atomic_chat.api_key.is_some(),
            "vllm" => self.providers.vllm.api_key.is_some(),
            "ovms" => self.providers.ovms.api_key.is_some(),
            "openai_codex" => self.providers.openai_codex.api_key.is_some(),
            "github_copilot" => self.providers.github_copilot.api_key.is_some(),
            "azure_openai" => self.providers.azure_openai.api_key.is_some(),
            "bedrock" => self.providers.bedrock.api_key.is_some(),
            "custom" => self.providers.custom.api_key.is_some(),
            _ => false,
        }
    }

    /// Get matched provider config (api_key, api_base, extra_headers). Falls back to first available.
    pub fn get_provider(
        &self,
        model: Option<&str>,
        preset: Option<&ModelPresetConfig>,
    ) -> Option<ProviderConfig> {
        let (p, _) = self.match_provider(model, preset);
        p
    }

    /// Get the registry name of the matched provider (e.g. "deepseek", "openrouter").
    pub fn get_provider_name(
        &self,
        model: Option<&str>,
        preset: Option<&ModelPresetConfig>,
    ) -> Option<String> {
        let (_, name) = self.match_provider(model, preset);
        name
    }

    /// Get API key for the given model. Falls back to first available key.
    pub fn get_api_key(
        &self,
        model: Option<&str>,
        preset: Option<&ModelPresetConfig>,
    ) -> Option<String> {
        let p = self.get_provider(model, preset);
        p.and_then(|provider| provider.api_key)
    }

    /// Get API base URL for the given model, falling back to the provider default when present.
    pub fn get_api_base(
        &self,
        model: Option<&str>,
        preset: Option<&ModelPresetConfig>,
    ) -> Option<String> {
        let (p, name) = self.match_provider(model, preset);
        if let Some(provider) = p {
            if let Some(api_base) = provider.api_base {
                return Some(api_base);
            }
        }
        if let Some(name_str) = name {
            if let Some(spec) = find_provider_spec(&name_str) {
                if let Some(default_base) = spec.default_api_base {
                    return Some(default_base.to_string());
                }
            }
        }
        None
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
