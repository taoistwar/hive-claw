/// Telegram channel implementation using long polling.
///
/// Uses the python-telegram-bot SDK in Python; this is a skeleton implementation
/// with TODOs for the full teloxide/grammers integration.
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use log::{error, info, warn};
use serde::{Deserialize, Serialize};
use serde_json::Map;

use bus::MessageBus;
use bus::OutboundMessage;
use serde_json::Value;

use crate::base::{Channel, ChannelError, ChannelResult, TranscriptionSettings};
use crate::registry::ChannelEntry;

/// Telegram channel configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TelegramConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub token: String,
    #[serde(default)]
    pub allow_from: Vec<String>,
    #[serde(default)]
    pub proxy: Option<String>,
    #[serde(default)]
    pub reply_to_message: bool,
    #[serde(default)]
    pub react_emoji: String,
    #[serde(default)]
    pub group_policy: String,
    #[serde(default = "default_pool_size")]
    pub connection_pool_size: usize,
    #[serde(default = "default_pool_timeout")]
    pub pool_timeout: f64,
    #[serde(default = "default_true")]
    pub streaming: bool,
    #[serde(default)]
    pub inline_keyboards: bool,
    #[serde(default = "default_edit_interval")]
    pub stream_edit_interval: f64,
    #[serde(default)]
    pub transcription: Option<serde_json::Value>,
}

fn default_pool_size() -> usize {
    32
}
fn default_pool_timeout() -> f64 {
    5.0
}
fn default_true() -> bool {
    true
}
fn default_edit_interval() -> f64 {
    0.6
}

impl Default for TelegramConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            token: String::new(),
            allow_from: Vec::new(),
            proxy: None,
            reply_to_message: false,
            react_emoji: "\u{1f440}".to_string(),
            group_policy: "mention".to_string(),
            connection_pool_size: default_pool_size(),
            pool_timeout: default_pool_timeout(),
            streaming: default_true(),
            inline_keyboards: false,
            stream_edit_interval: default_edit_interval(),
            transcription: None,
        }
    }
}

/// Telegram channel using long polling.
pub struct TelegramChannel {
    config: TelegramConfig,
    bus: MessageBus,
    running: Arc<AtomicBool>,
    #[expect(
        dead_code,
        reason = "reserved for staged inbound media transcription integration"
    )]
    transcription: TranscriptionSettings,
}

impl TelegramChannel {
    pub fn from_value(
        value: Value,
        bus: MessageBus,
        transcription: TranscriptionSettings,
    ) -> Result<Self, String> {
        let config: TelegramConfig =
            serde_json::from_value(value).map_err(|e| format!("invalid telegram config: {}", e))?;
        Ok(Self {
            config,
            bus,
            running: Arc::new(AtomicBool::new(false)),
            transcription,
        })
    }
}

pub(crate) fn build(
    section: Value,
    bus: MessageBus,
    transcription: TranscriptionSettings,
) -> Result<ChannelEntry, String> {
    let ch = TelegramChannel::from_value(section, bus, transcription)?;
    let arc: Arc<dyn Channel> = Arc::new(ch);
    Ok(ChannelEntry {
        name: "telegram".into(),
        display_name: "Telegram".into(),
        channel: arc,
    })
}

#[async_trait]
impl Channel for TelegramChannel {
    fn name() -> &'static str {
        "telegram"
    }

    fn display_name() -> &'static str {
        "Telegram"
    }

    fn bus(&self) -> &MessageBus {
        &self.bus
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    async fn start(self: Arc<Self>) -> ChannelResult<()> {
        if self.config.token.is_empty() {
            error!("Telegram bot token not configured");
            return Err(ChannelError::Config("token required".into()));
        }

        self.running.store(true, Ordering::SeqCst);
        info!("Telegram channel started (polling mode)");

        // TODO: Implement Telegram bot using a Rust Telegram Bot library (e.g., teloxide, frankenstein)
        // The Python version uses python-telegram-bot with long polling
        // Features needed:
        // - Bot initialization and polling
        // - Command handlers (/start, /help, /new, /stop, etc.)
        // - Message handler (text, photos, video, voice, documents, location)
        // - Media group buffering
        // - Typing indicator loop
        // - Emoji reactions
        // - Progressive message editing (streaming)
        // - Markdown to HTML conversion (tables, code blocks, etc.)
        // - Inline keyboard buttons
        // - Callback query handling
        // - Media download (various types with transcription for audio)
        // - Reply context extraction
        // - Topic/thread session keys
        // - Group policy (open/mention)
        // - Allowlist matching (id|username format)

        while self.running.load(Ordering::SeqCst) {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }

        Ok(())
    }

    async fn stop(self: Arc<Self>) -> ChannelResult<()> {
        self.running.store(false, Ordering::SeqCst);
        info!("Telegram channel stopped");
        Ok(())
    }

    async fn send(self: Arc<Self>, _msg: OutboundMessage) -> ChannelResult<()> {
        // TODO: Implement Telegram message sending
        warn!("Telegram send not yet implemented");
        Err(ChannelError::Other("send not implemented".into()))
    }

    async fn send_delta(
        self: Arc<Self>,
        _chat_id: String,
        _delta: String,
        _metadata: Map<String, serde_json::Value>,
    ) -> ChannelResult<()> {
        // TODO: Implement Telegram progressive message editing
        warn!("Telegram send_delta not yet implemented");
        Ok(())
    }

    async fn login(self: Arc<Self>, _force: bool) -> ChannelResult<bool> {
        Ok(true)
    }

    fn default_config() -> serde_json::Map<String, serde_json::Value>
    where
        Self: Sized,
    {
        let config = TelegramConfig::default();
        let value = serde_json::to_value(&config).unwrap_or(serde_json::Value::Object(Map::new()));
        match value {
            serde_json::Value::Object(map) => map,
            _ => Map::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = TelegramChannel::default_config();
        assert_eq!(config.get("enabled").and_then(|v| v.as_bool()), Some(false));
    }

    #[test]
    fn test_name() {
        assert_eq!(TelegramChannel::name(), "telegram");
    }

    #[test]
    fn test_display_name() {
        assert_eq!(TelegramChannel::display_name(), "Telegram");
    }
}
