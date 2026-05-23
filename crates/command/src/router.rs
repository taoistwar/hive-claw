//! Minimal command routing table for slash commands (ported from
//! `nanobot/command/router.py`).
//!
//! Three tiers, checked in order:
//!   1. `priority` — exact-match commands handled before the dispatch lock
//!      (e.g. `/stop`, `/restart`).
//!   2. `exact` — exact-match commands handled inside the dispatch lock.
//!   3. `prefix` — longest-prefix-first match (e.g. `"/team "`).
//!   4. `interceptors` — fallback predicates (e.g. team-mode active check).

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use futures::future::BoxFuture;

// ===================================================================
// Bus events (from Python nanobot.bus.events)
// ===================================================================

/// A message inbound from a channel.
#[derive(Debug, Clone, Default)]
pub struct InboundMessage {
    pub channel: String,
    pub chat_id: String,
    pub session_key: String,
    pub content: String,
    pub metadata: HashMap<String, String>,
}

/// A message going out to a channel.
#[derive(Debug, Clone, Default)]
pub struct OutboundMessage {
    pub channel: String,
    pub chat_id: String,
    pub content: String,
    pub metadata: HashMap<String, String>,
}

// ===================================================================
// Session (from Python nanobot.session.manager)
// ===================================================================

/// Handle to a conversation session.
#[async_trait]
pub trait Session: Send + Sync {
    fn key(&self) -> &str;
    fn last_consolidated(&self) -> usize;
    fn history_len(&self) -> usize;
    fn drain_after_consolidation(&self) -> Vec<serde_json::Value>
    where
        Self: Sized,
    {
        Vec::new()
    }
    fn clear(&self);
}

// ===================================================================
// Dream memory git helpers
// ===================================================================

/// One entry of the Dream memory commit log.
#[derive(Debug, Clone)]
pub struct DreamCommit {
    pub sha: String,
    pub timestamp: String,
    pub message: String,
}

/// Git-backed store wrapping the Dream memory directory.
pub trait DreamGit: Send + Sync {
    fn is_initialized(&self) -> bool;
    fn show_commit_diff(&self, sha: &str) -> Option<(DreamCommit, String)>;
    fn log(&self, max_entries: usize) -> Vec<DreamCommit>;
    fn revert(&self, sha: &str) -> Option<String>;
}

/// Memory store backing the consolidator.
pub trait MemoryStore: Send + Sync {
    fn git(&self) -> Arc<dyn DreamGit>;
    fn get_last_dream_cursor(&self) -> usize;
}

// ===================================================================
// Consolidator / web config / providers (placeholder views)
// ===================================================================

#[derive(Debug, Clone, Default)]
pub struct TokenEstimate {
    pub tokens: u64,
    pub details: Option<serde_json::Value>,
}

pub trait Consolidator: Send + Sync {
    fn estimate_session_prompt_tokens(&self, session: &Arc<dyn Session>) -> TokenEstimate;
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

// ===================================================================
// Session manager
// ===================================================================

pub trait SessionManager: Send + Sync {
    fn get_or_create(&self, key: &str) -> Arc<dyn Session>;
    fn save(&self, session: &Arc<dyn Session>);
    fn invalidate(&self, key: &str);
}

// ===================================================================
// Dream runner / bus
// ===================================================================

#[async_trait]
pub trait DreamRunner: Send + Sync {
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

fn tokio_spawn(_fut: BoxFuture<'static, ()>) -> Result<(), ()> {
    Err(())
}

// ===================================================================
// Loop — the god-object handlers talk to
// ===================================================================

/// Abstract handle used by the built-in commands to reach into the agent
/// loop's internals.
#[async_trait]
pub trait Loop: Send + Sync {
    fn model(&self) -> String;
    fn start_time(&self) -> f64;
    fn last_usage(&self) -> HashMap<String, u64>;
    fn context_window_tokens(&self) -> u32;
    fn provider_generation(&self) -> ProviderGenerationView;
    fn web_config(&self) -> Option<WebConfigView>;

    fn sessions(&self) -> Arc<dyn SessionManager>;
    fn consolidator(&self) -> Arc<dyn Consolidator>;
    fn dream(&self) -> Arc<dyn DreamRunner>;
    fn bus(&self) -> Arc<dyn Bus>;
    fn subagents(&self) -> Arc<dyn SubagentRegistry>;

    async fn cancel_active_tasks(&self, session_key: &str) -> usize;
    fn active_task_count(&self, session_key: &str) -> usize;

    fn schedule_background(&self, fut: BoxFuture<'static, ()>) {
        match tokio_spawn(fut) {
            Ok(()) => {}
            Err(_) => log::warn!(
                "schedule_background: no tokio runtime active; dropping background task"
            ),
        }
    }
}

/// Everything a command handler needs to produce a response.
///
/// Lifetimes on the contained sub-objects are erased to `Arc<dyn …>` to
/// mirror Python's reference semantics. `loop_` uses a trailing underscore
/// because `loop` is a Rust keyword.
pub struct CommandContext {
    pub msg: InboundMessage,
    pub session: Option<Arc<dyn Session>>,
    pub key: String,
    pub raw: String,
    pub args: String,
    pub loop_: Option<Arc<dyn Loop>>,
}

impl CommandContext {
    pub fn new(msg: InboundMessage, key: impl Into<String>, raw: impl Into<String>) -> Self {
        Self {
            msg,
            session: None,
            key: key.into(),
            raw: raw.into(),
            args: String::new(),
            loop_: None,
        }
    }

    pub fn with_session(mut self, session: Arc<dyn Session>) -> Self {
        self.session = Some(session);
        self
    }

    pub fn with_loop(mut self, loop_: Arc<dyn Loop>) -> Self {
        self.loop_ = Some(loop_);
        self
    }
}

/// A command handler is an `Arc` wrapping an async closure that takes a
/// mutable reference to the context and returns an optional outbound
/// message.
pub type Handler = Arc<
    dyn for<'a> Fn(&'a mut CommandContext) -> BoxFuture<'a, Option<OutboundMessage>>
        + Send
        + Sync,
>;

/// Helper to box a concrete async function into a [`Handler`].
///
/// Usage:
/// ```ignore
/// fn my_cmd<'a>(ctx: &'a mut CommandContext)
///     -> BoxFuture<'a, Option<OutboundMessage>>
/// {
///     Box::pin(async move { /* … */ None })
/// }
/// let handler = command::handler(my_cmd);
/// ```
pub fn handler<F>(f: F) -> Handler
where
    F: for<'a> Fn(&'a mut CommandContext) -> BoxFuture<'a, Option<OutboundMessage>>
        + Send
        + Sync
        + 'static,
{
    Arc::new(f)
}

/// Pure dict-based command dispatch.
#[derive(Default, Clone)]
pub struct CommandRouter {
    priority: HashMap<String, Handler>,
    exact: HashMap<String, Handler>,
    /// Prefix table, kept sorted by descending prefix length so `dispatch`
    /// matches the longest prefix first (mirrors the Python sort in
    /// `prefix()`).
    prefix: Vec<(String, Handler)>,
    interceptors: Vec<Handler>,
}

impl CommandRouter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn priority(&mut self, cmd: impl Into<String>, handler: Handler) {
        self.priority.insert(cmd.into(), handler);
    }

    pub fn exact(&mut self, cmd: impl Into<String>, handler: Handler) {
        self.exact.insert(cmd.into(), handler);
    }

    pub fn prefix(&mut self, pfx: impl Into<String>, handler: Handler) {
        self.prefix.push((pfx.into(), handler));
        self.prefix
            .sort_by(|a, b| b.0.len().cmp(&a.0.len()));
    }

    pub fn intercept(&mut self, handler: Handler) {
        self.interceptors.push(handler);
    }

    /// Would `dispatch_priority` match? Case-insensitive, trimmed.
    pub fn is_priority(&self, text: &str) -> bool {
        self.priority.contains_key(&normalize(text))
    }

    /// Check whether `text` matches any non-priority command tier
    /// (exact or prefix). Does NOT check priority or interceptor tiers. If
    /// this returns `true`, [`dispatch`](Self::dispatch) is guaranteed to
    /// match a handler.
    pub fn is_dispatchable_command(&self, text: &str) -> bool {
        let cmd = normalize(text);
        if self.exact.contains_key(&cmd) {
            return true;
        }
        self.prefix.iter().any(|(p, _)| cmd.starts_with(p))
    }

    /// Dispatch a priority command. Called from `run()` without the lock.
    pub async fn dispatch_priority(
        &self,
        ctx: &mut CommandContext,
    ) -> Option<OutboundMessage> {
        let key = ctx.raw.to_lowercase();
        let handler = self.priority.get(&key)?.clone();
        handler(ctx).await
    }

    /// Try exact, prefix, then interceptors. Returns `None` if unhandled.
    pub async fn dispatch(&self, ctx: &mut CommandContext) -> Option<OutboundMessage> {
        let cmd = ctx.raw.to_lowercase();

        if let Some(handler) = self.exact.get(&cmd).cloned() {
            return handler(ctx).await;
        }

        // Prefix tier: scan longest-first (the Vec is kept sorted).
        for (pfx, handler) in self.prefix.iter() {
            if cmd.starts_with(pfx) {
                // Split the original `raw` rather than the lowercased copy
                // so argument casing is preserved.
                ctx.args = ctx.raw[pfx.len()..].to_string();
                let handler = handler.clone();
                return handler(ctx).await;
            }
        }

        for interceptor in self.interceptors.iter() {
            let interceptor = interceptor.clone();
            if let Some(out) = interceptor(ctx).await {
                return Some(out);
            }
        }

        None
    }
}

fn normalize(text: &str) -> String {
    text.trim().to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_ctx(raw: &str) -> CommandContext {
        CommandContext::new(InboundMessage::default(), "k", raw)
    }

    fn echo(name: &'static str) -> Handler {
        handler(move |ctx| {
            let content = format!("{}:{}", name, ctx.args);
            Box::pin(async move {
                Some(OutboundMessage {
                    content,
                    ..Default::default()
                })
            })
        })
    }

    #[tokio::test]
    async fn exact_and_prefix_dispatch() {
        let mut router = CommandRouter::new();
        router.exact("/help", echo("help"));
        router.prefix("/team ", echo("team"));

        let mut ctx = mk_ctx("/help");
        assert_eq!(
            router.dispatch(&mut ctx).await.unwrap().content,
            "help:"
        );

        let mut ctx = mk_ctx("/team foo bar");
        let out = router.dispatch(&mut ctx).await.unwrap();
        assert_eq!(out.content, "team:foo bar");
        assert_eq!(ctx.args, "foo bar");
    }

    #[tokio::test]
    async fn longest_prefix_wins() {
        let mut router = CommandRouter::new();
        router.prefix("/dream-log ", echo("log"));
        router.prefix("/dream ", echo("dream"));

        let mut ctx = mk_ctx("/dream-log abc");
        assert_eq!(
            router.dispatch(&mut ctx).await.unwrap().content,
            "log:abc"
        );
    }

    #[tokio::test]
    async fn priority_tier_separate_from_dispatch() {
        let mut router = CommandRouter::new();
        router.priority("/stop", echo("stop"));
        assert!(router.is_priority("  /STOP  "));
        assert!(!router.is_dispatchable_command("/stop"));
        let mut ctx = mk_ctx("/stop");
        assert_eq!(
            router.dispatch_priority(&mut ctx).await.unwrap().content,
            "stop:"
        );
    }

    #[tokio::test]
    async fn interceptors_run_last() {
        let mut router = CommandRouter::new();
        router.intercept(handler(|_ctx| {
            Box::pin(async {
                Some(OutboundMessage {
                    content: "intercepted".into(),
                    ..Default::default()
                })
            })
        }));
        let mut ctx = mk_ctx("anything");
        assert_eq!(
            router.dispatch(&mut ctx).await.unwrap().content,
            "intercepted"
        );
    }
}
