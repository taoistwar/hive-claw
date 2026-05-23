//! OpenAI-compatible HTTP API server.
//!
//! Rust port of `nanobot.api.server`. Exposes three routes:
//!
//! * `POST /v1/chat/completions` — accepts JSON *or* multipart/form-data,
//!   returns a Chat Completions response (optionally as SSE when
//!   `stream=true`).
//! * `GET  /v1/models`            — advertises the single configured model.
//! * `GET  /health`               — liveness probe.

pub mod server;

pub use server::{
    ApiAgent, ApiAnswer, ApiRequest, ChatCompletionChoice, ChatCompletionRequest,
    ChatCompletionResponse, ChatMessage, FileSizeExceeded, ModelInfo, ModelsList, StreamSink,
    Usage, assistant_completion, build_router, save_base64_data_url, safe_filename, serve,
    ApiServerConfig, ServerState, MAX_FILE_SIZE,
};
