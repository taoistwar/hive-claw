//! Helpers for runtime model preset selection.
//!
//! Port of `nanobot.agent.model_presets` Python module.

use std::collections::HashMap;
use std::sync::Arc;

use providers::LLMProvider;

/// A loader that resolves a preset name into a [`ProviderSnapshot`].
pub type PresetSnapshotLoader = Arc<dyn Fn(&str) -> ProviderSnapshot + Send + Sync>;

/// Signature helper that trims a selection tuple to its first two elements.
///
/// Mirrors `default_selection_signature` from Python.
/// Since the original Python uses opaque tuple elements, this Rust version
/// accepts a length and returns the truncated count.
pub fn default_selection_signature(signature_len: usize) -> usize {
    if signature_len == 0 {
        0
    } else {
        signature_len.min(2)
    }
}

/// Collect configured model presets plus a resolved `"default"` entry.
///
/// Equivalent to `configured_model_presets` in Python:
/// `{**config.model_presets, "default": config.resolve_default_preset()}`
///
/// # Arguments
/// * `model_presets` — map of named presets from config
/// * `default_preset` — the resolved default preset
pub fn configured_model_presets(
    model_presets: HashMap<String, ModelPresetConfig>,
    default_preset: ModelPresetConfig,
) -> HashMap<String, ModelPresetConfig> {
    let mut result = model_presets;
    result.insert("default".to_string(), default_preset);
    result
}

/// Build a [`PresetSnapshotLoader`] from an optional custom loader and a base provider.
///
/// If `provider_snapshot_loader` is provided, it is wrapped into a closure
/// that accepts the preset name. Otherwise the loader returns fallback
/// snapshots using the provided `base_provider` reference. Callers should
/// provide a real loader for production use with per-preset providers.
pub fn make_preset_snapshot_loader(
    provider_snapshot_loader: Option<PresetSnapshotLoader>,
    base_provider: Arc<dyn LLMProvider>,
) -> PresetSnapshotLoader {
    match provider_snapshot_loader {
        Some(loader) => Arc::new(move |name| loader(name)),
        None => Arc::new(move |name| ProviderSnapshot {
            provider: Arc::clone(&base_provider),
            model: format!("preset/{name}"),
            context_window_tokens: 65_536,
            signature: 0,
        }),
    }
}

/// Build a static snapshot from a fully wired provider and a preset config.
///
/// Mirrors `build_static_preset_snapshot` in Python:
/// - assigns `preset.to_generation_settings()` to the provider
/// - returns a `ProviderSnapshot` with the preset metadata
///
/// # Arguments
/// * `provider` — the LLM provider (mutated in place for generation settings)
/// * `name` — preset name
/// * `preset` — the `ModelPresetConfig` carrying model + context window info
///
/// Returns a [`ProviderSnapshot`].
pub fn build_static_preset_snapshot(
    provider: &Arc<dyn LLMProvider>,
    name: &str,
    preset: &ModelPresetConfig,
) -> ProviderSnapshot {
    // Note: in Rust we cannot mutate the provider behind Arc;
    // the generation settings would need to be carried separately.
    // This is a structural placeholder.
    let _ = provider;

    let signature = compute_signature(name, &preset.model);

    ProviderSnapshot {
        provider: Arc::clone(provider),
        model: preset.model.clone(),
        context_window_tokens: preset.context_window_tokens,
        signature,
    }
}

/// Build a runtime preset snapshot, optionally delegating to a loader.
///
/// If `loader` is provided, it is invoked with the preset name.
/// Otherwise falls back to [`build_static_preset_snapshot`].
///
/// # Arguments
/// * `name` — preset name
/// * `presets` — map of preset name to config
/// * `provider` — the LLM provider
/// * `loader` — optional custom snapshot loader
///
/// Returns a [`ProviderSnapshot`].
pub fn build_runtime_preset_snapshot(
    name: &str,
    presets: &HashMap<String, ModelPresetConfig>,
    provider: &Arc<dyn LLMProvider>,
    loader: Option<&PresetSnapshotLoader>,
) -> ProviderSnapshot {
    if let Some(loader) = loader {
        return loader(name);
    }
    let preset = presets
        .get(name)
        .unwrap_or_else(|| panic!("model_preset {name:?} not found in presets"));
    build_static_preset_snapshot(provider, name, preset)
}

/// Validate and normalize a preset name against the available presets.
///
/// Returns the trimmed name or raises an error.
///
/// # Errors
/// Returns `Err` if the name is empty or not found in `presets`.
pub fn normalize_preset_name(
    name: Option<&str>,
    presets: &HashMap<String, ModelPresetConfig>,
) -> Result<String, String> {
    let name = match name {
        Some(n) if !n.trim().is_empty() => n.trim().to_string(),
        _ => return Err("model_preset must be a non-empty string".to_string()),
    };
    if presets.contains_key(&name) {
        Ok(name)
    } else {
        let available: Vec<&str> = presets.keys().map(|k| k.as_str()).collect();
        let available_str = if available.is_empty() {
            "(none)".to_string()
        } else {
            available.join(", ")
        };
        Err(format!(
            "model_preset {name:?} not found. Available: {available_str}"
        ))
    }
}

// ---------------------------------------------------------------------------
// Deferred / placeholder types
// ---------------------------------------------------------------------------

/// Minimal placeholder for the model preset config.
///
/// Port of `nanobot.config.schema.ModelPresetConfig` (Python Pydantic model).
/// A named set of model + generation parameters for quick switching.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ModelPresetConfig {
    /// Model identifier (e.g. "anthropic/claude-opus-4-5").
    pub model: String,
    /// Provider name (e.g. "anthropic", "openrouter") or "auto" for auto-detection.
    #[serde(default)]
    pub provider: String,
    /// Maximum output tokens.
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    /// Context window size in tokens.
    #[serde(default = "default_context_window")]
    pub context_window_tokens: u32,
    /// Sampling temperature.
    #[serde(default = "default_temperature")]
    pub temperature: f64,
    /// Reasoning effort for reasoning models ("low", "medium", "high", "none").
    #[serde(default)]
    pub reasoning_effort: Option<String>,
}

fn default_max_tokens() -> u32 {
    8192
}

fn default_context_window() -> u32 {
    65_536
}

fn default_temperature() -> f64 {
    0.1
}

/// Minimal placeholder for provider snapshots.
///
/// The real definition lives in `crate::loop_::ProviderSnapshot`.
/// This local copy avoids circular imports; once the dependency graph
/// is cleaned up, import from `crate::loop_` instead.
#[derive(Clone)]
pub struct ProviderSnapshot {
    pub provider: Arc<dyn LLMProvider>,
    pub model: String,
    pub context_window_tokens: u32,
    pub signature: u64,
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn compute_signature(name: &str, model: &str) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    "model_preset".hash(&mut hasher);
    name.hash(&mut hasher);
    model.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_preset_name_valid() {
        let mut presets = HashMap::new();
        presets.insert(
            "fast".to_string(),
            ModelPresetConfig {
                model: "gpt-4o-mini".to_string(),
                provider: "openai".to_string(),
                max_tokens: 4096,
                context_window_tokens: 8192,
                temperature: 0.0,
                reasoning_effort: None,
            },
        );
        let result = normalize_preset_name(Some("fast"), &presets);
        assert_eq!(result, Ok("fast".to_string()));
    }

    #[test]
    fn test_normalize_preset_name_empty() {
        let presets: HashMap<String, ModelPresetConfig> = HashMap::new();
        let result = normalize_preset_name(Some(""), &presets);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("non-empty string"));
    }

    #[test]
    fn test_normalize_preset_name_none() {
        let presets: HashMap<String, ModelPresetConfig> = HashMap::new();
        let result = normalize_preset_name(None, &presets);
        assert!(result.is_err());
    }

    #[test]
    fn test_normalize_preset_name_missing() {
        let presets: HashMap<String, ModelPresetConfig> = HashMap::new();
        let result = normalize_preset_name(Some("unknown"), &presets);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    #[test]
    fn test_configured_model_presets_includes_default() {
        let mut presets = HashMap::new();
        presets.insert(
            "smart".to_string(),
            ModelPresetConfig {
                model: "gpt-4".to_string(),
                provider: "openai".to_string(),
                max_tokens: 8192,
                context_window_tokens: 8192,
                temperature: 0.7,
                reasoning_effort: Some("high".to_string()),
            },
        );
        let default = ModelPresetConfig {
            model: "gpt-4o-mini".to_string(),
            provider: "auto".to_string(),
            max_tokens: 4096,
            context_window_tokens: 4096,
            temperature: 0.1,
            reasoning_effort: None,
        };
        let result = configured_model_presets(presets, default.clone());
        assert!(result.contains_key("smart"));
        assert_eq!(result.get("default"), Some(&default));
    }
}
