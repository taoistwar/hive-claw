//! LLM adapter (T120 / US5)
//!
//! 启动期从 `LLM_PRESETS_PATH` 加载 `llm_presets.toml`，解析为 PresetEntry 集合。
//! 每个命名 preset 都按 `providers[]` 顺序构造 primary + fallback provider chain。

use providers::{Backend, FallbackProvider, FallbackTarget, LLMProvider, ProviderBuildConfig};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
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
    /// 隐藏字段：providers chain 配置原文。
    #[serde(skip)]
    pub providers_raw: Vec<ProviderConfig>,
    pub max_tokens: u32,
    pub temperature: f32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProviderConfig {
    pub kind: String,
    #[serde(default)]
    pub auth: ProviderAuth,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub api_key_env: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderAuth {
    #[default]
    ApiKey,
    None,
}

#[derive(Debug, Deserialize)]
struct PresetRaw {
    name: String,
    description: String,
    #[serde(default)]
    default: bool,
    #[serde(default)]
    providers: Vec<ProviderConfig>,
    #[serde(default = "default_max_tokens")]
    max_tokens: u32,
    #[serde(default = "default_temperature")]
    temperature: f32,
}

fn default_max_tokens() -> u32 {
    2048
}
fn default_temperature() -> f32 {
    0.7
}

#[derive(Debug, Deserialize)]
struct PresetsFile {
    #[serde(default)]
    preset: Vec<PresetRaw>,
}

#[derive(Clone)]
struct CachedProviderChain {
    provider: Arc<dyn LLMProvider>,
    primary_model: String,
}

#[derive(Default, Clone)]
pub struct LlmRegistry {
    pub presets: HashMap<LlmPresetName, PresetEntry>,
    pub default_name: Option<LlmPresetName>,
    chains: HashMap<LlmPresetName, CachedProviderChain>,
}

impl std::fmt::Debug for LlmRegistry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LlmRegistry")
            .field("presets", &self.presets)
            .field("default_name", &self.default_name)
            .field("cached_chain_count", &self.chains.len())
            .finish()
    }
}

impl LlmRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 启动期调用：解析 `llm_presets.toml`。
    /// 严格校验：配置文件必须存在，且恰好 1 个 default = true。
    /// default preset 无效时返回错误，由启动入口 fail-fast；无效的非 default
    /// preset 记录静态告警并跳过。
    pub fn load_from_path(path: &str) -> Result<Arc<Self>, LlmAdapterError> {
        if !std::path::Path::new(path).exists() {
            return Err(LlmAdapterError::ConfigMissing(path.to_string()));
        }
        let content = std::fs::read_to_string(path)
            .map_err(|e| LlmAdapterError::Parse(format!("read {path}: {e}")))?;
        let file: PresetsFile =
            toml::from_str(&content).map_err(|e| LlmAdapterError::Parse(format!("toml: {e}")))?;

        let mut defaults: Vec<String> = file
            .preset
            .iter()
            .filter(|raw| raw.default)
            .map(|raw| raw.name.clone())
            .collect();
        let default_name = match defaults.len() {
            0 => return Err(LlmAdapterError::NoDefault),
            1 => defaults.remove(0),
            _ => return Err(LlmAdapterError::MultipleDefaults(defaults)),
        };

        let mut names = HashSet::new();
        for raw in &file.preset {
            if !names.insert(raw.name.as_str()) {
                return Err(LlmAdapterError::Parse("duplicate preset name".to_string()));
            }
        }

        let mut presets = HashMap::new();
        let mut chains = HashMap::new();
        for raw in file.preset {
            let chain = match build_complete_chain(&raw) {
                Ok(chain) => chain,
                Err(error) if raw.default => return Err(error),
                Err(_) => {
                    tracing::warn!(
                        error_kind = "llm_non_default_preset_invalid",
                        "invalid non-default LLM preset skipped"
                    );
                    continue;
                }
            };

            if raw.default {
                debug_assert_eq!(raw.name, default_name);
            }
            chains.insert(raw.name.clone(), chain);
            presets.insert(
                raw.name.clone(),
                PresetEntry {
                    name: raw.name.clone(),
                    description: raw.description,
                    is_default: raw.default,
                    providers_raw: raw.providers,
                    max_tokens: raw.max_tokens,
                    temperature: raw.temperature,
                },
            );
        }

        tracing::info!(
            preset_count = presets.len(),
            default = %default_name,
            "LlmRegistry loaded"
        );
        Ok(Arc::new(Self {
            presets,
            default_name: Some(default_name),
            chains,
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

    /// Resolve preset generation defaults without silently substituting a
    /// different preset.
    ///
    /// `None` resolves the configured default. Every explicit name, including
    /// the empty string, must exist in the startup registry.
    pub fn resolve_generation_defaults(
        &self,
        preset_name: Option<&str>,
    ) -> Result<(u32, f32), LlmAdapterError> {
        let entry = self.resolve(preset_name)?;
        Ok((entry.max_tokens, entry.temperature))
    }

    /// 克隆启动期缓存的完整 provider chain。
    ///
    /// 所有 runtime LLM 调用都使用此入口，因此同一 preset 共享 provider HTTP
    /// clients 与 circuit-breaker 状态。显式未知 preset 返回 `Unknown`；只有
    /// `None` 才解析为 default preset。
    pub fn build_chain(
        &self,
        preset_name: Option<&str>,
    ) -> Result<(Arc<dyn LLMProvider>, String), LlmAdapterError> {
        let entry = self.resolve(preset_name)?;
        let chain = self.chains.get(&entry.name).ok_or_else(|| {
            LlmAdapterError::Parse(format!(
                "preset {} has no cached provider chain",
                entry.name
            ))
        })?;
        Ok((Arc::clone(&chain.provider), chain.primary_model.clone()))
    }
}

fn build_complete_chain(raw: &PresetRaw) -> Result<CachedProviderChain, LlmAdapterError> {
    let mut configs = raw.providers.iter();
    let primary_config = configs
        .next()
        .ok_or_else(|| LlmAdapterError::Parse(format!("preset {} has no providers", raw.name)))?;
    let primary_model = primary_config.model.clone().unwrap_or_default();
    let primary = build_one(primary_config)?;
    let fallback_targets = configs
        .map(|config| {
            let model = config.model.clone().unwrap_or_default();
            build_one(config).map(|provider| FallbackTarget::new(model, provider))
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok(CachedProviderChain {
        provider: Arc::new(FallbackProvider::new(primary, fallback_targets)),
        primary_model,
    })
}

fn provider_api_key(cfg: &ProviderConfig) -> Result<Option<String>, LlmAdapterError> {
    if matches!(cfg.auth, ProviderAuth::None) {
        if cfg.api_key_env.is_some() {
            return Err(LlmAdapterError::Parse(
                "no-auth provider must not declare a credential environment".to_string(),
            ));
        }
        let base_url = cfg.base_url.as_deref().ok_or_else(|| {
            LlmAdapterError::Parse(
                "no-auth provider requires an explicit valid base URL".to_string(),
            )
        })?;
        let parsed = reqwest::Url::parse(base_url).map_err(|_| {
            LlmAdapterError::Parse(
                "no-auth provider requires an explicit valid base URL".to_string(),
            )
        })?;
        if base_url.is_empty()
            || base_url.trim() != base_url
            || !matches!(parsed.scheme(), "http" | "https")
            || parsed.host_str().is_none()
        {
            return Err(LlmAdapterError::Parse(
                "no-auth provider requires an explicit valid base URL".to_string(),
            ));
        }
        return Ok(None);
    }

    let Some(env_name) = cfg.api_key_env.as_deref() else {
        return Err(LlmAdapterError::Parse(
            "provider credential environment is required".to_string(),
        ));
    };
    if env_name.is_empty() || env_name.trim() != env_name {
        return Err(LlmAdapterError::Parse(
            "provider credential environment is unavailable".to_string(),
        ));
    }

    match std::env::var(env_name) {
        Ok(value) if !value.trim().is_empty() => Ok(Some(value)),
        Ok(_) | Err(_) => Err(LlmAdapterError::Parse(
            "provider credential environment is unavailable".to_string(),
        )),
    }
}

/// 从单个 ProviderConfig 构造 Arc<dyn LLMProvider>
fn build_one(cfg: &ProviderConfig) -> Result<Arc<dyn LLMProvider>, LlmAdapterError> {
    let backend = match cfg.kind.as_str() {
        "anthropic" => Backend::Anthropic,
        "azure_openai" => Backend::AzureOpenAI,
        "bedrock" => Backend::Bedrock,
        "github_copilot" => Backend::GitHubCopilot,
        "openai_codex" => Backend::OpenAICodex,
        "openai_compat" | "openai" => Backend::OpenAICompat,
        other => {
            return Err(LlmAdapterError::Parse(format!(
                "未知 provider kind: {other}"
            )));
        }
    };
    let api_key = provider_api_key(cfg)?;
    let build = ProviderBuildConfig {
        model: cfg.model.clone().unwrap_or_default(),
        api_key,
        api_base: cfg.base_url.clone(),
        extra_headers: None,
        extra_body: None,
        region: None,
        profile: None,
    };
    providers::build_provider(backend, build).map_err(LlmAdapterError::Parse)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn write_presets(contents: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("hiveweb-llm-presets-{}.toml", uuid::Uuid::new_v4()));
        std::fs::write(&path, contents).expect("write temporary LLM preset config");
        path
    }

    #[test]
    fn missing_preset_file_is_a_startup_error() {
        let path = std::env::temp_dir().join(format!(
            "hiveweb-missing-llm-presets-{}.toml",
            uuid::Uuid::new_v4()
        ));

        assert!(matches!(
            LlmRegistry::load_from_path(path.to_str().expect("UTF-8 temp path")),
            Err(LlmAdapterError::ConfigMissing(_))
        ));
    }

    #[test]
    fn malformed_preset_file_is_a_startup_error() {
        let path = write_presets("[[preset]\nname =");

        let result = LlmRegistry::load_from_path(path.to_str().expect("UTF-8 temp path"));
        std::fs::remove_file(path).expect("remove temporary LLM preset config");

        assert!(matches!(result, Err(LlmAdapterError::Parse(_))));
    }

    #[test]
    fn preset_file_requires_exactly_one_default() {
        let no_default = write_presets(
            r#"
[[preset]]
name = "optional"
description = "optional"
"#,
        );
        let no_default_result =
            LlmRegistry::load_from_path(no_default.to_str().expect("UTF-8 temp path"));
        std::fs::remove_file(no_default).expect("remove temporary LLM preset config");

        let multiple_defaults = write_presets(
            r#"
[[preset]]
name = "first"
description = "first"
default = true

[[preset]]
name = "second"
description = "second"
default = true
"#,
        );
        let multiple_result =
            LlmRegistry::load_from_path(multiple_defaults.to_str().expect("UTF-8 temp path"));
        std::fs::remove_file(multiple_defaults).expect("remove temporary LLM preset config");

        assert!(matches!(no_default_result, Err(LlmAdapterError::NoDefault)));
        assert!(matches!(
            multiple_result,
            Err(LlmAdapterError::MultipleDefaults(_))
        ));
    }

    #[test]
    fn duplicate_preset_names_are_a_startup_error() {
        let path = write_presets(
            r#"
[[preset]]
name = "duplicate"
description = "default"
default = true

  [[preset.providers]]
  kind = "openai_compat"
  model = "example"

[[preset]]
name = "duplicate"
description = "optional"
default = false

  [[preset.providers]]
  kind = "openai_compat"
  model = "example"
"#,
        );

        let result = LlmRegistry::load_from_path(path.to_str().expect("UTF-8 temp path"));
        std::fs::remove_file(path).expect("remove temporary LLM preset config");

        assert!(matches!(result, Err(LlmAdapterError::Parse(_))));
    }

    #[test]
    fn invalid_default_provider_is_a_startup_error() {
        let path = write_presets(
            r#"
[[preset]]
name = "default"
description = "default"
default = true

  [[preset.providers]]
  kind = "unsupported"
  model = "example"
"#,
        );

        let result = LlmRegistry::load_from_path(path.to_str().expect("UTF-8 temp path"));
        std::fs::remove_file(path).expect("remove temporary LLM preset config");

        assert!(matches!(result, Err(LlmAdapterError::Parse(_))));
    }

    #[test]
    fn configured_credential_env_must_exist_and_be_non_empty() {
        for credential_name in ["HIVEWEB_TEST_MISSING_LLM_CREDENTIAL_8F18D8A7", ""] {
            let path = write_presets(&format!(
                r#"
[[preset]]
name = "default"
description = "default"
default = true

  [[preset.providers]]
  kind = "openai_compat"
  model = "example"
  api_key_env = "{credential_name}"
"#
            ));

            let result = LlmRegistry::load_from_path(path.to_str().expect("UTF-8 temp path"));
            std::fs::remove_file(path).expect("remove temporary LLM preset config");

            assert!(
                matches!(result, Err(LlmAdapterError::Parse(_))),
                "credential env `{credential_name}` must fail closed"
            );
        }
    }

    #[test]
    fn missing_credential_skips_only_the_non_default_preset() {
        let path = write_presets(
            r#"
[[preset]]
name = "local-default"
description = "local default"
default = true

  [[preset.providers]]
  kind = "openai_compat"
  model = "local-model"
  auth = "none"
  base_url = "http://127.0.0.1:11434/v1"

[[preset]]
name = "missing-cloud-credential"
description = "invalid optional preset"
default = false

  [[preset.providers]]
  kind = "openai_compat"
  model = "cloud-model"
  api_key_env = "HIVEWEB_TEST_MISSING_LLM_CREDENTIAL_0D185E03"
"#,
        );

        let registry =
            LlmRegistry::load_from_path(path.to_str().expect("UTF-8 temp path")).expect("load");
        std::fs::remove_file(path).expect("remove temporary LLM preset config");

        assert_eq!(registry.default_name.as_deref(), Some("local-default"));
        assert!(registry.presets.contains_key("local-default"));
        assert!(!registry.presets.contains_key("missing-cloud-credential"));
    }

    #[test]
    fn invalid_non_default_preset_is_skipped_when_default_is_valid() {
        let path = write_presets(
            r#"
[[preset]]
name = "default"
description = "default"
default = true

  [[preset.providers]]
  kind = "openai_compat"
  model = "example"
  auth = "none"
  base_url = "http://127.0.0.1:11434/v1"

[[preset]]
name = "broken"
description = "broken"
default = false

  [[preset.providers]]
  kind = "unsupported"
  model = "example"
"#,
        );

        let registry =
            LlmRegistry::load_from_path(path.to_str().expect("UTF-8 temp path")).expect("load");
        std::fs::remove_file(path).expect("remove temporary LLM preset config");

        assert_eq!(registry.default_name.as_deref(), Some("default"));
        assert!(registry.presets.contains_key("default"));
        assert!(!registry.presets.contains_key("broken"));
    }

    #[test]
    fn default_preset_without_a_provider_is_a_startup_error() {
        let path = write_presets(
            r#"
[[preset]]
name = "default"
description = "default"
default = true
"#,
        );

        let result = LlmRegistry::load_from_path(path.to_str().expect("UTF-8 temp path"));
        std::fs::remove_file(path).expect("remove temporary LLM preset config");

        assert!(matches!(result, Err(LlmAdapterError::Parse(_))));
    }

    #[test]
    fn generation_defaults_resolve_null_but_fail_closed_for_explicit_unknown_names() {
        let path = write_presets(
            r#"
[[preset]]
name = "default"
description = "default"
default = true
max_tokens = 137
temperature = 0.25

  [[preset.providers]]
  kind = "openai_compat"
  model = "default-model"
  auth = "none"
  base_url = "http://127.0.0.1:11434/v1"

[[preset]]
name = "explicit"
description = "explicit"
default = false
max_tokens = 911
temperature = 0.55

  [[preset.providers]]
  kind = "openai_compat"
  model = "explicit-model"
  auth = "none"
  base_url = "http://127.0.0.1:11435/v1"
"#,
        );
        let registry =
            LlmRegistry::load_from_path(path.to_str().expect("UTF-8 temp path")).expect("load");
        std::fs::remove_file(path).expect("remove temporary LLM preset config");

        assert_eq!(
            registry
                .resolve_generation_defaults(None)
                .expect("NULL uses default"),
            (137, 0.25)
        );
        assert_eq!(
            registry
                .resolve_generation_defaults(Some("explicit"))
                .expect("resolve explicit preset"),
            (911, 0.55)
        );
        assert!(matches!(
            registry.resolve_generation_defaults(Some("")),
            Err(LlmAdapterError::Unknown(name)) if name.is_empty()
        ));
        assert!(matches!(
            registry.resolve_generation_defaults(Some("missing")),
            Err(LlmAdapterError::Unknown(name)) if name == "missing"
        ));
    }
}
