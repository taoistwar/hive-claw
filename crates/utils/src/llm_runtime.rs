/// Small helpers for passing the active LLM provider/model together.

use std::sync::Arc;

/// A bundled LLM provider and model reference.
#[derive(Debug, Clone)]
pub struct LLMRuntime {
    /// TODO: Replace with actual provider type from the `providers` crate.
    pub provider: String,
    pub model: String,
}

impl LLMRuntime {
    pub fn new(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
        }
    }
}

/// A resolver function that returns an LLMRuntime.
pub type LLMRuntimeResolver = Arc<dyn Fn() -> LLMRuntime + Send + Sync>;

/// Create a static LLMRuntime resolver.
pub fn static_llm_runtime(provider: impl Into<String>, model: impl Into<String>) -> LLMRuntimeResolver {
    let runtime = LLMRuntime::new(provider, model);
    Arc::new(move || runtime.clone())
}
