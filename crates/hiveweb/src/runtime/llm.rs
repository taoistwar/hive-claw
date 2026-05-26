//! LLM adapter (T120 / US5)
//!
//! 启动期从 `LLM_PRESETS_PATH` 加载 `llm_presets.toml`，解析为 PresetEntry 集合。
//! MVP：providers crate 集成留 TODO（actual primary+fallback provider construction）；
//! 本 commit 只完成 toml 解析 + default 标记校验 + Agent.model_preset 存在性校验。

use providers::{Backend, LLMProvider, ProviderBuildConfig};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;

pub type LlmPresetName = String;

#[derive(Debug, thiserror::Error)]
pub enum LlmAdapterError {
    #[error("preset config not found at {0}")]
    ConfigMissing(String),
    #[error("no default preset declared")]
    NoDefault,
    #[error("multiple defaults declared: {0:?}")]
    MultipleDefaults(Vec<String>),
    #[error("preset parse error: {0}")]
    Parse(String),
    #[error("unknown preset {0}")]
    Unknown(String),
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PresetEntry {
    pub name: LlmPresetName,
    pub description: String,
    pub is_default: bool,
    /// 隐藏字段：providers chain 配置原文（实际 provider 构造在后续 commit）
    #[serde(skip)]
    pub providers_raw: Vec<ProviderConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProviderConfig {
    pub kind: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub api_key_env: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PresetRaw {
    name: String,
    description: String,
    #[serde(default)]
    default: bool,
    #[serde(default)]
    providers: Vec<ProviderConfig>,
}

#[derive(Debug, Deserialize)]
struct PresetsFile {
    #[serde(default)]
    preset: Vec<PresetRaw>,
}

#[derive(Debug, Default, Clone)]
pub struct LlmRegistry {
    pub presets: HashMap<LlmPresetName, PresetEntry>,
    pub default_name: Option<LlmPresetName>,
}

impl LlmRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 启动期调用：解析 `llm_presets.toml`。
    /// 严格校验：恰好 1 个 default = true；否则 panic（fail-fast，plan §Startup）。
    pub fn load_from_path(path: &str) -> Result<Arc<Self>, LlmAdapterError> {
        if !std::path::Path::new(path).exists() {
            tracing::warn!(
                path,
                "llm_presets.toml not found; LlmRegistry will be empty (Agents 不能使用 model_preset)"
            );
            return Ok(Arc::new(Self::default()));
        }
        let content = std::fs::read_to_string(path)
            .map_err(|e| LlmAdapterError::Parse(format!("read {path}: {e}")))?;
        let file: PresetsFile =
            toml::from_str(&content).map_err(|e| LlmAdapterError::Parse(format!("toml: {e}")))?;

        let mut presets = HashMap::new();
        let mut defaults: Vec<String> = Vec::new();
        for raw in file.preset {
            if raw.default {
                defaults.push(raw.name.clone());
            }
            presets.insert(
                raw.name.clone(),
                PresetEntry {
                    name: raw.name.clone(),
                    description: raw.description,
                    is_default: raw.default,
                    providers_raw: raw.providers,
                },
            );
        }
        let default_name = match defaults.len() {
            0 => return Err(LlmAdapterError::NoDefault),
            1 => Some(defaults.remove(0)),
            _ => return Err(LlmAdapterError::MultipleDefaults(defaults)),
        };

        tracing::info!(
            preset_count = presets.len(),
            default = ?default_name,
            "LlmRegistry loaded"
        );
        Ok(Arc::new(Self {
            presets,
            default_name,
        }))
    }

    pub fn resolve(&self, preset: Option<&str>) -> Result<&PresetEntry, LlmAdapterError> {
        let name = preset
            .map(String::from)
            .or_else(|| self.default_name.clone())
            .ok_or(LlmAdapterError::NoDefault)?;
        self.presets
            .get(&name)
            .ok_or(LlmAdapterError::Unknown(name))
    }

    /// 给前端 `/api/agents/model-presets` 用的快照
    pub fn snapshot(&self) -> Vec<PresetEntry> {
        let mut v: Vec<PresetEntry> = self.presets.values().cloned().collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }

    /// 把 preset 的 **primary** provider 实例化（T128 MVP — FallbackProvider 链留待后续）。
    pub fn build_primary(
        &self,
        preset_name: Option<&str>,
    ) -> Result<(Arc<dyn LLMProvider>, String), LlmAdapterError> {
        let entry = self.resolve(preset_name)?;
        let Some(first) = entry.providers_raw.first() else {
            return Err(LlmAdapterError::Parse(format!(
                "preset {} 缺 providers[]",
                entry.name
            )));
        };
        let backend = match first.kind.as_str() {
            "anthropic" => Backend::Anthropic,
            "azure_openai" => Backend::AzureOpenAI,
            "bedrock" => Backend::Bedrock,
            "github_copilot" => Backend::GitHubCopilot,
            "openai_codex" => Backend::OpenAICodex,
            "openai_compat" | "openai" => Backend::OpenAICompat,
            other => {
                return Err(LlmAdapterError::Parse(format!(
                    "未知 provider kind: {other}"
                )))
            }
        };
        let api_key = first
            .api_key_env
            .as_deref()
            .and_then(|env_name| std::env::var(env_name).ok());
        let model = first.model.clone().unwrap_or_default();
        let cfg = ProviderBuildConfig {
            model: model.clone(),
            api_key,
            api_base: first.base_url.clone(),
            extra_headers: None,
            extra_body: None,
            region: None,
            profile: None,
        };
        let provider = providers::build_provider(backend, cfg).map_err(LlmAdapterError::Parse)?;
        Ok((provider, model))
    }
}
