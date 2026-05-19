//! Lightweight built-in slash-command pre-filter for the [`AgentLoop`].
//!
//! The full `command` crate exposes a rich router with placeholder `Loop`
//! / `Session` traits. Wiring the agent runtime through those adapters is
//! tracked separately; until then this module provides a self-contained
//! pre-filter that handles the most common commands end-to-end against
//! real agent state:
//!
//! * `/help` — list tools and available commands
//! * `/version` — print the crate version
//! * `/status` — show model, workspace, history length, context window
//! * `/clear` / `/new` — wipe session history
//! * `/model` — show or set the active model (in-memory only)
//!
//! Install with [`AgentLoop::set_command_prefilter`]:
//!
//! ```ignore
//! let prefilter = BuiltinPrefilter::new(
//!     sessions.clone(),
//!     tools.clone(),
//!     AgentRuntimeInfo {
//!         model: "gpt-4o".into(),
//!         workspace: cfg.workspace.clone(),
//!         context_window_tokens: cfg.context_window_tokens,
//!     },
//! );
//! loop_.set_command_prefilter(prefilter.into_hook());
//! ```

use std::path::PathBuf;
use std::sync::Arc;

use futures::FutureExt;
use tokio::sync::Mutex;

use bus::InboundMessage;
use session::manager::SessionManager;

use crate::loop_::CommandPrefilter;
use crate::tools::ToolRegistry;

/// Snapshot of runtime info surfaced by `/status` / `/model`.
#[derive(Clone, Debug)]
pub struct AgentRuntimeInfo {
    pub model: String,
    pub workspace: PathBuf,
    pub context_window_tokens: Option<u32>,
}

/// Built-in command pre-filter.
pub struct BuiltinPrefilter {
    sessions: Arc<Mutex<SessionManager>>,
    tools: ToolRegistry,
    info: Arc<tokio::sync::RwLock<AgentRuntimeInfo>>,
}

impl BuiltinPrefilter {
    pub fn new(
        sessions: Arc<Mutex<SessionManager>>,
        tools: ToolRegistry,
        info: AgentRuntimeInfo,
    ) -> Self {
        Self {
            sessions,
            tools,
            info: Arc::new(tokio::sync::RwLock::new(info)),
        }
    }

    /// Box this prefilter into the [`CommandPrefilter`] hook expected by
    /// [`AgentLoop::set_command_prefilter`].
    pub fn into_hook(self) -> CommandPrefilter {
        let sessions = self.sessions;
        let tools = self.tools;
        let info = self.info;
        Arc::new(move |msg: &InboundMessage| {
            let sessions = sessions.clone();
            let tools = tools.clone();
            let info = info.clone();
            let msg = msg.clone();
            async move { dispatch(&msg, &sessions, &tools, &info).await }.boxed()
        })
    }
}

async fn dispatch(
    msg: &InboundMessage,
    sessions: &Arc<Mutex<SessionManager>>,
    tools: &ToolRegistry,
    info: &Arc<tokio::sync::RwLock<AgentRuntimeInfo>>,
) -> Option<String> {
    let text = msg.content.trim();
    if !text.starts_with('/') {
        return None;
    }
    // Split into (cmd, args). Command is case-insensitive.
    let (cmd_raw, args) = match text.split_once(char::is_whitespace) {
        Some((c, a)) => (c, a.trim()),
        None => (text, ""),
    };
    let cmd = cmd_raw.to_lowercase();

    match cmd.as_str() {
        "/help" | "/?" => Some(help_text(tools).await),
        "/version" => Some(format!(
            "RustBot agent v{} (crate: agent)",
            env!("CARGO_PKG_VERSION")
        )),
        "/status" => Some(status_text(sessions, info, &msg.session_key()).await),
        "/clear" | "/new" => Some(clear_session(sessions, &msg.session_key()).await),
        "/model" => Some(model_cmd(info, args).await),
        _ => None,
    }
}

async fn help_text(tools: &ToolRegistry) -> String {
    let mut names = tools.tool_names().await;
    names.sort();
    let mut out = String::from("Available commands:\n");
    out.push_str("  /help             show this help\n");
    out.push_str("  /version          print agent version\n");
    out.push_str("  /status           show runtime status\n");
    out.push_str("  /clear | /new     clear current session history\n");
    out.push_str("  /model [name]     show or set active model\n");
    out.push_str("\nAvailable tools:\n");
    if names.is_empty() {
        out.push_str("  (none registered)\n");
    } else {
        for n in names {
            out.push_str(&format!("  {n}\n"));
        }
    }
    out
}

async fn status_text(
    sessions: &Arc<Mutex<SessionManager>>,
    info: &Arc<tokio::sync::RwLock<AgentRuntimeInfo>>,
    session_key: &str,
) -> String {
    let history_len = {
        let mut mgr = sessions.lock().await;
        let s = mgr.get_or_create(session_key);
        s.messages.len()
    };
    let snap = info.read().await.clone();
    let ctx_window = snap
        .context_window_tokens
        .map(|n| n.to_string())
        .unwrap_or_else(|| "<provider default>".into());
    format!(
        "Status:\n  model:             {}\n  workspace:         {}\n  context window:    {}\n  session:           {}\n  history length:    {}",
        snap.model,
        snap.workspace.display(),
        ctx_window,
        session_key,
        history_len,
    )
}

async fn clear_session(
    sessions: &Arc<Mutex<SessionManager>>,
    session_key: &str,
) -> String {
    let mut mgr = sessions.lock().await;
    let mut s = mgr.get_or_create(session_key);
    let removed = s.messages.len();
    s.clear();
    match mgr.save(s, false) {
        Ok(()) => format!("Cleared session {session_key} ({removed} messages)."),
        Err(e) => format!(
            "Cleared in-memory session {session_key} ({removed} messages) but failed to persist: {e}"
        ),
    }
}

async fn model_cmd(
    info: &Arc<tokio::sync::RwLock<AgentRuntimeInfo>>,
    args: &str,
) -> String {
    if args.is_empty() {
        let snap = info.read().await;
        return format!("Current model: {}", snap.model);
    }
    let mut snap = info.write().await;
    let old = std::mem::replace(&mut snap.model, args.to_string());
    format!("Model changed: {old} -> {}", snap.model)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn sample_msg(text: &str) -> InboundMessage {
        InboundMessage {
            channel: "test".into(),
            chat_id: "c1".into(),
            sender_id: "u1".into(),
            content: text.into(),
            ..Default::default()
        }
    }

    fn unique_workspace(tag: &str) -> PathBuf {
        let ws = std::env::temp_dir().join(format!("rustbot_prefilter_{tag}"));
        let _ = std::fs::remove_dir_all(&ws);
        std::fs::create_dir_all(&ws).unwrap();
        ws
    }

    fn setup(tag: &str) -> (Arc<Mutex<SessionManager>>, ToolRegistry, AgentRuntimeInfo) {
        let ws = unique_workspace(tag);
        let mgr = SessionManager::new(ws.clone());
        let sessions = Arc::new(Mutex::new(mgr));
        let tools = ToolRegistry::new();
        let info = AgentRuntimeInfo {
            model: "test-model".into(),
            workspace: ws,
            context_window_tokens: Some(128_000),
        };
        (sessions, tools, info)
    }

    #[tokio::test]
    async fn non_slash_passes_through() {
        let (sessions, tools, info) = setup("passthrough");
        let info = Arc::new(tokio::sync::RwLock::new(info));
        let out = dispatch(&sample_msg("hello world"), &sessions, &tools, &info).await;
        assert!(out.is_none());
    }

    #[tokio::test]
    async fn help_lists_tools_and_commands() {
        let (sessions, tools, info) = setup("help");
        let info = Arc::new(tokio::sync::RwLock::new(info));
        let out = dispatch(&sample_msg("/help"), &sessions, &tools, &info).await.unwrap();
        assert!(out.contains("/help"));
        assert!(out.contains("/clear"));
        assert!(out.contains("Available tools"));
    }

    #[tokio::test]
    async fn version_returns_pkg_version() {
        let (sessions, tools, info) = setup("version");
        let info = Arc::new(tokio::sync::RwLock::new(info));
        let out = dispatch(&sample_msg("/VERSION"), &sessions, &tools, &info).await.unwrap();
        assert!(out.contains(env!("CARGO_PKG_VERSION")));
    }

    #[tokio::test]
    async fn status_reports_model_and_history() {
        let (sessions, tools, info) = setup("status");
        let info = Arc::new(tokio::sync::RwLock::new(info));
        let msg = sample_msg("/status");
        {
            let mut mgr = sessions.lock().await;
            let mut s = mgr.get_or_create(&msg.session_key());
            s.add_message("user", "hi", HashMap::new());
            mgr.save(s, false).unwrap();
        }
        let out = dispatch(&msg, &sessions, &tools, &info).await.unwrap();
        assert!(out.contains("test-model"));
        assert!(out.contains("history length:"));
    }

    #[tokio::test]
    async fn clear_wipes_history() {
        let (sessions, tools, info) = setup("clear");
        let info = Arc::new(tokio::sync::RwLock::new(info));
        let msg = sample_msg("/clear");
        {
            let mut mgr = sessions.lock().await;
            let mut s = mgr.get_or_create(&msg.session_key());
            s.add_message("user", "a", HashMap::new());
            s.add_message("assistant", "b", HashMap::new());
            mgr.save(s, false).unwrap();
        }
        let out = dispatch(&msg, &sessions, &tools, &info).await.unwrap();
        assert!(out.starts_with("Cleared session"));
        let mut mgr = sessions.lock().await;
        let s = mgr.get_or_create(&msg.session_key());
        assert_eq!(s.messages.len(), 0);
    }

    #[tokio::test]
    async fn model_cmd_reads_and_writes() {
        let (sessions, tools, info) = setup("model");
        let info = Arc::new(tokio::sync::RwLock::new(info));
        let out = dispatch(&sample_msg("/model"), &sessions, &tools, &info).await.unwrap();
        assert!(out.contains("test-model"));
        let out = dispatch(&sample_msg("/model gpt-4o-mini"), &sessions, &tools, &info)
            .await
            .unwrap();
        assert!(out.contains("gpt-4o-mini"));
        let out = dispatch(&sample_msg("/model"), &sessions, &tools, &info).await.unwrap();
        assert!(out.contains("gpt-4o-mini"));
    }

    #[tokio::test]
    async fn unknown_slash_returns_none() {
        let (sessions, tools, info) = setup("unknown");
        let info = Arc::new(tokio::sync::RwLock::new(info));
        let out = dispatch(&sample_msg("/nope"), &sessions, &tools, &info).await;
        assert!(out.is_none());
    }
}
