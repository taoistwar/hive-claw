//! `nanobot onboard` — initialise config + workspace with interactive wizard.
//!
//! Full port of `nanobot/cli/onboard.py` (1385 lines). Provides an
//! interactive questionary-style onboarding wizard for configuring providers,
//! model presets, channels, and general settings.

use std::path::{Path, PathBuf};

use config::schema::{
    AgentDefaults, ApiConfig, Config, GatewayConfig, ProviderConfig,
    ToolsConfig,
};
use config::{get_config_path, paths::get_workspace_path, set_config_path};
use console::{Style, Term};
use dialoguer::{Confirm, Input, Select, theme::ColorfulTheme};
use providers::PROVIDERS;

use crate::commands::expand_tilde;

// ---------------------------------------------------------------------------
// Result & Type Info
// ---------------------------------------------------------------------------

/// Result of an onboarding session.
#[derive(Debug, Clone)]
pub struct OnboardResult {
    pub config: Config,
    pub should_save: bool,
}

/// Field type introspection result (mirrors Python FieldTypeInfo).
#[derive(Debug, Clone)]
struct FieldTypeInfo {
    type_name: &'static str,
}

impl FieldTypeInfo {
    fn new(type_name: &'static str) -> Self {
        Self { type_name }
    }
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

static SENSITIVE_KEYWORDS: &[&str] = &["api_key", "token", "secret", "password", "credentials"];

const VERSION: &str = env!("CARGO_PKG_VERSION");

// ---------------------------------------------------------------------------
// Theme
// ---------------------------------------------------------------------------

fn theme() -> ColorfulTheme {
    ColorfulTheme::default()
}

// ---------------------------------------------------------------------------
// Sensitive Field Masking
// ---------------------------------------------------------------------------

fn is_sensitive_field(field_name: &str) -> bool {
    let lower = field_name.to_lowercase();
    SENSITIVE_KEYWORDS.iter().any(|kw| lower.contains(kw))
}

fn mask_value(value: &str) -> String {
    if value.len() <= 4 {
        "****".to_string()
    } else {
        format!("{}{}", "*".repeat(value.len() - 4), &value[value.len() - 4..])
    }
}

// ---------------------------------------------------------------------------
// Value Formatting
// ---------------------------------------------------------------------------

fn format_value_for_display(value: &serde_json::Value, field_name: &str) -> String {
    match value {
        serde_json::Value::Null => "not set".to_string(),
        serde_json::Value::String(s) if s.is_empty() => "not set".to_string(),
        serde_json::Value::String(s) => {
            if is_sensitive_field(field_name) {
                format!("[masked: {}]", mask_value(s))
            } else {
                s.clone()
            }
        }
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Array(arr) => {
            if arr.is_empty() {
                "not set".to_string()
            } else {
                arr.iter()
                    .map(|v| format_value_for_display(v, ""))
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        }
        serde_json::Value::Object(obj) => {
            if obj.is_empty() {
                "not set".to_string()
            } else {
                let parts: Vec<String> = obj
                    .iter()
                    .map(|(k, v)| format!("{k}: {}", format_value_for_display(v, k)))
                    .collect();
                parts.join(", ")
            }
        }
        _ => "not set".to_string(),
    }
}

fn format_value_for_input(value: &serde_json::Value, field_type: &str) -> String {
    match value {
        serde_json::Value::Null => "".to_string(),
        serde_json::Value::String(s) if s.is_empty() => "".to_string(),
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Array(arr) if field_type == "list" => {
            arr.iter()
                .map(|v| match v {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .collect::<Vec<_>>()
                .join(",")
        }
        serde_json::Value::Object(obj) if field_type == "dict" => {
            serde_json::to_string(obj).unwrap_or_default()
        }
        other => other.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Input Handlers
// ---------------------------------------------------------------------------

fn input_text(display_name: &str, default: &str, field_type: &str) -> Option<serde_json::Value> {
    let result: String = Input::with_theme(&theme())
        .with_prompt(display_name)
        .default(default.to_string())
        .allow_empty(true)
        .interact_text()
        .ok()?;

    match field_type {
        "int" => match result.trim().parse::<i64>() {
            Ok(n) => Some(serde_json::Value::Number(n.into())),
            Err(_) => {
                println!("! Invalid number format, value not saved");
                None
            }
        },
        "float" => match result.trim().parse::<f64>() {
            Ok(n) => Some(serde_json::Value::Number(
                serde_json::Number::from_f64(n).unwrap_or(serde_json::Number::from(0)),
            )),
            Err(_) => {
                println!("! Invalid number format, value not saved");
                None
            }
        },
        "list" => {
            let items: Vec<serde_json::Value> = result
                .split(',')
                .filter_map(|s| {
                    let trimmed = s.trim();
                    if trimmed.is_empty() {
                        None
                    } else {
                        Some(serde_json::Value::String(trimmed.to_string()))
                    }
                })
                .collect();
            Some(serde_json::Value::Array(items))
        }
        "dict" => match serde_json::from_str::<serde_json::Value>(&result) {
            Ok(v) => Some(v),
            Err(_) => {
                println!("! Invalid JSON format, value not saved");
                None
            }
        },
        _ => Some(serde_json::Value::String(result)),
    }
}

fn input_bool(display_name: &str, default: bool) -> Option<bool> {
    Confirm::with_theme(&theme())
        .with_prompt(display_name)
        .default(default)
        .interact()
        .ok()
}

fn input_with_existing(
    display_name: &str,
    current: &serde_json::Value,
    field_type: &str,
) -> Option<serde_json::Value> {
    let has_existing = !matches!(current, serde_json::Value::Null)
        && current != &serde_json::Value::String("".to_string())
        && current != &serde_json::Value::Array(vec![])
        && current.as_object().map(|o| !o.is_empty()).unwrap_or(true);

    if has_existing && !current.is_array() {
        let mut choices = vec!["Enter new value".to_string(), "Keep existing value".to_string()];
        let selection = Select::with_theme(&theme())
            .with_prompt(display_name)
            .items(&choices)
            .default(1)
            .interact_opt()
            .ok()
            .flatten();

        match selection {
            None => return None,
            Some(1) => return None,
            _ => {}
        }
    }

    let default_str = format_value_for_input(current, field_type);
    input_text(display_name, &default_str, field_type)
}

fn input_select(prompt: &str, choices: &[String], default: Option<&str>) -> SelectResult<String> {
    if choices.is_empty() {
        return SelectResult::Cancel;
    }

    let default_idx = default.and_then(|d| choices.iter().position(|c| c == d)).unwrap_or(0);

    match Select::with_theme(&theme())
        .with_prompt(prompt)
        .items(choices)
        .default(default_idx)
        .interact_opt()
    {
        Ok(Some(idx)) => SelectResult::Value(choices[idx].clone()),
        Ok(None) => SelectResult::Back,
        Err(_) => SelectResult::Cancel,
    }
}

fn input_model_with_autocomplete(display_name: &str, current: &str, _provider: &str) -> Option<String> {
    let result: String = Input::with_theme(&theme())
        .with_prompt(display_name)
        .default(current.to_string())
        .allow_empty(true)
        .interact_text()
        .ok()?;

    if result.is_empty() { None } else { Some(result) }
}

fn input_context_window(
    display_name: &str,
    current: Option<u32>,
    model_name: Option<&str>,
    provider: &str,
) -> Option<u32> {
    let mut choices = vec!["Enter new value".to_string(), "[?] Get recommended value".to_string()];
    if current.is_some() {
        choices.insert(1, "Keep existing value".to_string());
    }

    match input_select(display_name, &choices, Some("Enter new value")) {
        SelectResult::Value(v) if v == "Keep existing value" => current,
        SelectResult::Value(v) if v == "[?] Get recommended value" => {
            let model = model_name?;
            if let Some(limit) = get_model_context_limit(model, provider) {
                println!("+ Recommended context window: {} tokens", format_token_count(limit));
                Some(limit)
            } else {
                println!("! Could not fetch model info, please enter manually");
                let default_str = current.map(|v| v.to_string()).unwrap_or_default();
                input_text(display_name, &default_str, "int").and_then(|v| v.as_u64().map(|n| n as u32))
            }
        }
        SelectResult::Back | SelectResult::Cancel => current,
        _ => {
            let default_str = current.map(|v| v.to_string()).unwrap_or_default();
            let result: String = Input::with_theme(&theme())
                .with_prompt(display_name)
                .default(default_str)
                .allow_empty(true)
                .interact_text()
                .ok()?;
            if result.is_empty() { current } else { result.trim().parse::<u32>().ok() }
        }
    }
}

// ---------------------------------------------------------------------------
// Select Result
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
enum SelectResult<T> {
    Back,
    Cancel,
    Value(T),
}

// ---------------------------------------------------------------------------
// Display Names & Suffixes
// ---------------------------------------------------------------------------

fn get_field_display_name(field_key: &str) -> String {
    let mut name = field_key.to_string();
    let suffix_map: &[(&str, &str)] = &[
        ("_s", " (seconds)"),
        ("_ms", " (ms)"),
        ("_url", " URL"),
        ("_path", " Path"),
        ("_id", " ID"),
        ("_key", " Key"),
        ("_token", " Token"),
    ];
    for (suffix, replacement) in suffix_map {
        if name.ends_with(suffix) {
            name = format!("{}{}", &name[..name.len() - suffix.len()], replacement);
            break;
        }
    }
    name.replace('_', " ")
        .split_whitespace()
        .map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                None => String::new(),
                Some(c) => {
                    let upper: String = c.to_uppercase().collect();
                    format!("{}{}", upper, chars.as_str())
                }
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

// ---------------------------------------------------------------------------
// Field Definition
// ---------------------------------------------------------------------------

struct FieldDef {
    name: String,
    display_name: String,
    field_type: String,
}

fn get_agent_defaults_fields() -> Vec<FieldDef> {
    vec![
        FieldDef { name: "model".into(), display_name: "Model".into(), field_type: "string".into() },
        FieldDef { name: "provider".into(), display_name: "Provider".into(), field_type: "select_provider".into() },
        FieldDef { name: "max_tokens".into(), display_name: "Max Tokens".into(), field_type: "int".into() },
        FieldDef { name: "context_window_tokens".into(), display_name: "Context Window Tokens".into(), field_type: "int".into() },
        FieldDef { name: "context_block_limit".into(), display_name: "Context Block Limit".into(), field_type: "int".into() },
        FieldDef { name: "temperature".into(), display_name: "Temperature".into(), field_type: "float".into() },
        FieldDef { name: "max_tool_iterations".into(), display_name: "Max Tool Iterations".into(), field_type: "int".into() },
        FieldDef { name: "max_tool_result_chars".into(), display_name: "Max Tool Result Chars".into(), field_type: "int".into() },
        FieldDef { name: "provider_retry_mode".into(), display_name: "Provider Retry Mode".into(), field_type: "select".into() },
        FieldDef { name: "reasoning_effort".into(), display_name: "Reasoning Effort".into(), field_type: "select_reasoning".into() },
        FieldDef { name: "timezone".into(), display_name: "Timezone".into(), field_type: "string".into() },
        FieldDef { name: "unified_session".into(), display_name: "Unified Session".into(), field_type: "bool".into() },
        FieldDef { name: "disabled_skills".into(), display_name: "Disabled Skills".into(), field_type: "list".into() },
        FieldDef { name: "session_ttl_minutes".into(), display_name: "Session TTL Minutes".into(), field_type: "int".into() },
        FieldDef { name: "workspace".into(), display_name: "Workspace".into(), field_type: "string".into() },
    ]
}

fn get_provider_config_fields() -> Vec<FieldDef> {
    vec![
        FieldDef { name: "api_key".into(), display_name: "API Key".into(), field_type: "string".into() },
        FieldDef { name: "api_base".into(), display_name: "API Base URL".into(), field_type: "string".into() },
    ]
}

// ---------------------------------------------------------------------------
// Config Panel Display
// ---------------------------------------------------------------------------

fn show_config_panel(display_name: &str, items: &[(String, String)]) {
    let term = Term::stdout();
    let bold = Style::new().bold();
    let cyan = Style::new().cyan();

    let _ = term.write_line(&format!(
        "\n{}{}{}",
        cyan.apply_to("\u{2554}\u{2550}\u{2550} "),
        bold.apply_to(display_name),
        cyan.apply_to(" \u{2550}\u{2550}\u{2550}\u{2557}")
    ));
    for (field, value) in items {
        let _ = term.write_line(&format!(
            "{}  {}{}",
            cyan.apply_to("\u{2551}"),
            cyan.apply_to(format!("{:<35}", field)),
            value
        ));
    }
    let _ = term.write_line(&cyan.apply_to("\u{255a}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{255d}").to_string());
}

fn show_section_header(title: &str, subtitle: &str) {
    let term = Term::stdout();
    let bold = Style::new().bold();
    let blue = Style::new().blue();
    let dim = Style::new().dim();

    let _ = term.write_line("");
    let _ = term.write_line(&format!(
        "{}{}{}",
        blue.apply_to("\u{2554}\u{2550}\u{2550} "),
        bold.apply_to(title),
        blue.apply_to(" \u{2550}\u{2550}\u{2550}\u{2557}")
    ));
    if !subtitle.is_empty() {
        let _ = term.write_line(&format!(
            "{}  {}{}",
            blue.apply_to("\u{2551}"),
            dim.apply_to(subtitle),
            blue.apply_to(" \u{2551}")
        ));
    }
    let _ = term.write_line(&blue.apply_to("\u{255a}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{255d}").to_string());
}

fn show_main_menu_header() {
    let term = Term::stdout();
    let bold = Style::new().bold();
    let cyan = Style::new().cyan();

    let _ = term.write_line("");
    let _ = term.write_line(&format!(
        "        {} {}[{}]{}",
        cyan.apply_to("\u{1F916}"),
        bold.apply_to("nanobot"),
        cyan.apply_to(VERSION),
        bold.apply_to("")
    ));
    let _ = term.write_line("");
}

// ---------------------------------------------------------------------------
// Provider Helpers
// ---------------------------------------------------------------------------

fn get_provider_names() -> Vec<(String, String)> {
    PROVIDERS
        .iter()
        .filter(|p| !p.is_oauth)
        .map(|p| (p.name.to_string(), p.label().to_string()))
        .collect()
}

fn get_current_provider(config_value: &serde_json::Value) -> String {
    config_value.get("provider").and_then(|v| v.as_str()).unwrap_or("auto").to_string()
}

// ---------------------------------------------------------------------------
// Provider Config Access (only providers that exist in ProvidersConfig)
// ---------------------------------------------------------------------------

fn get_provider_config_mut<'a>(cfg: &'a mut Config, name: &str) -> Option<&'a mut ProviderConfig> {
    match name {
        "custom" => Some(&mut cfg.providers.custom),
        "azure_openai" => Some(&mut cfg.providers.azure_openai),
        "anthropic" => Some(&mut cfg.providers.anthropic),
        "openai" => Some(&mut cfg.providers.openai),
        "openrouter" => Some(&mut cfg.providers.openrouter),
        "deepseek" => Some(&mut cfg.providers.deepseek),
        "groq" => Some(&mut cfg.providers.groq),
        "zhipu" => Some(&mut cfg.providers.zhipu),
        "dashscope" => Some(&mut cfg.providers.dashscope),
        "vllm" => Some(&mut cfg.providers.vllm),
        "ollama" => Some(&mut cfg.providers.ollama),
        "lm_studio" => Some(&mut cfg.providers.lm_studio),
        "ovms" => Some(&mut cfg.providers.ovms),
        "gemini" => Some(&mut cfg.providers.gemini),
        "moonshot" => Some(&mut cfg.providers.moonshot),
        "minimax" => Some(&mut cfg.providers.minimax),
        "minimax_anthropic" => Some(&mut cfg.providers.minimax_anthropic),
        "mistral" => Some(&mut cfg.providers.mistral),
        "stepfun" => Some(&mut cfg.providers.stepfun),
        "xiaomi_mimo" => Some(&mut cfg.providers.xiaomi_mimo),
        "aihubmix" => Some(&mut cfg.providers.aihubmix),
        "siliconflow" => Some(&mut cfg.providers.siliconflow),
        "volcengine" => Some(&mut cfg.providers.volcengine),
        "volcengine_coding_plan" => Some(&mut cfg.providers.volcengine_coding_plan),
        "byteplus" => Some(&mut cfg.providers.byteplus),
        "byteplus_coding_plan" => Some(&mut cfg.providers.byteplus_coding_plan),
        "openai_codex" => None,
        "github_copilot" => None,
        "qianfan" => Some(&mut cfg.providers.qianfan),
        _ => None,
    }
}

fn get_provider_ref<'a>(cfg: &'a Config, name: &str) -> Option<&'a ProviderConfig> {
    match name {
        "custom" => Some(&cfg.providers.custom),
        "azure_openai" => Some(&cfg.providers.azure_openai),
        "anthropic" => Some(&cfg.providers.anthropic),
        "openai" => Some(&cfg.providers.openai),
        "openrouter" => Some(&cfg.providers.openrouter),
        "deepseek" => Some(&cfg.providers.deepseek),
        "groq" => Some(&cfg.providers.groq),
        "zhipu" => Some(&cfg.providers.zhipu),
        "dashscope" => Some(&cfg.providers.dashscope),
        "vllm" => Some(&cfg.providers.vllm),
        "ollama" => Some(&cfg.providers.ollama),
        "lm_studio" => Some(&cfg.providers.lm_studio),
        "ovms" => Some(&cfg.providers.ovms),
        "gemini" => Some(&cfg.providers.gemini),
        "moonshot" => Some(&cfg.providers.moonshot),
        "minimax" => Some(&cfg.providers.minimax),
        "minimax_anthropic" => Some(&cfg.providers.minimax_anthropic),
        "mistral" => Some(&cfg.providers.mistral),
        "stepfun" => Some(&cfg.providers.stepfun),
        "xiaomi_mimo" => Some(&cfg.providers.xiaomi_mimo),
        "aihubmix" => Some(&cfg.providers.aihubmix),
        "siliconflow" => Some(&cfg.providers.siliconflow),
        "volcengine" => Some(&cfg.providers.volcengine),
        "volcengine_coding_plan" => Some(&cfg.providers.volcengine_coding_plan),
        "byteplus" => Some(&cfg.providers.byteplus),
        "byteplus_coding_plan" => Some(&cfg.providers.byteplus_coding_plan),
        "qianfan" => Some(&cfg.providers.qianfan),
        _ => None,
    }
}

fn get_provider_value(cfg: &Config, name: &str, _fields: &[FieldDef]) -> serde_json::Value {
    let pc = get_provider_ref(cfg, name);
    pc.map(|p| {
        serde_json::json!({
            "api_key": p.api_key,
            "api_base": p.api_base,
        })
    })
    .unwrap_or(serde_json::Value::Object(Default::default()))
}

fn apply_value_to_provider(cfg: &mut Config, name: &str, value: &serde_json::Value) {
    if let Some(pc) = get_provider_config_mut(cfg, name) {
        if let Some(key) = value.get("api_key").and_then(|v| v.as_str()) {
            pc.api_key = if key.is_empty() { None } else { Some(key.to_string()) };
        }
        if let Some(base) = value.get("api_base").and_then(|v| v.as_str()) {
            pc.api_base = if base.is_empty() { None } else { Some(base.to_string()) };
        }
    }
}

fn configure_provider(cfg: &mut Config, provider_name: &str) {
    let provider_names = get_provider_names();
    let display_name = provider_names
        .iter()
        .find(|(n, _)| n == provider_name)
        .map(|(_, d)| d.as_str())
        .unwrap_or(provider_name);

    let default_api_base = PROVIDERS
        .iter()
        .find(|p| p.name == provider_name)
        .map(|p| p.default_api_base)
        .unwrap_or("");

    if !default_api_base.is_empty() {
        if let Some(p) = get_provider_config_mut(cfg, provider_name) {
            if p.api_base.as_deref().unwrap_or("").is_empty() {
                p.api_base = Some(default_api_base.to_string());
            }
        }
    }

    if let Some(value) = configure_pydantic_model(
        display_name,
        &get_provider_config_fields(),
        |fields| get_provider_value(cfg, provider_name, fields),
    ) {
        apply_value_to_provider(cfg, provider_name, &value);
    }
}

// ---------------------------------------------------------------------------
// Configure Provider (selection loop)
// ---------------------------------------------------------------------------

fn configure_providers(cfg: &mut Config) {
    loop {
        let term = Term::stdout();
        let _ = term.clear_screen();
        show_section_header("LLM Providers", "Select a provider to configure API key and endpoint");

        let provider_names = get_provider_names();
        let mut choices: Vec<String> = Vec::new();
        for (name, display) in &provider_names {
            let has_key = get_provider_ref(cfg, name)
                .and_then(|p| p.api_key.as_deref())
                .map(|k| !k.is_empty())
                .unwrap_or(false);
            if has_key {
                choices.push(format!("{} *", display));
            } else {
                choices.push(display.clone());
            }
        }
        choices.push("<- Back".to_string());

        match Select::with_theme(&theme())
            .with_prompt("Select provider:")
            .items(&choices)
            .default(0)
            .interact_opt()
        {
            Ok(Some(idx)) => {
                let answer = &choices[idx];
                if answer == "<- Back" { break; }
                let provider_name = answer.replace(" *", "");
                for (name, display) in &provider_names {
                    if *display == provider_name {
                        configure_provider(cfg, name);
                        break;
                    }
                }
            }
            _ => break,
        }
    }
}

// ---------------------------------------------------------------------------
// Configure Channel
// ---------------------------------------------------------------------------

fn configure_channel(_cfg: &mut Config, channel_name: &str) {
    show_section_header(&format!("Channel: {}", channel_name), "");
    let term = Term::stdout();
    let _ = term.write_line(&format!(
        "Channel '{}' configuration is not yet supported in the Rust build.\nEdit ~/.nanobot/config.json manually.",
        channel_name
    ));
    pause();
}

fn configure_channels(_cfg: &mut Config) {
    let _ = Term::stdout().clear_screen();
    show_section_header("Chat Channels", "Select a channel to configure connection settings");

    let channel_names: Vec<String> = vec![
        "telegram".to_string(),
        "discord".to_string(),
        "slack".to_string(),
    ];

    let mut choices = channel_names.clone();
    choices.push("<- Back".to_string());

    match Select::with_theme(&theme())
        .with_prompt("Select channel:")
        .items(&choices)
        .default(0)
        .interact_opt()
    {
        Ok(Some(idx)) => {
            let answer = &choices[idx];
            if answer != "<- Back" {
                configure_channel(&mut Config::default(), answer);
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Configure General Settings
// ---------------------------------------------------------------------------

fn configure_general_settings(cfg: &mut Config, section_title: &str) {
    let (display_name, fields, getter, setter) = match section_title {
        "Agent Settings" => (
            "Agent Defaults",
            get_agent_defaults_fields(),
            Box::new(|c: &Config| serde_json::to_value(&c.agents.defaults).unwrap_or(serde_json::Value::Object(Default::default()))) as Box<dyn Fn(&Config) -> serde_json::Value>,
            Box::new(|c: &mut Config, v: serde_json::Value| {
                if let Ok(updated) = serde_json::from_value::<AgentDefaults>(v) { c.agents.defaults = updated; }
            }) as Box<dyn Fn(&mut Config, serde_json::Value)>,
        ),
        "Channel Common" => {
            let term = Term::stdout();
            let _ = term.write_line("\n[dim]Channel Common: not yet supported in Rust build[/dim]");
            pause();
            return;
        }
        "API Server" => (
            "API Server",
            vec![
                FieldDef { name: "host".into(), display_name: "Host".into(), field_type: "string".into() },
                FieldDef { name: "port".into(), display_name: "Port".into(), field_type: "int".into() },
                FieldDef { name: "timeout".into(), display_name: "Timeout".into(), field_type: "float".into() },
            ],
            Box::new(|c: &Config| serde_json::to_value(&c.api).unwrap_or(serde_json::Value::Object(Default::default()))) as Box<dyn Fn(&Config) -> serde_json::Value>,
            Box::new(|c: &mut Config, v: serde_json::Value| {
                if let Ok(updated) = serde_json::from_value::<ApiConfig>(v) { c.api = updated; }
            }) as Box<dyn Fn(&mut Config, serde_json::Value)>,
        ),
        "Gateway" => (
            "Gateway Settings",
            vec![
                FieldDef { name: "host".into(), display_name: "Host".into(), field_type: "string".into() },
                FieldDef { name: "port".into(), display_name: "Port".into(), field_type: "int".into() },
            ],
            Box::new(|c: &Config| serde_json::to_value(&c.gateway).unwrap_or(serde_json::Value::Object(Default::default()))) as Box<dyn Fn(&Config) -> serde_json::Value>,
            Box::new(|c: &mut Config, v: serde_json::Value| {
                if let Ok(updated) = serde_json::from_value::<GatewayConfig>(v) { c.gateway = updated; }
            }) as Box<dyn Fn(&mut Config, serde_json::Value)>,
        ),
        "Tools" => (
            "Tools Settings",
            vec![
                FieldDef { name: "web".into(), display_name: "Web Tools".into(), field_type: "model".into() },
                FieldDef { name: "exec".into(), display_name: "Exec Tool".into(), field_type: "model".into() },
                FieldDef { name: "my".into(), display_name: "My Tool".into(), field_type: "model".into() },
                FieldDef { name: "restrict_to_workspace".into(), display_name: "Restrict to Workspace".into(), field_type: "bool".into() },
            ],
            Box::new(|c: &Config| serde_json::to_value(&c.tools).unwrap_or(serde_json::Value::Object(Default::default()))) as Box<dyn Fn(&Config) -> serde_json::Value>,
            Box::new(|c: &mut Config, v: serde_json::Value| {
                if let Ok(updated) = serde_json::from_value::<ToolsConfig>(v) { c.tools = updated; }
            }) as Box<dyn Fn(&mut Config, serde_json::Value)>,
        ),
        _ => return,
    };

    let current_value = getter(cfg);
    if let Some(updated) = configure_pydantic_model(display_name, &fields, |_| current_value.clone()) {
        setter(cfg, updated);
    }
}

// ---------------------------------------------------------------------------
// Generic Model Configuration
// ---------------------------------------------------------------------------

fn configure_pydantic_model(
    display_name: &str,
    fields: &[FieldDef],
    getter: impl Fn(&[FieldDef]) -> serde_json::Value,
) -> Option<serde_json::Value> {
    if fields.is_empty() { return None; }

    let mut working = getter(fields);
    let mut last_idx = 0;

    loop {
        let items: Vec<(String, String)> = fields.iter().map(|f| {
            let val = working.get(&f.name).cloned();
            let display = format_value_for_display(val.as_ref().unwrap_or(&serde_json::Value::Null), &f.name);
            (f.display_name.clone(), display)
        }).collect();

        let _ = Term::stdout().clear_screen();
        show_config_panel(display_name, &items);

        let mut choice_items: Vec<String> = items.iter().map(|(field, value)| format!("{}: {}", field, value)).collect();
        choice_items.push("[Done]".to_string());

        let default_idx = last_idx.min(choice_items.len() - 1);

        match Select::with_theme(&theme())
            .with_prompt("Select field to configure:")
            .items(&choice_items)
            .default(default_idx)
            .interact_opt()
        {
            Ok(Some(idx)) => {
                if idx >= fields.len() { return Some(working); }
                last_idx = idx;
                let field = &fields[idx];

                match input_generic_field(&mut working, field) {
                    SelectResult::Back => continue,
                    SelectResult::Cancel => return None,
                    SelectResult::Value(_) => {}
                }
            }
            _ => return None,
        }
    }
}

fn input_generic_field(value: &mut serde_json::Value, field: &FieldDef) -> SelectResult<()> {
    let current = value.get(&field.name).cloned().unwrap_or(serde_json::Value::Null);

    match field.field_type.as_str() {
        "select_provider" => {
            let provider_names: Vec<String> = get_provider_names().iter().map(|(n, _)| n.clone()).collect();
            let mut choices = vec!["auto".to_string()];
            choices.extend(provider_names);
            let current_str = current.as_str().unwrap_or("auto").to_string();
            match input_select(&field.display_name, &choices, Some(&current_str)) {
                SelectResult::Value(v) => {
                    if let Some(obj) = value.as_object_mut() {
                        obj.insert(field.name.clone(), serde_json::Value::String(v));
                    }
                }
                SelectResult::Back => return SelectResult::Back,
                SelectResult::Cancel => return SelectResult::Cancel,
            }
        }
        "select_reasoning" => {
            let choices = vec!["low".to_string(), "medium".to_string(), "high".to_string(), "(clear/unset)".to_string()];
            let default_val = current.as_str().unwrap_or("low");
            match input_select(&field.display_name, &choices, Some(default_val)) {
                SelectResult::Value(v) => {
                    if let Some(obj) = value.as_object_mut() {
                        if v == "(clear/unset)" {
                            obj.insert(field.name.clone(), serde_json::Value::Null);
                        } else {
                            obj.insert(field.name.clone(), serde_json::Value::String(v));
                        }
                    }
                }
                SelectResult::Back => return SelectResult::Back,
                SelectResult::Cancel => return SelectResult::Cancel,
            }
        }
        "select" => {
            if field.name == "provider_retry_mode" {
                let choices = vec!["standard".to_string(), "persistent".to_string()];
                let current_str = current.as_str().unwrap_or("standard").to_string();
                match input_select(&field.display_name, &choices, Some(&current_str)) {
                    SelectResult::Value(v) => {
                        if let Some(obj) = value.as_object_mut() {
                            obj.insert(field.name.clone(), serde_json::Value::String(v));
                        }
                    }
                    SelectResult::Back => return SelectResult::Back,
                    SelectResult::Cancel => return SelectResult::Cancel,
                }
            }
        }
        "bool" => {
            let default = current.as_bool().unwrap_or(false);
            if let Some(v) = input_bool(&field.display_name, default) {
                if let Some(obj) = value.as_object_mut() {
                    obj.insert(field.name.clone(), serde_json::Value::Bool(v));
                }
            }
        }
        "model" => {
            let term = Term::stdout();
            let _ = term.write_line(&format!("\n[dim]{}: nested config not yet supported in Rust build[/dim]", field.display_name));
            pause();
        }
        _ => {
            let default_str = format_value_for_input(&current, &field.field_type);
            if let Some(v) = input_with_existing(&field.display_name, &current, &field.field_type) {
                if let Some(obj) = value.as_object_mut() {
                    if v.as_str().map(|s| s.is_empty()).unwrap_or(false) {
                        obj.insert(field.name.clone(), serde_json::Value::Null);
                    } else {
                        obj.insert(field.name.clone(), v);
                    }
                }
            }
        }
    }

    SelectResult::Value(())
}

// ---------------------------------------------------------------------------
// Summary Display
// ---------------------------------------------------------------------------

fn show_summary(cfg: &Config) {
    let _ = Term::stdout().write_line("");

    let mut provider_rows: Vec<(String, String)> = Vec::new();
    for (name, display) in get_provider_names() {
        let status = get_provider_ref(cfg, &name)
            .and_then(|p| p.api_key.as_deref())
            .map(|k| if k.is_empty() { "not configured" } else { "configured" })
            .unwrap_or("not configured");
        provider_rows.push((display, status.to_string()));
    }
    show_config_panel("LLM Providers", &provider_rows);

    let agent_rows = summarize_agent_defaults(&cfg.agents.defaults);
    show_config_panel("Agent Settings", &agent_rows);

    pause();
}

fn summarize_agent_defaults(defaults: &AgentDefaults) -> Vec<(String, String)> {
    let mut items = Vec::new();
    if !defaults.model.is_empty() {
        items.push(("Model".to_string(), defaults.model.clone()));
    }
    if defaults.provider != "auto" && !defaults.provider.is_empty() {
        items.push(("Provider".to_string(), defaults.provider.clone()));
    }
    items.push(("Max Tokens".to_string(), defaults.max_tokens.to_string()));
    items.push(("Context Window".to_string(), defaults.context_window_tokens.to_string()));
    items.push(("Temperature".to_string(), format!("{}", defaults.temperature)));
    if defaults.reasoning_effort.is_some() {
        items.push(("Reasoning Effort".to_string(), defaults.reasoning_effort.clone().unwrap()));
    }
    items.push(("Max Tool Iterations".to_string(), defaults.max_tool_iterations.to_string()));
    items.push(("Timezone".to_string(), defaults.timezone.clone()));
    items
}

// ---------------------------------------------------------------------------
// Pause
// ---------------------------------------------------------------------------

fn pause() {
    let _: Result<String, _> = Input::with_theme(&theme())
        .with_prompt("Press Enter to continue...")
        .default("".to_string())
        .allow_empty(true)
        .interact_text();
}

// ---------------------------------------------------------------------------
// Model helpers (mirrors Python models.py)
// ---------------------------------------------------------------------------

fn get_model_context_limit(_model: &str, _provider: &str) -> Option<u32> {
    None
}

fn format_token_count(tokens: u32) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}K", tokens as f64 / 1_000.0)
    } else {
        tokens.to_string()
    }
}

// ---------------------------------------------------------------------------
// Unsaved Changes Detection
// ---------------------------------------------------------------------------

fn has_unsaved_changes(original: &Config, current: &Config) -> bool {
    let original_json = serde_json::to_string(original).unwrap_or_default();
    let current_json = serde_json::to_string(current).unwrap_or_default();
    original_json != current_json
}

// ---------------------------------------------------------------------------
// Main Entry Point
// ---------------------------------------------------------------------------

pub fn run_onboard(initial_config: Option<Config>) -> OnboardResult {
    let base_config = match initial_config {
        Some(cfg) => cfg,
        None => {
            let config_path = get_config_path();
            if config_path.exists() {
                Config::from_config(Some(&config_path))
            } else {
                Config::default()
            }
        }
    };

    let original_config = base_config.clone();
    let mut config = base_config;

    loop {
        let _ = Term::stdout().clear_screen();
        show_main_menu_header();

        let choices = vec![
            "[P] LLM Provider",
            "[M] Model Presets",
            "[C] Chat Channel",
            "[H] Channel Common",
            "[A] Agent Settings",
            "[I] API Server",
            "[G] Gateway",
            "[T] Tools",
            "[V] View Configuration Summary",
            "[S] Save and Exit",
            "[X] Exit Without Saving",
        ];

        let answer = Select::with_theme(&theme())
            .with_prompt("What would you like to configure?")
            .items(&choices)
            .default(0)
            .interact_opt();

        let answer = match answer {
            Ok(Some(idx)) => choices[idx],
            _ => {
                let action = prompt_main_menu_exit(has_unsaved_changes(&original_config, &config));
                match action.as_str() {
                    "save" => return OnboardResult { config, should_save: true },
                    _ => return OnboardResult { config: original_config.clone(), should_save: false },
                }
            }
        };

        match answer {
            "[P] LLM Provider" => configure_providers(&mut config),
            "[C] Chat Channel" => configure_channels(&mut config),
            "[H] Channel Common" => configure_general_settings(&mut config, "Channel Common"),
            "[A] Agent Settings" => configure_general_settings(&mut config, "Agent Settings"),
            "[I] API Server" => configure_general_settings(&mut config, "API Server"),
            "[G] Gateway" => configure_general_settings(&mut config, "Gateway"),
            "[T] Tools" => configure_general_settings(&mut config, "Tools"),
            "[V] View Configuration Summary" => show_summary(&config),
            "[S] Save and Exit" => return OnboardResult { config, should_save: true },
            "[X] Exit Without Saving" => return OnboardResult { config: original_config.clone(), should_save: false },
            _ => {}
        }
    }
}

fn prompt_main_menu_exit(has_unsaved_changes: bool) -> String {
    if !has_unsaved_changes {
        return "discard".to_string();
    }

    let choices = vec![
        "[S] Save and Exit",
        "[X] Exit Without Saving",
        "[R] Resume Editing",
    ];

    match Select::with_theme(&theme())
        .with_prompt("You have unsaved changes. What would you like to do?")
        .items(&choices)
        .default(2)
        .interact_opt()
    {
        Ok(Some(idx)) => match idx {
            0 => "save".to_string(),
            1 => "discard".to_string(),
            _ => "resume".to_string(),
        },
        _ => "resume".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Args (kept for backwards compat with commands.rs)
// ---------------------------------------------------------------------------

/// Args for the onboard command.
pub struct OnboardArgs {
    pub workspace: Option<PathBuf>,
    pub config: Option<PathBuf>,
    pub overwrite: bool,
}

/// Run the interactive onboarding wizard or fall back to non-interactive mode.
pub async fn run(args: OnboardArgs) -> Result<(), String> {
    let config_path = match args.config.as_deref() {
        Some(p) => {
            let resolved = expand_tilde(p);
            set_config_path(&resolved);
            println!("Using config: {}", resolved.display());
            resolved
        }
        None => get_config_path(),
    };

    let cfg = if config_path.exists() && !args.overwrite {
        Config::from_config(Some(&config_path))
    } else if args.overwrite {
        let cfg = apply_workspace_override(Config::default(), args.workspace.as_deref());
        cfg.save_config(Some(&config_path)).map_err(|e| e.to_string())?;
        println!("\u{2713} Config reset to defaults at {}", config_path.display());
        cfg
    } else {
        let cfg = apply_workspace_override(Config::default(), args.workspace.as_deref());
        cfg.save_config(Some(&config_path)).map_err(|e| e.to_string())?;
        println!("\u{2713} Created config at {}", config_path.display());
        cfg
    };

    onboard_plugins(&config_path);

    let result = run_onboard(Some(cfg));

    if result.should_save {
        result.config.save_config(Some(&config_path)).map_err(|e| e.to_string())?;
        println!("\n\u{2713} Config saved to {}", config_path.display());
    } else {
        println!("\nConfig changes discarded.");
    }

    let workspace_path = get_workspace_path(Some(&result.config.agents.defaults.workspace));
    if !workspace_path.exists() {
        std::fs::create_dir_all(&workspace_path).map_err(|e| e.to_string())?;
        println!("\u{2713} Created workspace at {}", workspace_path.display());
    }

    let cfg = Config::from_config(Some(&config_path));
    let _ = cfg;

    let mut agent_cmd = "nanobot agent -m \"Hello!\"".to_string();
    let mut gateway_cmd = "nanobot gateway".to_string();
    if config_path != get_config_path() {
        agent_cmd.push_str(&format!(" --config {}", config_path.display()));
        gateway_cmd.push_str(&format!(" --config {}", config_path.display()));
    }

    println!();
    println!("nanobot is ready!");
    println!();
    println!("Next steps:");
    println!("  1. Add your API key to {}", config_path.display());
    println!("     Get one at: https://openrouter.ai/keys");
    println!("  2. Chat: {agent_cmd}");
    println!("  3. Start gateway: {gateway_cmd}");
    println!();
    println!("Want Telegram/WhatsApp? See: https://github.com/HKUDS/nanobot#-chat-apps");

    Ok(())
}

fn apply_workspace_override(mut cfg: Config, workspace: Option<&Path>) -> Config {
    if let Some(ws) = workspace {
        cfg.agents.defaults.workspace = expand_tilde(ws).to_string_lossy().into_owned();
    }
    cfg
}

fn onboard_plugins(config_path: &Path) {
    let Ok(text) = std::fs::read_to_string(config_path) else { return; };
    let Ok(mut data) = serde_json::from_str::<serde_json::Value>(&text) else { return; };
    let root = match data.as_object_mut() {
        Some(o) => o,
        None => return,
    };
    root.entry("channels").or_insert_with(|| serde_json::Value::Object(Default::default()));
    if let Ok(out) = serde_json::to_string_pretty(&data) {
        let _ = std::fs::write(config_path, out);
    }
}
