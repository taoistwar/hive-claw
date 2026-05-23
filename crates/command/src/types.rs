//! Placeholder types bridging into yet-to-be-ported modules.
//!
//! The Python `nanobot.command` package imports types from `nanobot.bus`,
//! `nanobot.session`, and interacts with an agent `Loop` object. Those
//! modules have not been ported yet, so we declare minimal analogues here
//! so the command layer can compile and be unit-tested today. When the real
//! crates arrive, these definitions will be replaced by re-exports.
//!
//! Everything exposed from this file is intentionally conservative — only
//! the fields/methods actually used by `router.rs` / `builtin.rs` are
//! declared.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use futures::future::BoxFuture;

// ---------------------------------------------------------------------------
// Bus events (re-exported from the `bus` crate)
// ---------------------------------------------------------------------------

pub use bus::{InboundMessage, OutboundMessage};

// ---------------------------------------------------------------------------
// Session
// ---------------------------------------------------------------------------

/// Handle to a conversation session (mirrors `nanobot.session.manager.Session`).
#[async_trait]
pub trait Session: Send + Sync {
    fn key(&self) -> &str;
    fn last_consolidated(&self) -> usize;
    /// Number of messages currently in the history (used for `/status`).
    fn history_len(&self) -> usize;
    /// Return the last `max_messages` messages as JSON values.
    /// If `max_messages` is 0, return all messages.
    fn get_history(&self, max_messages: usize) -> Vec<serde_json::Value>;
    /// Drain messages on top of `last_consolidated` and return them as a
    /// snapshot (used by `/new`).
    fn drain_after_consolidation(&self) -> Vec<serde_json::Value>
    where
        Self: Sized,
    {
        Vec::new()
    }
    /// Clear all in-memory messages; a fresh session state is retained.
    fn clear(&self);
}

// ---------------------------------------------------------------------------
// Dream memory git helpers
// ---------------------------------------------------------------------------

/// One entry of the Dream memory commit log.
#[derive(Debug, Clone)]
pub struct DreamCommit {
    pub sha: String,
    pub timestamp: String,
    pub message: String,
}

/// Git-backed store wrapping the Dream memory directory
/// (mirrors `consolidator.store.git` in the Python port).
pub trait DreamGit: Send + Sync {
    fn is_initialized(&self) -> bool;
    fn show_commit_diff(&self, sha: &str) -> Option<(DreamCommit, String)>;
    fn log(&self, max_entries: usize) -> Vec<DreamCommit>;
    fn revert(&self, sha: &str) -> Option<String>;
}

/// Memory store backing the consolidator (narrow slice used by commands).
pub trait MemoryStore: Send + Sync {
    fn git(&self) -> Arc<dyn DreamGit>;
    fn get_last_dream_cursor(&self) -> usize;
}

// ---------------------------------------------------------------------------
// Consolidator / web config / providers (placeholder views)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct TokenEstimate {
    pub tokens: u64,
    /// Mirrors the second element of the Python tuple return; exact meaning
    /// is not needed by the command layer so it is treated as opaque.
    pub details: Option<serde_json::Value>,
}

pub trait Consolidator: Send + Sync {
    fn estimate_session_prompt_tokens(&self, session: &Arc<dyn Session>) -> TokenEstimate;
    /// Archive a drained message snapshot. The returned future is spawned
    /// via [`Loop::schedule_background`] in the Python port.
    fn archive(&self, snapshot: Vec<serde_json::Value>) -> BoxFuture<'static, ()>;
    fn store(&self) -> Arc<dyn MemoryStore>;
}

#[derive(Debug, Clone, Default)]
pub struct WebSearchView {
    pub provider: String,
    pub api_key: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct WebConfigView {
    pub search: Option<WebSearchView>,
}

/// Minimal view over the active provider used by `/status`.
#[derive(Debug, Clone, Default)]
pub struct ProviderGenerationView {
    pub max_tokens: u32,
}

// ---------------------------------------------------------------------------
// Session manager
// ---------------------------------------------------------------------------

pub trait SessionManager: Send + Sync {
    fn get_or_create(&self, key: &str) -> Arc<dyn Session>;
    fn save(&self, session: &Arc<dyn Session>);
    fn invalidate(&self, key: &str);
}

// ---------------------------------------------------------------------------
// Dream runner / bus
// ---------------------------------------------------------------------------

#[async_trait]
pub trait DreamRunner: Send + Sync {
    /// Returns `true` if real work was performed.
    async fn run(&self) -> bool;
}

#[async_trait]
pub trait Bus: Send + Sync {
    async fn publish_outbound(&self, msg: OutboundMessage);
}

#[async_trait]
pub trait SubagentRegistry: Send + Sync {
    fn get_running_count_by_session(&self, key: &str) -> usize;
}

// ---------------------------------------------------------------------------
// Loop — the god-object handlers talk to
// ---------------------------------------------------------------------------

/// Abstract handle used by the built-in commands to reach into the agent
/// loop's internals. Mirrors the subset of the Python `Loop` surface that
/// `builtin.py` relies on; the concrete impl will be provided by the
/// runtime crate once ported.
#[async_trait]
pub trait Loop: Send + Sync {
    // Simple "scalar" accessors -------------------------------------------
    fn model(&self) -> String;
    fn start_time(&self) -> f64;
    fn last_usage(&self) -> HashMap<String, u64>;
    fn context_window_tokens(&self) -> u32;
    fn provider_generation(&self) -> ProviderGenerationView;
    fn web_config(&self) -> Option<WebConfigView>;

    // Model presets -------------------------------------------------------
    /// Set of configured model preset names (e.g. "fast", "smart").
    fn model_presets(&self) -> std::collections::HashSet<String>;
    /// Currently active preset name, or `"default"` if none selected.
    fn model_preset(&self) -> String;
    /// Switch to a named preset. Returns an error string on failure.
    fn set_model_preset(&self, name: &str) -> Result<(), String>;

    // Sub-object accessors -------------------------------------------------
    fn sessions(&self) -> Arc<dyn SessionManager>;
    fn consolidator(&self) -> Arc<dyn Consolidator>;
    fn dream(&self) -> Arc<dyn DreamRunner>;
    fn bus(&self) -> Arc<dyn Bus>;
    fn subagents(&self) -> Arc<dyn SubagentRegistry>;

    // Task management ------------------------------------------------------

    /// Cancel all active tasks and subagents for *session_key*; returns how
    /// many were cancelled.
    async fn cancel_active_tasks(&self, session_key: &str) -> usize;

    /// Non-done active task count for `/status`.
    fn active_task_count(&self, session_key: &str) -> usize;

    /// Equivalent of `loop._schedule_background`. The default impl uses
    /// `tokio::spawn` when a runtime is available but the Python version
    /// simply calls `asyncio.create_task`. Implementors may override to
    /// route through their own task bookkeeping.
    fn schedule_background(&self, fut: BoxFuture<'static, ()>) {
        // Spawn via tokio if a runtime handle is accessible; otherwise drop
        // the future with a warning so tests without a runtime still run.
        match tokio_spawn(fut) {
            Ok(()) => {}
            Err(_) => log::warn!(
                "schedule_background: no tokio runtime active; dropping background task"
            ),
        }
    }
}

fn tokio_spawn(_fut: BoxFuture<'static, ()>) -> Result<(), ()> {
    // Intentionally a no-op shim: the command crate itself does not pull in
    // tokio as a runtime dep. Real `Loop` implementors will override
    // `schedule_background` with their own spawner (typically
    // `tokio::spawn(fut);`).
    Err(())
}
