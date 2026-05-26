//! LLM adapter（research §7 / FR-025）
//!
//! 启动期从 `LLM_PRESETS_PATH` 加载 `llm_presets.toml`，对每个 preset 构造
//! `providers::FallbackProvider`（primary + fallback 链），存入
//! `HashMap<LlmPresetName, Arc<dyn LLMProvider>>`。运行时按 Agent.model_preset
//! 解析；NULL 走全局默认。
//!
//! 当前为骨架；具体 toml 解析 + provider 构造接通后补 US5。

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

/// 简化的占位类型；后续会替换为 `Arc<providers::FallbackProvider>` 等真实类型。
#[derive(Debug, Clone)]
pub struct PresetEntry {
    pub name: LlmPresetName,
    pub description: String,
    pub is_default: bool,
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

    /// 加载 llm_presets.toml；真实实现见 US5。
    pub fn load_from_path(path: &str) -> Result<Arc<Self>, LlmAdapterError> {
        if !std::path::Path::new(path).exists() {
            return Err(LlmAdapterError::ConfigMissing(path.into()));
        }
        // TODO(US5): toml::from_str + providers::make_provider + FallbackProvider::new
        Ok(Arc::new(Self::default()))
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
}
