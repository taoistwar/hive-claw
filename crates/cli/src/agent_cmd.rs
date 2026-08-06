//! `nanobot agent` — interactive REPL or single-shot turn.
//!
//! Streamlined port of `nanobot/cli/commands.py::agent`. Drops the
//! Python prompt_toolkit / Rich rendering layer in favour of a plain
//! rustyline REPL. `--logs` enables `INFO` level forwarding from the
//! agent crates to stderr.

use std::io::{self, Read};
use std::path::PathBuf;
use std::sync::Arc;

use bus::InboundMessage;
use chrono::Local;
use rustyline::DefaultEditor;
use rustyline::error::ReadlineError;

use crate::LoopBundle;
use crate::runtime::Runtime;

/// Args for `nanobot agent`.
#[derive(Debug, Default, Clone)]
pub struct AgentArgs {
    /// Single-shot message. When `None`, drops into a REPL.
    pub message: Option<String>,
    /// Override the session ID. Defaults to `cli:default`.
    pub session: Option<String>,
    /// Override the workspace from the config.
    pub workspace: Option<PathBuf>,
    /// Override the config path.
    pub config: Option<PathBuf>,
    /// Reserved — no Markdown rendering in the Rust REPL yet.
    pub markdown: bool,
    /// Enable `INFO`-level log forwarding to stderr.
    pub logs: bool,
}

pub async fn run(args: AgentArgs) -> Result<(), String> {
    if args.logs {
        let _ = env_logger::Builder::from_default_env()
            .filter_level(log::LevelFilter::Info)
            .try_init();
    }

    let Runtime { config: cfg, .. } =
        Runtime::from_config(args.config.as_deref(), args.workspace.as_deref())?;
    let bundle = LoopBundle::build_agent_loop(&cfg, None).await?;
    let agent = Arc::new(bundle.agent);

    let session = args
        .session
        .clone()
        .unwrap_or_else(|| "cli:default".to_string());
    let (channel, chat_id) = split_session(&session);

    if let Some(message) = args.message {
        return one_shot(&agent, &channel, &chat_id, &session, &message).await;
    }

    repl(agent, channel, chat_id, session).await
}

async fn one_shot(
    agent: &Arc<agent::AgentLoop>,
    channel: &str,
    chat_id: &str,
    session_key: &str,
    message: &str,
) -> Result<(), String> {
    let body = if message == "-" {
        let mut buf = String::new();
        io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| e.to_string())?;
        buf
    } else {
        message.to_string()
    };

    let inbound = InboundMessage {
        channel: channel.into(),
        sender_id: "cli".into(),
        chat_id: chat_id.into(),
        content: body,
        timestamp: Local::now(),
        media: Vec::new(),
        metadata: Default::default(),
        session_key_override: Some(session_key.into()),
    };
    let result = agent.process_inbound(inbound).await?;
    if let Some(text) = result.final_content {
        println!("{text}");
    }
    Ok(())
}

async fn repl(
    agent: Arc<agent::AgentLoop>,
    channel: String,
    chat_id: String,
    session_key: String,
) -> Result<(), String> {
    let mut rl = DefaultEditor::new().map_err(|e| e.to_string())?;
    let history = config::paths::get_cli_history_path();
    if let Some(parent) = history.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = rl.load_history(&history);

    println!("nanobot agent — session: {session_key}");
    println!("Type a message and press Enter. /exit to quit.");

    loop {
        match rl.readline("» ") {
            Ok(line) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                if matches!(trimmed, "/exit" | "/quit" | ":q") {
                    break;
                }
                let _ = rl.add_history_entry(line.as_str());
                let inbound = InboundMessage {
                    channel: channel.clone(),
                    sender_id: "cli".into(),
                    chat_id: chat_id.clone(),
                    content: trimmed.to_string(),
                    timestamp: Local::now(),
                    media: Vec::new(),
                    metadata: Default::default(),
                    session_key_override: Some(session_key.clone()),
                };
                match agent.process_inbound(inbound).await {
                    Ok(result) => {
                        if let Some(text) = result.final_content {
                            println!("{text}");
                        }
                    }
                    Err(e) => eprintln!("error: {e}"),
                }
            }
            Err(ReadlineError::Interrupted) | Err(ReadlineError::Eof) => break,
            Err(e) => return Err(e.to_string()),
        }
    }
    let _ = rl.save_history(&history);
    Ok(())
}

fn split_session(key: &str) -> (String, String) {
    if let Some((a, b)) = key.split_once(':')
        && !a.is_empty()
        && !b.is_empty()
    {
        return (a.to_string(), b.to_string());
    }
    ("cli".into(), key.to_string())
}
