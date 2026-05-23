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

pub mod commands;
pub mod models;
pub mod onboard;
pub mod stream;

pub use commands::{Cli, Command, dispatch};
pub use commands::{LoopBundle, Runtime, migrate_cron_store, expand_tilde};
pub use commands::{AgentArgs, GatewayArgs};
pub use commands::{ProviderChoice, default_provider};
pub use models::{find_model_info, format_token_count, get_all_models, get_model_context_limit, get_model_suggestions};
pub use stream::{PauseGuard, StreamRenderer, ThinkingSpinner};
