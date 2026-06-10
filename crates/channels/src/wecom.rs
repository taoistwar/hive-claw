/// WeCom (企业微信/Enterprise WeChat) channel implementation using WebSocket long connection.
///
/// Uses the wecom_aibot_sdk in Python; this is a skeleton implementation with TODOs.

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

/// WeCom channel configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WecomConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub bot_id: String,
    #[serde(default)]
    pub secret: String,
    #[serde(default)]
    pub allow_from: Vec<String>,
    #[serde(default)]
    pub welcome_message: String,
    #[serde(default)]
    pub transcription: Option<serde_json::Value>,
}

impl Default for WecomConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            bot_id: String::new(),
            secret: String::new(),
            allow_from: Vec::new(),
            welcome_message: String::new(),
            transcription: None,
        }
    }
}

/// WeCom channel using WebSocket long connection.
pub struct WecomChannel {
    config: WecomConfig,
    bus: MessageBus,
    running: Arc<AtomicBool>,
    transcription: TranscriptionSettings,
}

impl WecomChannel {
    pub fn from_value(
        value: Value,
        bus: MessageBus,
        transcription: TranscriptionSettings,
    ) -> Result<Self, String> {
        let config: WecomConfig = serde_json::from_value(value)
            .map_err(|e| format!("invalid wecom config: {}", e))?;
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
    let ch = WecomChannel::from_value(section, bus, transcription)?;
    let arc: Arc<dyn Channel> = Arc::new(ch);
    Ok(ChannelEntry {
        name: "wecom".into(),
        display_name: "WeCom".into(),
        channel: arc,
    })
}

#[async_trait]
impl Channel for WecomChannel {
    fn name() -> &'static str {
        "wecom"
    }

    fn display_name() -> &'static str {
        "WeCom"
    }

    fn bus(&self) -> &MessageBus {
        &self.bus
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    async fn start(self: Arc<Self>) -> ChannelResult<()> {
        if self.config.bot_id.is_empty() || self.config.secret.is_empty() {
            error!("WeCom bot_id/secret not configured");
            return Err(ChannelError::Config("bot_id and secret required".into()));
        }

        self.running.store(true, Ordering::SeqCst);
        info!("WeCom channel started (WebSocket long connection)");

        // TODO: Implement WeCom channel using a Rust WeCom SDK or raw WebSocket
        // The Python version uses wecom_aibot_sdk with WSClient
        // Features needed:
        // - WebSocket long connection (no public IP required)
        // - Reconnect with heartbeat
        // - Event handlers (connected, authenticated, disconnected, error)
        // - Message type handlers (text, image, voice, file, mixed)
        // - enter_chat event for welcome messages
        // - Media download (AES-encrypted)
        // - Media upload (3-step: init/chunk/finish with base64)
        // - Message deduplication (LRU cache)
        // - Frame storage for replies
        // - Stream reply (aibot_respond_msg)
        // - Proactive send (markdown only)

        while self.running.load(Ordering::SeqCst) {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }

        Ok(())
    }

    async fn stop(self: Arc<Self>) -> ChannelResult<()> {
        self.running.store(false, Ordering::SeqCst);
        info!("WeCom channel stopped");
        Ok(())
    }

    async fn send(self: Arc<Self>, _msg: OutboundMessage) -> ChannelResult<()> {
        // TODO: Implement WeCom message sending
        warn!("WeCom send not yet implemented");
        Err(ChannelError::Other("send not implemented".into()))
    }

    async fn send_delta(
        self: Arc<Self>,
        _chat_id: String,
        _delta: String,
        _metadata: Map<String, serde_json::Value>,
    ) -> ChannelResult<()> {
        // TODO: Implement WeCom stream reply
        warn!("WeCom send_delta not yet implemented");
        Ok(())
    }

    async fn login(self: Arc<Self>, _force: bool) -> ChannelResult<bool> {
        Ok(true)
    }

    fn default_config() -> serde_json::Map<String, serde_json::Value>
    where
        Self: Sized,
    {
        let config = WecomConfig::default();
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
        let config = WecomChannel::default_config();
        assert_eq!(config.get("enabled").and_then(|v| v.as_bool()), Some(false));
    }

    #[test]
    fn test_name() {
        assert_eq!(WecomChannel::name(), "wecom");
    }

    #[test]
    fn test_display_name() {
        assert_eq!(WecomChannel::display_name(), "WeCom");
    }
}
