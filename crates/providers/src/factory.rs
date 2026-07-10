//! Build a concrete [`LLMProvider`] from a [`Backend`] variant and config.
//!
//! Mirrors Python's ``nanobot.providers.factory._make_provider_core`` —
//! the CLI layer only knows *which backend* to use; the actual construction
//! logic lives here inside the providers crate.

use std::env;
use std::sync::Arc;

use serde_json::{Map, Value};

use crate::GenerationSettings;
use crate::anthropic_provider::{AnthropicConfig, AnthropicProvider};
use crate::azure_openai_provider::{AzureOpenAIConfig, AzureOpenAIProvider};
use crate::base::LLMProvider;
use crate::bedrock_provider::{BedrockConfig, BedrockProvider};
use crate::github_copilot_provider::GitHubCopilotProvider;
use crate::openai_codex_provider::{OpenAICodexConfig, OpenAICodexProvider};
use crate::openai_compat_provider::{OpenAICompatConfig, OpenAICompatProvider};
use crate::registry::{Backend, ProviderSpec, find_by_model, find_by_name};
use config::schema::{Config, ProviderConfig};
use config::{get_config_path, paths::is_default_workspace, set_config_path};

/// Snapshot of a built provider chain, including fallback windows and config signature.
#[derive(Clone)]
pub struct ProviderSnapshot {
    pub provider: Arc<dyn LLMProvider>,
    pub model: String,
    pub context_window_tokens: u32,
    pub signature: Vec<Value>,
}

/// Compute a signature for a single preset (model + provider + credentials + settings).
fn preset_signature(
    cfg: &Config,
    model: &str,
    provider_config: Option<&ProviderConfig>,
    spec: Option<&ProviderSpec>,
    max_tokens: u32,
    temperature: f32,
    reasoning_effort: Option<&str>,
    context_window_tokens: u32,
) -> Vec<Value> {
    let provider_name = spec.map(|s| s.name.to_string()).unwrap_or_default();
    let api_key = provider_config
        .and_then(|p| p.api_key.as_deref())
        .unwrap_or("")
        .to_string();
    let api_base = provider_config
        .and_then(|p| p.api_base.as_deref())
        .unwrap_or("")
        .to_string();
    let extra_headers = provider_config
        .and_then(|p| p.extra_headers.as_ref())
        .map(|h| {
            let m: Map<String, Value> = h
                .iter()
                .map(|(k, v)| (k.clone(), Value::String(v.clone())))
                .collect();
            Value::Object(m)
        })
        .unwrap_or(Value::Null);
    let extra_body = Value::Null;
    let region = Value::Null;
    let profile = Value::Null;

    vec![
        Value::String(model.to_string()),
        Value::String(provider_name),
        Value::String(api_key),
        Value::String(api_base),
        extra_headers,
        extra_body,
        region,
        profile,
        Value::Number(serde_json::Number::from(max_tokens)),
        Value::Number(
            serde_json::Number::from_f64(temperature as f64)
                .unwrap_or_else(|| serde_json::Number::from_f64(0.0).unwrap()),
        ),
        Value::String(reasoning_effort.unwrap_or("").to_string()),
        Value::Number(serde_json::Number::from(context_window_tokens)),
    ]
}

/// Return the config fields that affect the active provider chain.
pub fn provider_signature(cfg: &Config) -> Vec<Value> {
    let defaults = &cfg.agents.defaults;
    let model = &defaults.model;
    let spec = resolve_spec(cfg);
    let provider_config = provider_config_for(cfg, spec);
    let max_tokens = defaults.max_tokens;
    let temperature = defaults.temperature;
    let reasoning_effort = defaults.reasoning_effort.as_deref();
    let context_window_tokens = defaults.context_window_tokens;

    let mut sig = preset_signature(
        cfg,
        model,
        provider_config,
        spec,
        max_tokens,
        temperature,
        reasoning_effort,
        context_window_tokens,
    );

    sig.push(Value::Array(vec![]));

    sig
}

/// Build a provider and return a ProviderSnapshot with fallback window calculation.
pub fn build_provider_snapshot(cfg: &Config) -> Result<ProviderSnapshot, String> {
    let defaults = &cfg.agents.defaults;
    let model = defaults.model.clone();
    let context_window = defaults.context_window_tokens;

    let provider = make_provider(cfg)?;
    let signature = provider_signature(cfg);

    Ok(ProviderSnapshot {
        provider,
        model,
        context_window_tokens: context_window,
        signature,
    })
}

/// Load config from path and build a ProviderSnapshot.
pub fn load_provider_snapshot(config_path: Option<&str>) -> Result<ProviderSnapshot, String> {
    let cfg = Config::from_config(config_path.map(|p| std::path::Path::new(p)));
    build_provider_snapshot(&cfg)
}

/// High-level config for building any provider.
#[derive(Debug, Clone, Default)]
pub struct ProviderBuildConfig {
    /// Model name (may contain provider prefix).
    pub model: String,
    /// API key (may be empty for OAuth/direct providers).
    pub api_key: Option<String>,
    /// API base URL (may be empty).
    pub api_base: Option<String>,
    /// Extra headers from config.
    pub extra_headers: Option<Map<String, serde_json::Value>>,
    /// Extra body fields from config.
    pub extra_body: Option<Map<String, serde_json::Value>>,
    /// AWS region (bedrock only).
    pub region: Option<String>,
    /// AWS profile (bedrock only).
    pub profile: Option<String>,
}

/// Build a provider from a [`Backend`] and config.
pub fn build_provider(
    backend: Backend,
    cfg: ProviderBuildConfig,
) -> Result<Arc<dyn LLMProvider>, String> {
    match backend {
        Backend::OpenAICodex => build_openai_codex(&cfg),
        Backend::AzureOpenAI => build_azure_openai(&cfg),
        Backend::GitHubCopilot => build_github_copilot(&cfg),
        Backend::Bedrock => build_bedrock(&cfg),
        Backend::Anthropic => build_anthropic(&cfg),
        Backend::OpenAICompat => build_openai_compat(&cfg),
    }
}

/// Auto-detect the backend from the model name and config.
pub fn detect_backend_and_build(
    model: &str,
    cfg: ProviderBuildConfig,
) -> Result<Arc<dyn LLMProvider>, String> {
    let spec = find_by_model_or_prefix(model);
    let backend = spec.map(|s| s.backend).unwrap_or(Backend::OpenAICompat);
    build_provider(backend, cfg)
}

fn find_by_model_or_prefix(model: &str) -> Option<&'static ProviderSpec> {
    // Check for explicit "provider/" prefix first
    if let Some(pos) = model.find('/') {
        let prefix = &model[..pos];
        if let Some(spec) = find_by_name(prefix) {
            return Some(spec);
        }
    }
    // Fall back to keyword matching
    crate::registry::find_by_model(model)
}

fn build_openai_codex(cfg: &ProviderBuildConfig) -> Result<Arc<dyn LLMProvider>, String> {
    let model = cfg.model.clone();
    let codex_cfg = OpenAICodexConfig::new(model);
    let p = OpenAICodexProvider::new(codex_cfg)
        .map_err(|e| format!("failed to init codex provider: {e}"))?;
    Ok(Arc::new(p))
}

fn build_azure_openai(cfg: &ProviderBuildConfig) -> Result<Arc<dyn LLMProvider>, String> {
    let api_key = cfg
        .api_key
        .clone()
        .ok_or_else(|| "Azure OpenAI api_key is required".to_string())?;
    let api_base = cfg
        .api_base
        .clone()
        .ok_or_else(|| "Azure OpenAI api_base is required".to_string())?;
    let deployment = cfg.model.clone();
    let mut azure_cfg = AzureOpenAIConfig::new(api_base, api_key, deployment);
    if let Some(extra) = &cfg.extra_headers {
        for (k, v) in extra {
            if let Some(v_str) = v.as_str() {
                azure_cfg = azure_cfg.with_extra_header(k, v_str);
            }
        }
    }
    let p = AzureOpenAIProvider::new(azure_cfg).map_err(|e| e.to_string())?;
    Ok(Arc::new(p))
}

fn build_github_copilot(cfg: &ProviderBuildConfig) -> Result<Arc<dyn LLMProvider>, String> {
    let model = if cfg.model.is_empty() {
        "github-copilot/gpt-4.1".to_string()
    } else {
        cfg.model.clone()
    };
    let p = GitHubCopilotProvider::new(model)
        .map_err(|e| format!("failed to init copilot provider: {e}"))?;
    Ok(Arc::new(p))
}

fn build_bedrock(cfg: &ProviderBuildConfig) -> Result<Arc<dyn LLMProvider>, String> {
    let model = if cfg.model.is_empty() {
        "bedrock/global.anthropic.claude-opus-4-7".to_string()
    } else {
        cfg.model.clone()
    };
    let mut bcfg = BedrockConfig::new(model);
    if let Some(key) = &cfg.api_key {
        bcfg = bcfg.with_api_key(key);
    }
    if let Some(region) = &cfg.region {
        bcfg = bcfg.with_region(region);
    }
    if let Some(base) = &cfg.api_base {
        bcfg = bcfg.with_api_base(base);
    }
    if let Some(extra) = &cfg.extra_body {
        bcfg = bcfg.with_extra_body(extra.clone());
    }
    let p =
        BedrockProvider::new(bcfg).map_err(|e| format!("failed to init bedrock provider: {e}"))?;
    Ok(Arc::new(p))
}

fn build_anthropic(cfg: &ProviderBuildConfig) -> Result<Arc<dyn LLMProvider>, String> {
    let model = cfg.model.clone();
    let mut acfg = AnthropicConfig::new(model);
    if let Some(key) = &cfg.api_key {
        acfg = acfg.with_api_key(key);
    }
    if let Some(base) = &cfg.api_base {
        acfg = acfg.with_api_base(base);
    }
    if let Some(extra) = &cfg.extra_headers {
        for (k, v) in extra {
            if let Some(v_str) = v.as_str() {
                acfg = acfg.with_extra_header(k, v_str);
            }
        }
    }
    let p = AnthropicProvider::new(acfg);
    Ok(Arc::new(p))
}

fn build_openai_compat(cfg: &ProviderBuildConfig) -> Result<Arc<dyn LLMProvider>, String> {
    let model = &cfg.model;
    let mut ocfg = OpenAICompatConfig::new(model);
    if let Some(key) = &cfg.api_key {
        ocfg = ocfg.with_api_key(key);
    }
    if let Some(base) = &cfg.api_base {
        ocfg = ocfg.with_api_base(base);
    }
    // Try to find a matching spec by model name
    if let Some(spec) = find_by_model_or_prefix(model) {
        ocfg = ocfg.with_spec(spec);
    }
    if let Some(extra) = &cfg.extra_headers {
        for (k, v) in extra {
            if let Some(v_str) = v.as_str() {
                ocfg = ocfg.with_extra_header(k, v_str);
            }
        }
    }
    if let Some(extra) = &cfg.extra_body {
        ocfg = ocfg.with_extra_body(extra.clone());
    }
    let p = OpenAICompatProvider::new(ocfg);
    Ok(Arc::new(p))
}

/// Resolve region from environment variables (AWS_REGION, AWS_DEFAULT_REGION).
pub fn env_region() -> Option<String> {
    env::var("AWS_REGION")
        .ok()
        .or_else(|| env::var("AWS_DEFAULT_REGION").ok())
}

/// Resolve API key from environment.
pub fn env_api_key(var: &str) -> Option<String> {
    env::var(var).ok()
}

/// Resolve API base from environment.
pub fn env_api_base(var: &str) -> Option<String> {
    env::var(var).ok()
}

/// Build the right [`LLMProvider`] for the active config (Python
/// `_make_provider`).
pub fn make_provider(cfg: &Config) -> Result<Arc<dyn LLMProvider>, String> {
    let model = cfg.agents.defaults.model.clone();
    let provider_spec = resolve_spec(cfg);
    let provider_config = provider_config_for(cfg, provider_spec);
    let backend = provider_spec
        .map(|s| s.backend)
        .unwrap_or(Backend::OpenAICompat);

    // ---- credential validation (mirror Python checks) ----
    match backend {
        Backend::AzureOpenAI => {
            let ok = provider_config
                .map(|p| {
                    !p.api_key.as_deref().unwrap_or("").is_empty()
                        && !p.api_base.as_deref().unwrap_or("").is_empty()
                })
                .unwrap_or(false);
            if !ok {
                return Err("Azure OpenAI requires api_key and api_base. \
                     Set them in ~/.nanobot/config.json under providers.azureOpenai \
                     and use `model` for the deployment name."
                    .into());
            }
        }
        Backend::OpenAICompat if !model.starts_with("bedrock/") => {
            let has_key = provider_config
                .and_then(|p| p.api_key.as_deref())
                .map(|k| !k.is_empty())
                .unwrap_or(false);
            let exempt = provider_spec
                .map(|s| s.is_oauth || s.is_local || s.is_direct)
                .unwrap_or(false);
            if !has_key && !exempt {
                let name = provider_spec.map(|s| s.name).unwrap_or("");
                return Err(format!(
                    "No API key configured for provider '{name}'. Set one in \
                     ~/.nanobot/config.json under the providers section."
                ));
            }
        }
        _ => {}
    }

    let generation = GenerationSettings::from_config(cfg);

    let provider: Arc<dyn LLMProvider> = match backend {
        Backend::OpenAICodex => {
            let mut cx_cfg = OpenAICodexConfig::default();
            cx_cfg.default_model = model;
            let p = OpenAICodexProvider::new(cx_cfg).map_err(|e| format!("openai_codex: {e}"))?;
            Arc::new(p)
        }
        Backend::GitHubCopilot => {
            let p =
                GitHubCopilotProvider::new(model).map_err(|e| format!("github_copilot: {e}"))?;
            Arc::new(p)
        }
        Backend::AzureOpenAI => {
            let pc = provider_config
                .ok_or_else(|| "Azure OpenAI provider config missing".to_string())?;
            let endpoint = pc.api_base.clone().unwrap_or_default();
            let api_key = pc.api_key.clone().unwrap_or_default();
            let cfg_az = AzureOpenAIConfig::new(endpoint, api_key, model.clone());
            let p = AzureOpenAIProvider::new(cfg_az).map_err(|e| format!("azure_openai: {e}"))?;
            Arc::new(p.with_generation(generation))
        }
        Backend::Anthropic => {
            let mut ant = AnthropicConfig::new(model.clone());
            if let Some(pc) = provider_config {
                if let Some(k) = &pc.api_key {
                    ant = ant.with_api_key(k.clone());
                }
                if let Some(headers) = &pc.extra_headers {
                    for (k, v) in headers {
                        ant = ant.with_extra_header(k, v);
                    }
                }
            }
            if let Some(base) = resolve_api_base(provider_spec, provider_config) {
                ant = ant.with_api_base(base);
            }
            Arc::new(AnthropicProvider::new(ant).with_generation(generation))
        }
        Backend::OpenAICompat => {
            let mut oc = OpenAICompatConfig::new(model.clone());
            if let Some(pc) = provider_config {
                if let Some(k) = &pc.api_key {
                    oc = oc.with_api_key(k.clone());
                }
                if let Some(headers) = &pc.extra_headers {
                    for (k, v) in headers {
                        oc = oc.with_extra_header(k, v);
                    }
                }
            }
            if let Some(s) = provider_spec {
                oc = oc.with_spec(s);
            }
            if let Some(base) = resolve_api_base(provider_spec, provider_config) {
                oc = oc.with_api_base(base);
            }
            Arc::new(OpenAICompatProvider::new(oc).with_generation(generation))
        }
        Backend::Bedrock => {
            let mut bc = BedrockConfig::new(model.clone());
            if let Some(pc) = provider_config {
                if let Some(k) = &pc.api_key {
                    bc = bc.with_api_key(k.clone());
                }
            }
            if let Some(base) = resolve_api_base(provider_spec, provider_config) {
                bc = bc.with_api_base(base);
            }
            Arc::new(
                BedrockProvider::new(bc)
                    .map_err(|e| format!("bedrock: {e}"))?
                    .with_generation(generation),
            )
        }
    };
    Ok(provider)
}

fn resolve_api_base(
    provider_spec: Option<&ProviderSpec>,
    provider_config: Option<&ProviderConfig>,
) -> Option<String> {
    if let Some(pc) = provider_config {
        if let Some(b) = &pc.api_base {
            if !b.is_empty() {
                return Some(b.clone());
            }
        }
    }
    if let Some(s) = provider_spec {
        if !s.default_api_base.is_empty() {
            return Some(s.default_api_base.to_string());
        }
    }
    None
}

/// Resolve a [`ProviderSpec`] for the default model — honours the
/// `"auto"` sentinel just like Python.
pub fn resolve_spec(cfg: &Config) -> Option<&'static ProviderSpec> {
    let defaults = &cfg.agents.defaults;
    let configured = defaults.provider.trim();
    if !configured.is_empty() && configured != "auto" {
        if let Some(spec) = find_by_name(configured) {
            return Some(spec);
        }
    }
    find_by_model(&defaults.model)
}

/// Pull the [`ProviderConfig`] entry matching *spec* from the root config.
pub fn provider_config_for<'a>(
    cfg: &'a Config,
    spec: Option<&ProviderSpec>,
) -> Option<&'a ProviderConfig> {
    let spec = spec?;
    let p = &cfg.providers;
    Some(match spec.name {
        "custom" => &p.custom,
        "azure_openai" => &p.azure_openai,
        "anthropic" => &p.anthropic,
        "openai" => &p.openai,
        "openrouter" => &p.openrouter,
        "deepseek" => &p.deepseek,
        "groq" => &p.groq,
        "zhipu" => &p.zhipu,
        "dashscope" => &p.dashscope,
        "vllm" => &p.vllm,
        "ollama" => &p.ollama,
        "lm_studio" => &p.lm_studio,
        "ovms" => &p.ovms,
        "gemini" => &p.gemini,
        "moonshot" => &p.moonshot,
        "minimax" => &p.minimax,
        "minimax_anthropic" => &p.minimax_anthropic,
        "mistral" => &p.mistral,
        "stepfun" => &p.stepfun,
        "xiaomi_mimo" => &p.xiaomi_mimo,
        "aihubmix" => &p.aihubmix,
        "siliconflow" => &p.siliconflow,
        "volcengine" => &p.volcengine,
        "volcengine_coding_plan" => &p.volcengine_coding_plan,
        "byteplus" => &p.byteplus,
        "byteplus_coding_plan" => &p.byteplus_coding_plan,
        "openai_codex" => &p.openai_codex,
        "github_copilot" => &p.github_copilot,
        "qianfan" => &p.qianfan,
        _ => return None,
    })
}
