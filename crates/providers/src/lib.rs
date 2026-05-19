//! LLM provider abstractions shared by all agent code.
//!
//! Port of the non-network parts of `nanobot.providers.base`. Concrete
//! backends (Anthropic, OpenAI, ...) are intentionally left out of this
//! crate so downstream code depends only on a small, stable trait.

pub mod provider;
pub mod registry;
pub mod retry;
pub mod sanitize;
pub mod types;

pub mod anthropic;
pub mod azure_openai;
pub mod github_copilot;
pub mod oauth;
pub mod openai_codex;
pub mod openai_compat;
pub mod responses;
pub mod transcription;

pub use provider::{ChatRequest, LLMProvider, RetryMode, RetryWaitCallback};
pub use registry::{find_by_model, find_by_name, Backend, ProviderSpec, PROVIDERS};
pub use retry::{extract_retry_after_from_text, is_transient_response};
pub use sanitize::{enforce_role_alternation, sanitize_empty_content};
pub use types::{FinishReason, GenerationSettings, LLMResponse, ToolCallRequest, ToolChoice};

pub use anthropic::AnthropicProvider;
pub use azure_openai::AzureOpenAIProvider;
pub use github_copilot::{
    get_github_copilot_login_status, login_github_copilot, DeviceCodeInfo, DeviceFlow,
    GitHubCopilotProvider,
};
pub use oauth::{FileTokenStorage, OAuthToken};
pub use openai_codex::{OpenAICodexConfig, OpenAICodexProvider};
pub use openai_compat::OpenAICompatProvider;
pub use responses::{
    consume_events, convert_messages as convert_responses_messages,
    convert_tools as convert_responses_tools, parse_response_output, parse_sse_events,
};
pub use transcription::{
    GroqTranscriptionProvider, OpenAITranscriptionProvider, TranscriptionProvider,
};
