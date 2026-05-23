//! Rust port of `nanobot.cli`.
//!
//! Implemented commands:
//! * `version`, `onboard`, `status`
//! * `agent` (single-shot or REPL), `run` / `chat` (compat aliases)
//! * `serve` (OpenAI-compatible HTTP API)
//! * `gateway` (cron + heartbeat + bus loop + `/health`)
//! * `channels status` / `channels login`
//! * `plugins list`
//! * `provider login github-copilot`, `provider login openai-codex` (delegates
//!   to upstream `codex` CLI), `provider status …`
//!
//! What's still deferred:
//! * the interactive Python onboarding wizard
//! * Rich / prompt_toolkit terminal UI (we use plain text + rustyline)
//! * channel adapter implementations (the `channels` crate is empty)

pub mod adapter;
pub mod agent_cmd;
pub mod commands;
pub mod gateway;
pub mod models;
pub mod onboard;
pub mod provider_choice;
pub mod runtime;
pub mod status;
pub mod stream;

pub use adapter::AgentLoopApi;
pub use commands::{Cli, Command, dispatch};
pub use models::{find_model_info, format_token_count, get_all_models, get_model_context_limit, get_model_suggestions};
pub use provider_choice::{ProviderChoice, default_provider};
pub use runtime::{LoopBundle, Runtime, migrate_cron_store};
pub use stream::{PauseGuard, StreamRenderer, ThinkingSpinner};
