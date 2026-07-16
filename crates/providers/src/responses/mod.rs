//! OpenAI Responses API helpers — converters + SSE stream parsing.
//!
//! Port of `nanobot.providers.openai_responses`. Used by the Codex
//! provider and any future Responses-API-backed backend.

pub mod converters;
pub mod parsing;

pub use converters::{
    convert_messages, convert_tools, convert_user_message, map_finish_reason, split_tool_call_id,
};
pub use parsing::{
    ContentDeltaCallback, ToolCallDeltaCallback, consume_events, consume_sdk_stream, consume_sse,
    parse_response_output, parse_sse_events,
};
