//! Utility functions for nanobot (Rust port of `nanobot.utils`).

pub mod artifacts;
pub mod document;
pub mod evaluator;
pub mod file_edit_events;
pub mod gitstore;
pub mod helpers;
pub mod image_generation_intent;
pub mod llm_runtime;
pub mod logging_bridge;
pub mod media_decode;
pub mod path;
pub mod progress_events;
pub mod prompt_templates;
pub mod restart;
pub mod runtime;
pub mod searchusage;
pub mod strings;
pub mod subagent_channel_display;
pub mod tool_hints;

pub use artifacts::{
    ArtifactError, MediaDirResolver, decode_image_data_url, generated_image_tool_result,
    store_generated_image_artifact,
};
pub use file_edit_events::{
    FileEditTracker, FileSnapshot, StreamingFileEditTracker, build_file_edit_end_event,
    build_file_edit_error_event, build_file_edit_live_event, build_file_edit_pending_event,
    build_file_edit_start_event, display_file_edit_path, is_file_edit_tool, line_diff_stats,
    prepare_file_edit_tracker, read_file_snapshot, resolve_file_edit_path,
};
pub use helpers::{
    StatusContent, TokenCounter, build_assistant_message, build_image_content_blocks,
    build_status_content, current_time_str, detect_image_mime, ensure_dir, estimate_message_tokens,
    estimate_prompt_tokens, estimate_prompt_tokens_chain, find_legal_message_start,
    image_placeholder_text, image_placeholder_text_with, maybe_persist_tool_result, safe_filename,
    split_message, stringify_text_blocks, strip_think, timestamp, truncate_text,
};
pub use image_generation_intent::image_generation_prompt;
pub use llm_runtime::{LLMRuntime, LLMRuntimeResolver, static_llm_runtime};
pub use logging_bridge::{LoguruBridge, redirect_lib_logging};
pub use path::{abbreviate_path, abbreviate_path_with_len};
pub use progress_events::{
    invoke_file_edit_progress, invoke_on_progress, on_progress_accepts_file_edit_events,
    on_progress_accepts_tool_events,
};
pub use strings::to_snake;
pub use subagent_channel_display::{
    scrub_subagent_announce_body, scrub_subagent_messages_for_channel,
};
