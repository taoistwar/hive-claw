//! Helpers for restart notification messages (port of
//! `nanobot.utils.restart`).

use std::env;
use std::time::{SystemTime, UNIX_EPOCH};

pub const RESTART_NOTIFY_CHANNEL_ENV: &str = "NANOBOT_RESTART_NOTIFY_CHANNEL";
pub const RESTART_NOTIFY_CHAT_ID_ENV: &str = "NANOBOT_RESTART_NOTIFY_CHAT_ID";
pub const RESTART_STARTED_AT_ENV: &str = "NANOBOT_RESTART_STARTED_AT";

/// Immutable restart notice passed between process generations via env vars.
#[derive(Debug, Clone)]
pub struct RestartNotice {
    pub channel: String,
    pub chat_id: String,
    pub started_at_raw: String,
}

/// Build restart-completion text; include elapsed time when
/// `started_at_raw` is a valid unix-epoch seconds string.
pub fn format_restart_completed_message(started_at_raw: &str) -> String {
    if started_at_raw.is_empty() {
        return "Restart completed.".into();
    }
    match started_at_raw.parse::<f64>() {
        Ok(started_at) => {
            let now_s = now_seconds();
            let elapsed = (now_s - started_at).max(0.0);
            format!("Restart completed in {elapsed:.1}s.")
        }
        Err(_) => "Restart completed.".into(),
    }
}

/// Write restart-notice env values for the next process.
///
/// # Safety
/// `std::env::set_var` is only safe when no other threads touch the
/// environment. Callers should invoke this during single-threaded startup /
/// shutdown.
pub fn set_restart_notice_to_env(channel: &str, chat_id: &str) {
    unsafe {
        env::set_var(RESTART_NOTIFY_CHANNEL_ENV, channel);
        env::set_var(RESTART_NOTIFY_CHAT_ID_ENV, chat_id);
        env::set_var(RESTART_STARTED_AT_ENV, now_seconds().to_string());
    }
}

/// Read and clear restart-notice env values once for this process.
///
/// # Safety
/// `std::env::remove_var` is only safe when no other threads touch the
/// environment.
pub fn consume_restart_notice_from_env() -> Option<RestartNotice> {
    let channel = env::var(RESTART_NOTIFY_CHANNEL_ENV).unwrap_or_default().trim().to_string();
    let chat_id = env::var(RESTART_NOTIFY_CHAT_ID_ENV).unwrap_or_default().trim().to_string();
    let started_at_raw = env::var(RESTART_STARTED_AT_ENV).unwrap_or_default().trim().to_string();
    unsafe {
        env::remove_var(RESTART_NOTIFY_CHANNEL_ENV);
        env::remove_var(RESTART_NOTIFY_CHAT_ID_ENV);
        env::remove_var(RESTART_STARTED_AT_ENV);
    }
    if channel.is_empty() || chat_id.is_empty() {
        return None;
    }
    Some(RestartNotice {
        channel,
        chat_id,
        started_at_raw,
    })
}

/// Return `true` when a restart notice should be shown in this CLI session.
pub fn should_show_cli_restart_notice(notice: &RestartNotice, session_id: &str) -> bool {
    if notice.channel != "cli" {
        return false;
    }
    let cli_chat_id = session_id
        .split_once(':')
        .map(|(_, rest)| rest)
        .unwrap_or(session_id);
    notice.chat_id.is_empty() || notice.chat_id == cli_chat_id
}

fn now_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}
