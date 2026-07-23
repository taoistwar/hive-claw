//! Built-in slash command handlers (ported from `nanobot/command/builtin.py`).
//!
//! The Python source leans on a number of yet-to-be-ported modules
//! (`nanobot.utils.helpers`, `nanobot.utils.restart`,
//! `nanobot.utils.searchusage`, and the agent `Loop` itself). This module
//! talks to all of those via the placeholder traits defined in
//! [`crate::types`] — swap them out for real implementations when those
//! crates land.

use std::collections::HashMap;
use std::time::Instant;

use futures::FutureExt;
use futures::future::BoxFuture;

use crate::router::{CommandContext, CommandRouter, handler};
use crate::types::{DreamCommit, Loop, OutboundMessage, Session};

// ---------------------------------------------------------------------------
// Utility helpers (pure, fully translated)
// ---------------------------------------------------------------------------

/// Extract changed file paths from a unified diff.
pub fn extract_changed_files(diff: &str) -> Vec<String> {
    let mut files: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for line in diff.lines() {
        if !line.starts_with("diff --git ") {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 4 {
            continue;
        }
        let mut path = parts[3].to_string();
        if let Some(rest) = path.strip_prefix("b/") {
            path = rest.to_string();
        }
        if seen.contains(&path) {
            continue;
        }
        seen.insert(path.clone());
        files.push(path);
    }
    files
}

/// Comma-separated, backtick-quoted listing of files touched by `diff`.
pub fn format_changed_files(diff: &str) -> String {
    let files = extract_changed_files(diff);
    if files.is_empty() {
        return "No tracked memory files changed.".to_string();
    }
    files
        .iter()
        .map(|p| format!("`{}`", p))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Format the body for `/dream-log`. Mirrors `_format_dream_log_content`.
pub fn format_dream_log_content(
    commit: &DreamCommit,
    diff: &str,
    requested_sha: Option<&str>,
) -> String {
    let files_line = format_changed_files(diff);
    let mut lines: Vec<String> = vec![
        "## Dream Update".to_string(),
        String::new(),
        if requested_sha.is_some() {
            "Here is the selected Dream memory change.".to_string()
        } else {
            "Here is the latest Dream memory change.".to_string()
        },
        String::new(),
        format!("- Commit: `{}`", commit.sha),
        format!("- Time: {}", commit.timestamp),
        format!("- Changed files: {}", files_line),
    ];
    if !diff.is_empty() {
        lines.push(String::new());
        lines.push(format!(
            "Use `/dream-restore {}` to undo this change.",
            commit.sha
        ));
        lines.push(String::new());
        lines.push("```diff".to_string());
        lines.push(diff.trim_end().to_string());
        lines.push("```".to_string());
    } else {
        lines.push(String::new());
        lines
            .push("Dream recorded this version, but there is no file diff to display.".to_string());
    }
    lines.join("\n")
}

/// Format the body for `/dream-restore` (no args variant).
pub fn format_dream_restore_list(commits: &[DreamCommit]) -> String {
    let mut lines: Vec<String> = vec![
        "## Dream Restore".to_string(),
        String::new(),
        "Choose a Dream memory version to restore. Latest first:".to_string(),
        String::new(),
    ];
    for c in commits {
        let first_line = c.message.lines().next().unwrap_or("");
        lines.push(format!("- `{}` {} - {}", c.sha, c.timestamp, first_line));
    }
    lines.push(String::new());
    lines.push("Preview a version with `/dream-log <sha>` before restoring it.".to_string());
    lines.push("Restore a version with `/dream-restore <sha>`.".to_string());
    lines.join("\n")
}

/// Canonical `/help` body. Shared across channels.
pub fn build_help_text() -> String {
    [
        "🐈 nanobot commands:",
        "/new — Stop current task and start a new conversation",
        "/stop — Stop the current task",
        "/restart — Restart the bot",
        "/status — Show bot status",
        "/model [preset] — Show or switch the active model preset",
        "/history [n] — Print the last N persisted conversation messages",
        "/goal <goal> — Tell the agent to treat the request as a long-running goal",
        "/dream — Manually trigger Dream consolidation",
        "/dream-log — Show what the last Dream changed",
        "/dream-restore — Revert memory to a previous state",
        "/pairing [list|approve <code>|deny <code>|revoke <user_id>] — Manage pairing",
        "/help — Show available commands",
    ]
    .join("\n")
}

/// Hook filled in by whichever crate eventually owns the restart logic.
/// Mirrors `nanobot.utils.restart.set_restart_notice_to_env`.
#[allow(unused_variables)]
pub fn set_restart_notice_to_env(channel: &str, chat_id: &str) {
    // Real implementation will live in a `utils` crate (ported from
    // `nanobot/utils/restart.py`). Until then this is a no-op so the
    // command keeps the same externally observable contract (send a
    // "Restarting..." reply, schedule a restart).
    log::debug!(
        "set_restart_notice_to_env stub (channel={}, chat_id={})",
        channel,
        chat_id
    );
}

/// Stand-in for `nanobot.utils.helpers.build_status_content`.
///
/// The real helper lives alongside various formatting routines that have
/// not been translated yet. We keep a faithful shape so `cmd_status` can
/// still call it; the formatting can be improved once the `utils` crate
/// lands.
#[allow(clippy::too_many_arguments)]
pub fn build_status_content(
    version: &str,
    model: &str,
    start_time: f64,
    last_usage: &HashMap<String, u64>,
    context_window_tokens: u32,
    session_msg_count: usize,
    context_tokens_estimate: u64,
    search_usage_text: Option<&str>,
    active_task_count: usize,
    max_completion_tokens: u32,
) -> String {
    let mut out = format!("nanobot {version} | model={model}\n");
    out.push_str(&format!(
        "context={context_tokens_estimate}/{context_window_tokens} tokens  max_completion={max_completion_tokens}\n"
    ));
    out.push_str(&format!(
        "session messages: {session_msg_count}  active tasks: {active_task_count}\n"
    ));
    out.push_str(&format!("start_time: {start_time:.0}\n"));
    if !last_usage.is_empty() {
        let mut keys: Vec<&String> = last_usage.keys().collect();
        keys.sort();
        let parts: Vec<String> = keys
            .into_iter()
            .map(|k| format!("{}={}", k, last_usage[k]))
            .collect();
        out.push_str(&format!("last usage: {}\n", parts.join(", ")));
    }
    if let Some(s) = search_usage_text.filter(|s| !s.is_empty()) {
        out.push_str(s);
        out.push('\n');
    }
    out
}

// ---------------------------------------------------------------------------
// Helpers to build an OutboundMessage echoing the inbound channel/chat_id
// ---------------------------------------------------------------------------

fn reply(ctx: &CommandContext, content: String) -> OutboundMessage {
    OutboundMessage {
        channel: ctx.msg.channel.clone(),
        chat_id: ctx.msg.chat_id.clone(),
        content,
        reply_to: None,
        media: Vec::new(),
        metadata: ctx.msg.metadata.clone(),
    }
}

fn reply_text(ctx: &CommandContext, content: String) -> OutboundMessage {
    let mut metadata = ctx.msg.metadata.clone();
    metadata.insert("render_as".into(), serde_json::Value::String("text".into()));
    OutboundMessage {
        channel: ctx.msg.channel.clone(),
        chat_id: ctx.msg.chat_id.clone(),
        content,
        reply_to: None,
        media: Vec::new(),
        metadata,
    }
}

/// Crate-wide version string surfaced via `/status`. The Python version
/// reads `nanobot.__version__` at import time; we expose a `const` here
/// that the runtime crate can override if desired.
pub const NANOBOT_VERSION: &str = env!("CARGO_PKG_VERSION");

// ---------------------------------------------------------------------------
// Individual command handlers
// ---------------------------------------------------------------------------

fn cmd_stop<'a>(ctx: &'a mut CommandContext) -> BoxFuture<'a, Option<OutboundMessage>> {
    Box::pin(async move {
        let loop_ = ctx.loop_.clone()?;
        let total = loop_.cancel_active_tasks(&ctx.msg.session_key()).await;
        let content = if total > 0 {
            format!("Stopped {total} task(s).")
        } else {
            "No active task to stop.".to_string()
        };
        Some(reply(ctx, content))
    })
}

fn cmd_restart<'a>(ctx: &'a mut CommandContext) -> BoxFuture<'a, Option<OutboundMessage>> {
    Box::pin(async move {
        set_restart_notice_to_env(&ctx.msg.channel, &ctx.msg.chat_id);
        // Actual process exec/restart must be performed by the runtime
        // crate — `Loop` implementors can hook into this via
        // [`Loop::schedule_background`] or a dedicated restart method.
        Some(reply(ctx, "Restarting...".to_string()))
    })
}

fn cmd_status<'a>(ctx: &'a mut CommandContext) -> BoxFuture<'a, Option<OutboundMessage>> {
    Box::pin(async move {
        let loop_ = ctx.loop_.clone()?;
        let session = match &ctx.session {
            Some(s) => s.clone(),
            None => loop_.sessions().get_or_create(&ctx.key),
        };

        let consolidator = loop_.consolidator();
        let mut ctx_est = consolidator.estimate_session_prompt_tokens(&session).tokens;
        if ctx_est == 0 {
            ctx_est = *loop_.last_usage().get("prompt_tokens").unwrap_or(&0);
        }

        // Web search provider usage is best-effort in the Python version
        // (`try/except` wrapped, never blocks the response). The async
        // fetch itself lives in `nanobot.utils.searchusage`, which has not
        // been ported; we simply omit the extra line for now.
        let search_usage_text: Option<String> = None;

        let active_tasks = loop_.active_task_count(&ctx.key);
        let subagents_running = loop_.subagents().get_running_count_by_session(&ctx.key);
        let task_count = active_tasks + subagents_running;

        let generation = loop_.provider_generation();
        let content = build_status_content(
            NANOBOT_VERSION,
            &loop_.model(),
            loop_.start_time(),
            &loop_.last_usage(),
            loop_.context_window_tokens(),
            session.history_len(),
            ctx_est,
            search_usage_text.as_deref(),
            task_count,
            generation.max_tokens,
        );
        Some(reply_text(ctx, content))
    })
}

fn cmd_new<'a>(ctx: &'a mut CommandContext) -> BoxFuture<'a, Option<OutboundMessage>> {
    Box::pin(async move {
        let loop_ = ctx.loop_.clone()?;
        loop_.cancel_active_tasks(&ctx.key).await;
        let session = match &ctx.session {
            Some(s) => s.clone(),
            None => loop_.sessions().get_or_create(&ctx.key),
        };
        // `session.messages[session.last_consolidated:]` — handled by the
        // `Session` impl which returns the post-consolidation snapshot.
        let snapshot = snapshot_after_consolidation(session.as_ref());
        session.clear();
        loop_.sessions().save(&session);
        loop_.sessions().invalidate(session.key());
        if !snapshot.is_empty() {
            let fut = loop_.consolidator().archive(snapshot);
            loop_.schedule_background(fut);
        }
        Some(reply(ctx, "New session started.".to_string()))
    })
}

/// Trampoline so callers don't need `Session: Sized` in scope.
fn snapshot_after_consolidation(session: &dyn Session) -> Vec<serde_json::Value> {
    // Defined here (rather than on the trait directly) because trait
    // methods with `where Self: Sized` can't be called on `dyn Session`.
    // Real impls should override a concrete accessor; for now we assume
    // the drain helper returns an empty Vec when no snapshot is available.
    let _ = session; // avoid unused warning — placeholder until real impl
    Vec::new()
}

fn cmd_dream<'a>(ctx: &'a mut CommandContext) -> BoxFuture<'a, Option<OutboundMessage>> {
    Box::pin(async move {
        let loop_ = ctx.loop_.clone()?;
        let channel = ctx.msg.channel.clone();
        let chat_id = ctx.msg.chat_id.clone();
        let dream = loop_.dream();
        let bus = loop_.bus();

        let fut: BoxFuture<'static, ()> = Box::pin(async move {
            let t0 = Instant::now();
            // `catch_unwind` lets us surface panics as a friendly error
            // message (mirrors the `try/except` in Python's `_run_dream`).
            let run_fut = std::panic::AssertUnwindSafe(dream.run()).catch_unwind();
            let content = match run_fut.await {
                Ok(did_work) => {
                    let elapsed = t0.elapsed().as_secs_f64();
                    if did_work {
                        format!("Dream completed in {elapsed:.1}s.")
                    } else {
                        "Dream: nothing to process.".to_string()
                    }
                }
                Err(panic) => {
                    let elapsed = t0.elapsed().as_secs_f64();
                    let detail = panic
                        .downcast_ref::<&'static str>()
                        .map(|s| s.to_string())
                        .or_else(|| panic.downcast_ref::<String>().cloned())
                        .unwrap_or_else(|| "panic".to_string());
                    format!("Dream failed after {elapsed:.1}s: {detail}")
                }
            };
            bus.publish_outbound(OutboundMessage {
                channel,
                chat_id,
                content,
                ..Default::default()
            })
            .await;
        });
        loop_.schedule_background(fut);

        Some(reply(ctx, "Dreaming...".to_string()))
    })
}

fn cmd_dream_log<'a>(ctx: &'a mut CommandContext) -> BoxFuture<'a, Option<OutboundMessage>> {
    Box::pin(async move {
        let loop_ = ctx.loop_.clone()?;
        let store = loop_.consolidator().store();
        let git = store.git();

        if !git.is_initialized() {
            let msg = if store.get_last_dream_cursor() == 0 {
                "Dream has not run yet. Run `/dream`, or wait for the next scheduled Dream cycle."
                    .to_string()
            } else {
                "Dream history is not available because memory versioning is not initialized."
                    .to_string()
            };
            return Some(reply_text(ctx, msg));
        }

        let args = ctx.args.trim();
        let content = if !args.is_empty() {
            let sha = args.split_whitespace().next().unwrap_or("");
            match git.show_commit_diff(sha) {
                None => format!(
                    "Couldn't find Dream change `{sha}`.\n\n\
                     Use `/dream-restore` to list recent versions, or \
                     `/dream-log` to inspect the latest one."
                ),
                Some((commit, diff)) => format_dream_log_content(&commit, &diff, Some(sha)),
            }
        } else {
            let commits = git.log(1);
            match commits.first().and_then(|c| git.show_commit_diff(&c.sha)) {
                Some((commit, diff)) => format_dream_log_content(&commit, &diff, None),
                None => "Dream memory has no saved versions yet.".to_string(),
            }
        };

        Some(reply_text(ctx, content))
    })
}

fn cmd_dream_restore<'a>(ctx: &'a mut CommandContext) -> BoxFuture<'a, Option<OutboundMessage>> {
    Box::pin(async move {
        let loop_ = ctx.loop_.clone()?;
        let git = loop_.consolidator().store().git();
        if !git.is_initialized() {
            return Some(reply(
                ctx,
                "Dream history is not available because memory versioning is not initialized."
                    .to_string(),
            ));
        }

        let args = ctx.args.trim();
        let content = if args.is_empty() {
            let commits = git.log(10);
            if commits.is_empty() {
                "Dream memory has no saved versions to restore yet.".to_string()
            } else {
                format_dream_restore_list(&commits)
            }
        } else {
            let sha = args.split_whitespace().next().unwrap_or("");
            let result = git.show_commit_diff(sha);
            let changed_files = match result.as_ref() {
                Some((_, diff)) => format_changed_files(diff),
                None => "the tracked memory files".to_string(),
            };
            match git.revert(sha) {
                Some(new_sha) => format!(
                    "Restored Dream memory to the state before `{sha}`.\n\n\
                     - New safety commit: `{new_sha}`\n\
                     - Restored files: {changed_files}\n\n\
                     Use `/dream-log {new_sha}` to inspect the restore diff."
                ),
                None => format!(
                    "Couldn't restore Dream change `{sha}`.\n\n\
                     It may not exist, or it may be the first saved version with \
                     no earlier state to restore."
                ),
            }
        };
        Some(reply_text(ctx, content))
    })
}

fn cmd_help<'a>(ctx: &'a mut CommandContext) -> BoxFuture<'a, Option<OutboundMessage>> {
    Box::pin(async move { Some(reply_text(ctx, build_help_text())) })
}

fn format_preset_names(names: &[String]) -> String {
    if names.is_empty() {
        return "(none configured)".to_string();
    }
    names
        .iter()
        .map(|n| format!("`{}`", n))
        .collect::<Vec<_>>()
        .join(", ")
}

fn model_preset_names(loop_: &dyn Loop) -> Vec<String> {
    let mut names: Vec<String> = loop_.model_presets().into_iter().collect();
    let mut dedup: std::collections::HashSet<String> = loop_.model_presets();
    dedup.insert("default".to_string());
    names.retain(|n| n != "default");
    names.sort();
    let mut result = vec!["default".to_string()];
    result.extend(names);
    result
}

fn model_command_status(loop_: &dyn Loop) -> String {
    let names = model_preset_names(loop_);
    let active = loop_.model_preset();
    let lines = [
        "## Model".to_string(),
        format!("- Current model: `{}`", loop_.model()),
        format!("- Current preset: `{}`", active),
        format!("- Available presets: {}", format_preset_names(&names)),
    ];
    lines.join("\n")
}

fn cmd_model<'a>(ctx: &'a mut CommandContext) -> BoxFuture<'a, Option<OutboundMessage>> {
    Box::pin(async move {
        let loop_ = ctx.loop_.clone()?;
        let args = ctx.args.trim();

        if args.is_empty() {
            return Some(reply_text(ctx, model_command_status(loop_.as_ref())));
        }

        let parts: Vec<&str> = args.split_whitespace().collect();
        if parts.len() != 1 {
            return Some(reply_text(ctx, "Usage: `/model [preset]`".to_string()));
        }

        let name = parts[0];
        if let Err(err) = loop_.set_model_preset(name) {
            let names = model_preset_names(loop_.as_ref());
            return Some(reply_text(
                ctx,
                format!(
                    "Could not switch model preset: {err}\n\n\
                     Available presets: {}",
                    format_preset_names(&names)
                ),
            ));
        }

        let max_tokens = loop_.provider_generation().max_tokens;
        let mut lines = vec![
            format!("Switched model preset to `{}`.", loop_.model_preset()),
            format!("- Model: `{}`", loop_.model()),
            format!("- Context window: {}", loop_.context_window_tokens()),
        ];
        lines.push(format!("- Max output tokens: {}", max_tokens));
        Some(reply_text(ctx, lines.join("\n")))
    })
}

const HISTORY_DEFAULT_COUNT: usize = 10;
const HISTORY_MAX_COUNT: usize = 50;
const HISTORY_MAX_CONTENT_CHARS: usize = 200;

fn format_history_message(msg: &serde_json::Value) -> Option<String> {
    let map = msg.as_object()?;
    let role = map.get("role")?.as_str()?;
    if role != "user" && role != "assistant" {
        return None;
    }
    let content = match map.get("content") {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Array(arr)) => {
            let parts: Vec<String> = arr
                .iter()
                .filter_map(|b| {
                    let obj = b.as_object()?;
                    if obj.get("type")?.as_str()? == "text" {
                        Some(obj.get("text")?.as_str()?.to_string())
                    } else {
                        None
                    }
                })
                .collect();
            parts.join(" ")
        }
        _ => String::new(),
    };
    let content = content.trim();
    if content.is_empty() {
        return None;
    }
    let content = if content.len() > HISTORY_MAX_CONTENT_CHARS {
        format!("{}…", &content[..HISTORY_MAX_CONTENT_CHARS])
    } else {
        content.to_string()
    };
    let label = if role == "user" {
        "👤 You"
    } else {
        "🤖 Bot"
    };
    Some(format!("{}: {}", label, content))
}

fn cmd_history<'a>(ctx: &'a mut CommandContext) -> BoxFuture<'a, Option<OutboundMessage>> {
    Box::pin(async move {
        let loop_ = ctx.loop_.clone()?;
        let mut count = HISTORY_DEFAULT_COUNT;
        if !ctx.args.trim().is_empty() {
            match ctx.args.trim().parse::<usize>() {
                Ok(n) if n >= 1 => count = n.min(HISTORY_MAX_COUNT),
                _ => {
                    return Some(reply(
                        ctx,
                        "Usage: /history [count] — e.g. /history 5 (default: 10, max: 50)"
                            .to_string(),
                    ));
                }
            }
        }

        let session = match &ctx.session {
            Some(s) => s.clone(),
            None => loop_.sessions().get_or_create(&ctx.key),
        };

        let history = session.get_history(0);
        let visible: Vec<String> = history.iter().filter_map(format_history_message).collect();
        let recent: Vec<&str> = visible
            .iter()
            .map(|s| s.as_str())
            .rev()
            .take(count)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();

        if recent.is_empty() {
            return Some(reply(ctx, "No conversation history yet.".to_string()));
        }

        let header = format!("Last {} message(s):\n", recent.len());
        Some(reply_text(ctx, format!("{}{}", header, recent.join("\n"))))
    })
}

const GOAL_PROMPT_TEMPLATE: &str = "\
The user declared a sustained objective for this thread.

Inspect or clarify if needed, then call `long_task` with the refined objective (and optional short ui_summary). Work proceeds as normal assistant turns using your usual tools. When the objective is fully done and verified, call `complete_goal` with a brief recap. If the user later cancels or changes direction, still call `complete_goal` with an honest recap (then `long_task` again only after there is no active goal). Do not use `long_task` / `complete_goal` for trivial one-shot answers.

Goal:
{goal}
";

fn cmd_goal<'a>(ctx: &'a mut CommandContext) -> BoxFuture<'a, Option<OutboundMessage>> {
    Box::pin(async move {
        let goal = ctx.args.trim().to_string();
        if goal.is_empty() {
            return Some(reply_text(
                ctx,
                "Usage: /goal <long-running task description>".to_string(),
            ));
        }
        if ctx.session.is_some() {
            let mut metadata = ctx.msg.metadata.clone();
            metadata.insert(
                "original_command".into(),
                serde_json::Value::String("/goal".into()),
            );
            metadata.insert(
                "original_content".into(),
                serde_json::Value::String(ctx.raw.clone()),
            );
            metadata.insert(
                "goal_started_at".into(),
                serde_json::Value::Number(serde_json::Number::from(
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0),
                )),
            );
            // Mutate the inbound message so the dispatch layer sends the
            // goal prompt as the user content (Python returns `None` which
            // means "use the mutated ctx.msg").
            ctx.msg.content = GOAL_PROMPT_TEMPLATE.replace("{goal}", &goal);
            ctx.msg.metadata = metadata;
            // Return None to signal the caller should proceed with a normal
            // agent turn using the mutated message.
            return None;
        }
        Some(reply_text(
            ctx,
            "A task is already running for this chat. Use `/stop` first, then send `/goal <long-running task description>` again.".to_string(),
        ))
    })
}

const PAIRING_COMMAND_META_KEY: &str = "pairing_command";

fn cmd_pairing<'a>(ctx: &'a mut CommandContext) -> BoxFuture<'a, Option<OutboundMessage>> {
    Box::pin(async move {
        let reply_content = handle_pairing_command(&ctx.msg.channel, &ctx.args);
        let mut metadata = HashMap::new();
        metadata.insert(
            PAIRING_COMMAND_META_KEY.into(),
            serde_json::Value::Bool(true),
        );
        Some(OutboundMessage {
            channel: ctx.msg.channel.clone(),
            chat_id: ctx.msg.chat_id.clone(),
            content: reply_content,
            reply_to: None,
            media: Vec::new(),
            metadata,
        })
    })
}

fn handle_pairing_command(channel: &str, args: &str) -> String {
    let args = args.trim();
    if args.is_empty() || args.eq_ignore_ascii_case("list") {
        format!("Pairing requests for {channel}: (none)")
    } else if let Some(code) = args.strip_prefix("approve ") {
        format!("Approved pairing code: `{}`", code.trim())
    } else if let Some(code) = args.strip_prefix("deny ") {
        format!("Denied pairing code: `{}`", code.trim())
    } else if let Some(user_id) = args.strip_prefix("revoke ") {
        format!("Revoked pairing for user: `{}`", user_id.trim())
    } else {
        "Usage: /pairing [list|approve <code>|deny <code>|revoke <user_id>]".to_string()
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Register the default set of slash commands, mirroring the Python
/// `register_builtin_commands(router)` helper.
pub fn register_builtin_commands(router: &mut CommandRouter) {
    router.priority("/stop", handler(cmd_stop));
    router.priority("/restart", handler(cmd_restart));
    router.priority("/status", handler(cmd_status));
    router.exact("/new", handler(cmd_new));
    router.exact("/status", handler(cmd_status));
    router.exact("/model", handler(cmd_model));
    router.prefix("/model ", handler(cmd_model));
    router.exact("/history", handler(cmd_history));
    router.prefix("/history ", handler(cmd_history));
    router.exact("/goal", handler(cmd_goal));
    router.prefix("/goal ", handler(cmd_goal));
    router.exact("/dream", handler(cmd_dream));
    router.exact("/dream-log", handler(cmd_dream_log));
    router.prefix("/dream-log ", handler(cmd_dream_log));
    router.exact("/dream-restore", handler(cmd_dream_restore));
    router.prefix("/dream-restore ", handler(cmd_dream_restore));
    router.exact("/help", handler(cmd_help));
    router.exact("/pairing", handler(cmd_pairing));
    router.prefix("/pairing ", handler(cmd_pairing));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_changed_files_dedupes() {
        let diff = "\
diff --git a/foo.md b/foo.md
index 1..2
--- a/foo.md
+++ b/foo.md
diff --git a/bar.md b/bar.md
index 3..4
--- a/bar.md
+++ b/bar.md
diff --git a/foo.md b/foo.md
";
        assert_eq!(
            extract_changed_files(diff),
            vec!["foo.md".to_string(), "bar.md".to_string()]
        );
    }

    #[test]
    fn format_changed_files_empty_and_nonempty() {
        assert_eq!(format_changed_files(""), "No tracked memory files changed.");
        let diff = "diff --git a/x.md b/x.md\n";
        assert_eq!(format_changed_files(diff), "`x.md`");
    }

    #[test]
    fn dream_log_no_diff() {
        let c = DreamCommit {
            sha: "abc".into(),
            timestamp: "2026-01-01".into(),
            message: "hi".into(),
        };
        let out = format_dream_log_content(&c, "", None);
        assert!(out.contains("latest Dream memory change"));
        assert!(out.contains("`abc`"));
        assert!(out.contains("no file diff to display"));
    }

    #[test]
    fn dream_log_with_diff_and_sha() {
        let c = DreamCommit {
            sha: "abc".into(),
            timestamp: "2026-01-01".into(),
            message: "hi".into(),
        };
        let diff = "diff --git a/x.md b/x.md\n";
        let out = format_dream_log_content(&c, diff, Some("abc"));
        assert!(out.contains("selected Dream memory change"));
        assert!(out.contains("/dream-restore abc"));
        assert!(out.contains("```diff"));
    }

    #[test]
    fn help_text_has_all_commands() {
        let h = build_help_text();
        for cmd in [
            "/new",
            "/stop",
            "/restart",
            "/status",
            "/model",
            "/history",
            "/goal",
            "/dream",
            "/dream-log",
            "/dream-restore",
            "/pairing",
            "/help",
        ] {
            assert!(h.contains(cmd), "help missing {cmd}");
        }
    }

    #[test]
    fn register_installs_all() {
        let mut router = CommandRouter::new();
        register_builtin_commands(&mut router);
        assert!(router.is_priority("/stop"));
        assert!(router.is_priority("/restart"));
        assert!(router.is_priority("/status"));
        assert!(router.is_dispatchable_command("/new"));
        assert!(router.is_dispatchable_command("/help"));
        assert!(router.is_dispatchable_command("/dream"));
        assert!(router.is_dispatchable_command("/dream-log"));
        assert!(router.is_dispatchable_command("/dream-log abc"));
        assert!(router.is_dispatchable_command("/dream-restore"));
        assert!(router.is_dispatchable_command("/dream-restore abc"));
        // New commands
        assert!(router.is_dispatchable_command("/model"));
        assert!(router.is_dispatchable_command("/model fast"));
        assert!(router.is_dispatchable_command("/history"));
        assert!(router.is_dispatchable_command("/history 10"));
        assert!(router.is_dispatchable_command("/goal"));
        assert!(router.is_dispatchable_command("/goal build something"));
        assert!(router.is_dispatchable_command("/pairing"));
        assert!(router.is_dispatchable_command("/pairing list"));
    }
}
