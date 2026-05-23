//! Slash command routing and built-in handlers
//! (Rust port of `nanobot.command`).
//!
//! Layout:
//! * [`router`] — [`CommandRouter`] plus [`CommandContext`].
//! * [`builtin`] — [`register_builtin_commands`] and the handlers it wires
//!   in.
//! * [`types`] — placeholder bridging types (`InboundMessage`,
//!   `OutboundMessage`, `Session`, `Loop`, …) to be replaced by real
//!   re-exports once the surrounding crates are ported.

pub mod builtin;
pub mod router;
pub mod types;

pub use builtin::{
    build_help_text, build_status_content, extract_changed_files, format_changed_files,
    format_dream_log_content, format_dream_restore_list, register_builtin_commands,
    set_restart_notice_to_env, NANOBOT_VERSION,
};
pub use router::{handler, CommandContext, CommandRouter, Handler};
pub use types::{
    Bus, Consolidator, DreamCommit, DreamGit, DreamRunner, InboundMessage, Loop, MemoryStore,
    OutboundMessage, ProviderGenerationView, Session, SessionManager, SubagentRegistry,
    TokenEstimate, WebConfigView, WebSearchView,
};
