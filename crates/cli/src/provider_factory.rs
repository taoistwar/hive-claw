//! Convenience constructors for [`providers::LLMProvider`] instances.
//!
//! The CLI needs to build a concrete provider from environment variables
//! (or `--provider` flags). Keeping this in a dedicated module means each
//! subcommand just calls [`default_provider`] and doesn't care which
//! backend was selected.

use std::sync::Arc;

use providers::anthropic::{AnthropicConfig, AnthropicProvider};
use providers::azure_openai::{AzureOpenAIConfig, AzureOpenAIProvider};
use providers::openai_compat::{OpenAICompatConfig, OpenAICompatProvider};
use providers::registry::find_by_name;
use providers::{GitHubCopilotProvider, LLMProvider};

/// Which backend the CLI should instantiate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderChoice {
    OpenAI,
    Anthropic,
    Azure,
    GithubCopilot,
    Custom,
}

impl ProviderChoice {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.to_ascii_lowercase().as_str() {
            "openai" => Some(Self::OpenAI),
            "anthropic" | "claude" => Some(Self::Anthropic),
            "azure" | "azure_openai" | "azure-openai" => Some(Self::Azure),
            "github_copilot" | "github-copilot" | "copilot" => Some(Self::GithubCopilot),
            "custom" | "openai_compat" | "openai-compat" => Some(Self::Custom),
            _ => None,
        }
    }
}

/// Build a provider from environment variables.
///
/// Selection precedence:
/// * explicit `choice`
/// * `PROVIDER` env var
/// * OpenAI (API_KEY=`OPENAI_API_KEY`)
///
/// The caller can override the default model with the `model` argument
/// (usually from `--model` on the CLI).
pub fn default_provider(
    choice: Option<ProviderChoice>,
    model: Option<String>,
) -> Result<Arc<dyn LLMProvider>, String> {
    let choice = choice
        .or_else(|| std::env::var("PROVIDER").ok().and_then(|s| ProviderChoice::parse(&s)))
        .unwrap_or(ProviderChoice::OpenAI);

    match choice {
        ProviderChoice::OpenAI => build_openai_compat("openai", model, None),
        ProviderChoice::Custom => {
            let base = std::env::var("API_BASE")
                .or_else(|_| std::env::var("OPENAI_API_BASE"))
                .ok();
            build_openai_compat("openai", model, base)
        }
        ProviderChoice::Anthropic => {
            let api_key = std::env::var("ANTHROPIC_API_KEY")
                .map_err(|_| "ANTHROPIC_API_KEY is not set".to_string())?;
            let model = model.unwrap_or_else(|| "claude-3-5-sonnet-latest".into());
            let mut cfg = AnthropicConfig::new(model);
            cfg = cfg.with_api_key(api_key);
            Ok(Arc::new(AnthropicProvider::new(cfg)))
        }
        ProviderChoice::Azure => {
            let api_key = std::env::var("AZURE_OPENAI_API_KEY")
                .map_err(|_| "AZURE_OPENAI_API_KEY is not set".to_string())?;
            let endpoint = std::env::var("AZURE_OPENAI_ENDPOINT")
                .map_err(|_| "AZURE_OPENAI_ENDPOINT is not set".to_string())?;
            let deployment = std::env::var("AZURE_OPENAI_DEPLOYMENT")
                .or_else(|_| std::env::var("AZURE_OPENAI_MODEL"))
                .map_err(|_| "AZURE_OPENAI_DEPLOYMENT is not set".to_string())?;
            let mut cfg = AzureOpenAIConfig::new(endpoint, api_key, deployment);
            if let Ok(v) = std::env::var("AZURE_OPENAI_API_VERSION") {
                cfg = cfg.with_api_version(v);
            }
            // `model` on request overrides the default deployment — the
            // Azure provider already honours per-request `model` fields.
            let _ = model;
            Ok(Arc::new(
                AzureOpenAIProvider::new(cfg).map_err(|e| e.to_string())?,
            ))
        }
        ProviderChoice::GithubCopilot => {
            let model = model.unwrap_or_else(|| "gpt-4o".into());
            let p = GitHubCopilotProvider::new(model)
                .map_err(|e| format!("failed to init copilot provider: {e}"))?;
            Ok(Arc::new(p))
        }
    }
}

fn build_openai_compat(
    spec_name: &str,
    model: Option<String>,
    override_base: Option<String>,
) -> Result<Arc<dyn LLMProvider>, String> {
    let api_key = std::env::var("OPENAI_API_KEY")
        .or_else(|_| std::env::var("API_KEY"))
        .map_err(|_| "OPENAI_API_KEY (or API_KEY) is not set".to_string())?;
    let model = model
        .or_else(|| std::env::var("MODEL").ok())
        .unwrap_or_else(|| "gpt-4o-mini".into());
    let mut cfg = OpenAICompatConfig::new(model).with_api_key(api_key);
    if let Some(spec) = find_by_name(spec_name) {
        cfg = cfg.with_spec(spec);
    }
    if let Some(base) = override_base.or_else(|| std::env::var("OPENAI_API_BASE").ok()) {
        cfg = cfg.with_api_base(base);
    }
    Ok(Arc::new(OpenAICompatProvider::new(cfg)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_choice_aliases() {
        assert_eq!(ProviderChoice::parse("openai"), Some(ProviderChoice::OpenAI));
        assert_eq!(ProviderChoice::parse("Claude"), Some(ProviderChoice::Anthropic));
        assert_eq!(ProviderChoice::parse("azure-openai"), Some(ProviderChoice::Azure));
        assert_eq!(
            ProviderChoice::parse("github-copilot"),
            Some(ProviderChoice::GithubCopilot)
        );
        assert_eq!(ProviderChoice::parse("custom"), Some(ProviderChoice::Custom));
        assert_eq!(ProviderChoice::parse("nope"), None);
    }
}
