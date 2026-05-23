//! OpenAI-compatible HTTP API server.
//!
//! Rust port of `nanobot.api.server`. Exposes three routes:
//!
//! * `POST /v1/chat/completions` — accepts JSON *or* multipart/form-data,
//!   returns a Chat Completions response (optionally as SSE when
//!   `stream=true`).
//! * `GET  /v1/models`            — advertises the single configured model.
//! * `GET  /health`               — liveness probe.
//!
//! The server is deliberately decoupled from the agent runtime. Callers
//! provide any type implementing [`ApiAgent`]; this keeps the `api` crate
//! free of heavy dependencies and lets the CLI wire in the real
//! `AgentLoop` at the top level.

pub mod agent;
pub mod media;
pub mod server;
pub mod types;

pub use agent::{ApiAgent, ApiAnswer, ApiRequest, StreamSink};
pub use media::{save_base64_data_url, safe_filename, FileSizeExceeded, MAX_FILE_SIZE};
pub use server::{build_router, serve, ApiServerConfig, ServerState};
pub use types::{
    ChatCompletionChoice, ChatCompletionRequest, ChatCompletionResponse, ChatMessage, ModelInfo,
    ModelsList, Usage,
};
