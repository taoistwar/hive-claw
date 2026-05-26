/// Discord channel implementation.
///
/// Uses the discord.py SDK in Python; this is a skeleton implementation with TODOs
/// for the full discord-rs integration.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use log::{error, info, warn};
use serde::{Deserialize, Serialize};
use serde_json::Map;

use bus::MessageBus;
use bus::OutboundMessage;
use serde_json::Value;

use crate::base::{handle_inbound, Channel, ChannelError, ChannelResult, TranscriptionSettings};
use crate::registry::ChannelEntry;

/// Discord channel configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DiscordConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub bot_token: String,
    #[serde(default)]
    pub allow_from: Vec<String>,
    #[serde(default)]
    pub react_emoji: String,
    #[serde(default)]
    pub done_emoji: String,
    #[serde(default)]
    pub group_policy: String,
    #[serde(default = "default_true")]
    pub reply_in_thread: bool,
    #[serde(default = "default_true")]
    pub streaming: bool,
}

fn default_true() -> bool { true }

impl Default for DiscordConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            bot_token: String::new(),
            allow_from: Vec::new(),
            react_emoji: "👀".to_string(),
            done_emoji: "✅".to_string(),
            group_policy: "mention".to_string(),
            reply_in_thread: default_true(),
            streaming: default_true(),
        }
    }
}

/// Discord channel using the Discord SDK.
pub struct DiscordChannel {
    config: DiscordConfig,
    bus: MessageBus,
    running: Arc<AtomicBool>,
    transcription: TranscriptionSettings,
}

impl DiscordChannel {
    pub fn from_value(
        value: Value,
        bus: MessageBus,
        transcription: TranscriptionSettings,
    ) -> Result<Self, String> {
        let config: DiscordConfig = serde_json::from_value(value)
            .map_err(|e| format!("invalid discord config: {}", e))?;
        Ok(Self {
            config,
            bus,
            running: Arc::new(AtomicBool::new(false)),
            transcription,
        })
    }
}

#[async_trait]
impl Channel for DiscordChannel {
    fn name(&self) -> &'static str { "discord" }
    fn display_name(&self) -> &'static str { "Discord" }
    fn bus(&self) -> &MessageBus { &self.bus }
    fn is_running(&self) -> bool { self.running.load(Ordering::SeqCst) }

    async fn start(self: Arc<Self>) -> ChannelResult<()> {
        if self.config.bot_token.is_empty() {
            error!("Discord bot_token not configured");
            return Err(ChannelError::Config("bot_token required".into()));
        }
        self.running.store(true, Ordering::SeqCst);
        info!("Discord channel started");

        while self.running.load(Ordering::SeqCst) {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
        Ok(())
    }

    async fn stop(self: Arc<Self>) -> ChannelResult<()> {
        self.running.store(false, Ordering::SeqCst);
        info!("Discord channel stopped");
        Ok(())
    }

    async fn send(self: Arc<Self>, _msg: OutboundMessage) -> ChannelResult<()> {
        warn!("Discord send not yet implemented");
        Err(ChannelError::Other("send not implemented".into()))
    }

    async fn send_delta(
        self: Arc<Self>,
        _chat_id: String,
        _delta: String,
        _metadata: Map<String, serde_json::Value>,
    ) -> ChannelResult<()> {
        warn!("Discord send_delta not yet implemented");
        Ok(())
    }

    async fn login(self: Arc<Self>, _force: bool) -> ChannelResult<bool> { Ok(true) }

    fn default_config() -> Map<String, serde_json::Value> {
        let config = DiscordConfig::default();
        let value = serde_json::to_value(&config).unwrap_or(serde_json::Value::Object(Map::new()));
        match value { serde_json::Value::Object(map) => map, _ => Map::new() }
    }
}

pub(crate) fn build(
    section: Value,
    bus: MessageBus,
    transcription: TranscriptionSettings,
) -> Result<ChannelEntry, String> {
    let ch = DiscordChannel::from_value(section, bus, transcription)?;
    let arc: Arc<dyn Channel> = Arc::new(ch);
    Ok(ChannelEntry {
        name: "discord".into(),
        display_name: "Discord".into(),
        channel: arc,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = DiscordChannel::default_config();
        assert_eq!(config.get("enabled").and_then(|v| v.as_bool()), Some(false));
    }

    #[test]
    fn test_name() {
        assert_eq!(DiscordChannel::name(), "discord");
    }

    #[test]
    fn test_display_name() {
        assert_eq!(DiscordChannel::display_name(), "Discord");
    }
}
