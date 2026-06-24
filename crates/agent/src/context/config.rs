//! Configuration for AgentContext.

/// Soft limit configuration for each category.
#[derive(Debug, Clone)]
pub struct ContextConfig {
    /// Default soft limit for all categories (number of entries).
    /// When exceeded, a warn! log is emitted but the write is not rejected.
    pub soft_limit: usize,
    /// Maximum token budget for prompt building.
    pub max_prompt_tokens: usize,
    /// Token estimation mode: "chars" (character count) or "tiktoken".
    pub token_estimation_mode: String,
    /// When true, terminated contexts preserve their snapshot for debugging.
    pub debug_mode: bool,
    /// JSON schema version for serialization compatibility.
    pub schema_version: String,
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            soft_limit: 100,
            max_prompt_tokens: 8000,
            token_estimation_mode: "chars".to_string(),
            debug_mode: false,
            schema_version: "v1".to_string(),
        }
    }
}

impl ContextConfig {
    /// Create a new ContextConfig with custom soft limit.
    pub fn with_soft_limit(mut self, limit: usize) -> Self {
        self.soft_limit = limit;
        self
    }

    /// Create a new ContextConfig with custom max prompt tokens.
    pub fn with_max_prompt_tokens(mut self, tokens: usize) -> Self {
        self.max_prompt_tokens = tokens;
        self
    }

    /// Enable debug mode to preserve terminated context snapshots.
    pub fn with_debug_mode(mut self, enabled: bool) -> Self {
        self.debug_mode = enabled;
        self
    }
}
