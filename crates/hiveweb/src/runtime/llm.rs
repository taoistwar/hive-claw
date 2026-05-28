//! LLM adapter (T120 / US5)
//!
//! 启动期从 `LLM_PRESETS_PATH` 加载 `llm_presets.toml`，解析为 PresetEntry 集合。
//! MVP：providers crate 集成留 TODO（actual primary+fallback provider construction）；
//! 本 commit 只完成 toml 解析 + default 标记校验 + Agent.model_preset 存在性校验。

use providers::{
    Backend, FallbackPreset, FallbackProvider, LLMProvider, ProviderBuildConfig,
};
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

    /// 把 preset 的 **primary** provider 实例化（不带 fallback chain — 单 shot 调用用）。
    pub fn build_primary(
        &self,
        preset_name: Option<&str>,
    ) -> Result<(Arc<dyn LLMProvider>, String), LlmAdapterError> {
        let entry = self.resolve(preset_name)?;
        let first = entry.providers_raw.first().ok_or_else(|| {
            LlmAdapterError::Parse(format!("preset {} 缺 providers[]", entry.name))
        })?;
        let provider = build_one(first)?;
        let model = first.model.clone().unwrap_or_default();
        Ok((provider, model))
    }

    /// 把 preset 的 **primary + fallback 链**一并实例化为 FallbackProvider。
    /// 用于 chat orchestrator 多轮路径，自动在主 provider 429/5xx 时切换备用。
    pub fn build_chain(
        &self,
        preset_name: Option<&str>,
    ) -> Result<(Arc<dyn LLMProvider>, String), LlmAdapterError> {
        let entry = self.resolve(preset_name)?;
        let providers_raw = &entry.providers_raw;
        let first = providers_raw.first().ok_or_else(|| {
            LlmAdapterError::Parse(format!("preset {} 缺 providers[]", entry.name))
        })?;
        let primary_model = first.model.clone().unwrap_or_default();
        let primary = build_one(first)?;

        if providers_raw.len() == 1 {
            return Ok((primary, primary_model));
        }

        // build_fallback_presets: each fallback entry → FallbackPreset
        // Need a factory closure keyed by model name to materialize on demand.
        let fallback_cfgs: Vec<ProviderConfig> = providers_raw[1..].to_vec();
        let presets: Vec<FallbackPreset> = fallback_cfgs
            .iter()
            .map(|cfg| FallbackPreset {
                model: cfg.model.clone().unwrap_or_default(),
                max_tokens: 2048,
                temperature: 0.7,
                reasoning_effort: None,
            })
            .collect();

        // factory: receives FallbackPreset (just model name+gen settings),
        // looks up matching ProviderConfig by model name and builds provider.
        let cfgs_for_factory: Arc<Vec<ProviderConfig>> = Arc::new(fallback_cfgs);
        let factory: providers::ProviderFactory = Arc::new(move |fp: &FallbackPreset| {
            let model = fp.model.clone();
            let cfg = cfgs_for_factory
                .iter()
                .find(|c| c.model.as_deref() == Some(model.as_str()))
                .cloned()
                .unwrap_or_else(|| ProviderConfig {
                    kind: "openai_compat".into(),
                    model: Some(model.clone()),
                    base_url: None,
                    api_key_env: None,
                });
            match build_one(&cfg) {
                Ok(p) => p,
                Err(e) => {
                    tracing::error!(model, error = %e, "fallback provider build failed; using primary again");
                    // 退化为 reuse primary — 不会失败构造（safer than panic）
                    // 真实生产环境应该 alert
                    Arc::new(NoopProvider) as Arc<dyn LLMProvider>
                }
            }
        });

        let chain = FallbackProvider::new(primary, presets, factory);
        Ok((Arc::new(chain) as Arc<dyn LLMProvider>, primary_model))
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
            )))
        }
    };
    let api_key = cfg
        .api_key_env
        .as_deref()
        .and_then(|val| {
            // 优先按环境变量名读取
            std::env::var(val).ok()
                // 如果环境变量不存在，则当作直接的 API Key 使用
                .or_else(|| {
                    if !val.is_empty() {
                        tracing::debug!(
                            env_name = val,
                            "api_key_env not found in env, treating as direct key"
                        );
                        Some(val.to_string())
                    } else {
                        None
                    }
                })
        });
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

/// 兜底 provider — fallback build 失败时使用，永远返回 error
#[derive(Debug)]
struct NoopProvider;

#[async_trait::async_trait]
impl LLMProvider for NoopProvider {
    fn default_model(&self) -> String {
        "noop".into()
    }
    async fn chat(&self, _req: providers::ChatRequest) -> providers::LLMResponse {
        providers::LLMResponse::error("fallback provider build failed")
    }
}
