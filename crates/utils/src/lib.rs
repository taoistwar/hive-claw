//! Utility functions for nanobot (Rust port of `nanobot.utils`).

pub mod document;
pub mod evaluator;
pub mod gitstore;
pub mod helpers;
pub mod media_decode;
pub mod path;
pub mod prompt_templates;
pub mod restart;
pub mod runtime;
pub mod searchusage;
pub mod strings;
pub mod tool_hints;

pub use helpers::{
    StatusContent, TokenCounter, build_assistant_message, build_image_content_blocks,
    build_status_content, current_time_str, detect_image_mime, ensure_dir, estimate_message_tokens,
    estimate_prompt_tokens, estimate_prompt_tokens_chain, find_legal_message_start,
    image_placeholder_text, image_placeholder_text_with, maybe_persist_tool_result, safe_filename,
    split_message, stringify_text_blocks, strip_think, timestamp, truncate_text,
};
pub use path::{abbreviate_path, abbreviate_path_with_len};
pub use strings::to_snake;