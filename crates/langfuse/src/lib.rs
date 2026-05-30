//! Langfuse observability client for LLM tracing.
//!
//! Sends trace-create, generation-create, and generation-update events
//! to the Langfuse ingestion API (`POST /api/public/ingestion`)
//! asynchronously via a background task. All recording is non-blocking
//! and will silently drop events if the buffer is full.

mod client;
pub mod debug_log;

pub use client::{
    LangfuseClient, LangfuseConfig, TraceHandle, GenerationHandle, SpanHandle, ToolHandle,
    ObservationType, SpanLevel, TraceOptions, GenerationOptions, SpanOptions, EventOptions, ToolOptions,
    ScoreOptions, ScoreValueInput, GenerationEndOptions, ToolEndOptions,
};
