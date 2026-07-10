//! Convenience constructors for [`providers::LLMProvider`] instances.
//!
//! The CLI needs to build a concrete provider from environment variables
//! (or `--provider` flags). Keeping this in a dedicated module means each
//! subcommand just calls [`default_provider`] and doesn't care which
//! backend was selected.
//!
//! Mirrors Python's ``nanobot.providers.factory``.

use std::sync::Arc;

use providers::{
    Backend, LLMProvider, ProviderBuildConfig, build_provider, env_api_base, env_api_key,
    env_region, find_by_name,
};

/// Which backend the CLI should instantiate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderChoice {
    OpenAI,
    Anthropic,
    Azure,
    Bedrock,
    GithubCopilot,
    Custom,
}

impl ProviderChoice {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.to_ascii_lowercase().as_str() {
            "openai" => Some(Self::OpenAI),
            "anthropic" | "claude" => Some(Self::Anthropic),
            "azure" | "azure_openai" | "azure-openai" => Some(Self::Azure),
            "bedrock" | "aws_bedrock" | "aws-bedrock" => Some(Self::Bedrock),
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
        .or_else(|| {
            std::env::var("PROVIDER")
                .ok()
                .and_then(|s| ProviderChoice::parse(&s))
        })
        .unwrap_or(ProviderChoice::OpenAI);

    match choice {
        ProviderChoice::OpenAI => {
            let model = model
                .or_else(|| std::env::var("MODEL").ok())
                .unwrap_or_else(|| "gpt-4o-mini".into());
            let api_key = std::env::var("OPENAI_API_KEY")
                .or_else(|_| std::env::var("API_KEY"))
                .ok();
            let cfg = ProviderBuildConfig {
                model,
                api_key,
                api_base: std::env::var("OPENAI_API_BASE").ok(),
                extra_headers: None,
                extra_body: None,
                region: None,
                profile: None,
            };
            build_provider(Backend::OpenAICompat, cfg)
        }
        ProviderChoice::Custom => {
            let model = model
                .or_else(|| std::env::var("MODEL").ok())
                .unwrap_or_else(|| "gpt-4o-mini".into());
            let api_key = std::env::var("OPENAI_API_KEY")
                .or_else(|_| std::env::var("API_KEY"))
                .ok();
            let api_base = std::env::var("API_BASE")
                .or_else(|_| std::env::var("OPENAI_API_BASE"))
                .ok();
            let cfg = ProviderBuildConfig {
                model,
                api_key,
                api_base,
                extra_headers: None,
                extra_body: None,
                region: None,
                profile: None,
            };
            build_provider(Backend::OpenAICompat, cfg)
        }
        ProviderChoice::Anthropic => {
            let api_key = std::env::var("ANTHROPIC_API_KEY")
                .map_err(|_| "ANTHROPIC_API_KEY is not set".to_string())?;
            let model = model.unwrap_or_else(|| "claude-3-5-sonnet-latest".into());
            let cfg = ProviderBuildConfig {
                model,
                api_key: Some(api_key),
                api_base: None,
                extra_headers: None,
                extra_body: None,
                region: None,
                profile: None,
            };
            build_provider(Backend::Anthropic, cfg)
        }
        ProviderChoice::Azure => {
            let api_key = std::env::var("AZURE_OPENAI_API_KEY")
                .map_err(|_| "AZURE_OPENAI_API_KEY is not set".to_string())?;
            let endpoint = std::env::var("AZURE_OPENAI_ENDPOINT")
                .map_err(|_| "AZURE_OPENAI_ENDPOINT is not set".to_string())?;
            let deployment = std::env::var("AZURE_OPENAI_DEPLOYMENT")
                .or_else(|_| std::env::var("AZURE_OPENAI_MODEL"))
                .map_err(|_| "AZURE_OPENAI_DEPLOYMENT is not set".to_string())?;
            let cfg = ProviderBuildConfig {
                model: deployment,
                api_key: Some(api_key),
                api_base: Some(endpoint),
                extra_headers: None,
                extra_body: None,
                region: None,
                profile: None,
            };
            build_provider(Backend::AzureOpenAI, cfg)
        }
        ProviderChoice::Bedrock => {
            let model = model.unwrap_or_else(|| "bedrock/global.anthropic.claude-opus-4-7".into());
            let cfg = ProviderBuildConfig {
                model,
                api_key: env_api_key("AWS_ACCESS_KEY_ID"),
                api_base: env_api_base("BEDROCK_API_BASE"),
                extra_headers: None,
                extra_body: None,
                region: env_region(),
                profile: None,
            };
            build_provider(Backend::Bedrock, cfg)
        }
        ProviderChoice::GithubCopilot => {
            let model = model.unwrap_or_else(|| "github-copilot/gpt-4.1".into());
            let cfg = ProviderBuildConfig {
                model,
                api_key: None,
                api_base: None,
                extra_headers: None,
                extra_body: None,
                region: None,
                profile: None,
            };
            build_provider(Backend::GitHubCopilot, cfg)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_choice_aliases() {
        assert_eq!(
            ProviderChoice::parse("openai"),
            Some(ProviderChoice::OpenAI)
        );
        assert_eq!(
            ProviderChoice::parse("Claude"),
            Some(ProviderChoice::Anthropic)
        );
        assert_eq!(
            ProviderChoice::parse("azure-openai"),
            Some(ProviderChoice::Azure)
        );
        assert_eq!(
            ProviderChoice::parse("bedrock"),
            Some(ProviderChoice::Bedrock)
        );
        assert_eq!(
            ProviderChoice::parse("aws-bedrock"),
            Some(ProviderChoice::Bedrock)
        );
        assert_eq!(
            ProviderChoice::parse("github-copilot"),
            Some(ProviderChoice::GithubCopilot)
        );
        assert_eq!(
            ProviderChoice::parse("custom"),
            Some(ProviderChoice::Custom)
        );
        assert_eq!(ProviderChoice::parse("nope"), None);
    }

    #[test]
    fn find_bedrock_spec() {
        let spec = find_by_name("bedrock");
        assert!(spec.is_some());
        assert_eq!(spec.unwrap().backend, Backend::Bedrock);
    }
}
