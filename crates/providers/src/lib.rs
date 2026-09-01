//! LLM provider abstractions shared by all agent code.
//!
//! Port of the non-network parts of `nanobot.providers.base`. Concrete
//! backends (Anthropic, OpenAI, ...) are intentionally left out of this
//! crate so downstream code depends only on a small, stable trait.

pub mod base;
pub mod registry;

pub mod anthropic_provider;
pub mod azure_openai_provider;
pub mod bedrock_provider;
pub mod factory;
pub mod fallback_provider;
pub mod github_copilot_provider;
pub mod image_generation;
pub mod oauth;
pub mod openai_codex_provider;
pub mod openai_compat_provider;
pub mod responses;
pub mod transcription;

pub use base::{
    ChatRequest, FallbackReason, FallbackTransition, FallbackTransitionCallback, FinishReason,
    GenerationSettings, LLMProvider, LLMResponse, LlmCallOptions, RetryMode, RetryWaitCallback,
    StreamDeltaCallback, ToolCallRequest, ToolChoice, enforce_role_alternation,
    extract_retry_after_from_text, is_transient_response, pick_delay, sanitize_empty_content,
    strip_image_content, strip_image_content_inplace,
};
pub use registry::{Backend, PROVIDERS, ProviderSpec, find_by_model, find_by_name};

pub use anthropic_provider::AnthropicProvider;
pub use azure_openai_provider::AzureOpenAIProvider;
pub use bedrock_provider::BedrockProvider;
pub use factory::{
    ProviderBuildConfig, build_provider, detect_backend_and_build, env_api_base, env_api_key,
    env_region, make_provider, provider_config_for, resolve_spec,
};
pub use fallback_provider::{FallbackProvider, FallbackTarget};
pub use github_copilot_provider::{
    DeviceCodeInfo, DeviceFlow, GitHubCopilotProvider, get_github_copilot_login_status,
    login_github_copilot,
};
pub use image_generation::{
    AIHubMixImageGenerationClient, GeminiImageGenerationClient, GeneratedImageResponse,
    ImageGenerationError, ImageGenerationProvider, MiniMaxImageGenerationClient,
    OpenRouterImageGenerationClient, StepFunImageGenerationClient, get_image_gen_provider,
    image_gen_provider_names, register_all_image_gen_providers, register_image_gen_provider,
};
pub use oauth::{FileTokenStorage, OAuthToken};
pub use openai_codex_provider::{OpenAICodexConfig, OpenAICodexProvider};
pub use openai_compat_provider::OpenAICompatProvider;
pub use responses::{
    ContentDeltaCallback, ToolCallDeltaCallback, consume_events, consume_sse,
    convert_messages as convert_responses_messages, convert_tools as convert_responses_tools,
    parse_response_output, parse_sse_events,
};
pub use transcription::{
    GroqTranscriptionProvider, OpenAITranscriptionProvider, TranscriptionProvider,
};
