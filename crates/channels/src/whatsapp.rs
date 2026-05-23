/// WhatsApp channel implementation using Node.js bridge.
///
/// Connects to a Node.js bridge (@whiskeysockets/baileys) via WebSocket.
/// This is a skeleton implementation with TODOs.

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

/// WhatsApp channel configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WhatsAppConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_bridge_url")]
    pub bridge_url: String,
    #[serde(default)]
    pub bridge_token: String,
    #[serde(default)]
    pub allow_from: Vec<String>,
    #[serde(default)]
    pub group_policy: String,
    #[serde(default)]
    pub transcription: Option<serde_json::Value>,
}

fn default_bridge_url() -> String { "ws://localhost:3001".to_string() }

impl Default for WhatsAppConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            bridge_url: default_bridge_url(),
            bridge_token: String::new(),
            allow_from: Vec::new(),
            group_policy: "open".to_string(),
            transcription: None,
        }
    }
}

/// WhatsApp channel connecting to a Node.js bridge.
pub struct WhatsAppChannel {
    config: WhatsAppConfig,
    bus: MessageBus,
    running: Arc<AtomicBool>,
    transcription: TranscriptionSettings,
}

impl WhatsAppChannel {
    pub fn from_value(
        value: Value,
        bus: MessageBus,
        transcription: TranscriptionSettings,
    ) -> Result<Self, String> {
        let config: WhatsAppConfig = serde_json::from_value(value)
            .map_err(|e| format!("invalid whatsapp config: {}", e))?;
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
    let ch = WhatsAppChannel::from_value(section, bus, transcription)?;
    let arc: Arc<dyn Channel> = Arc::new(ch);
    Ok(ChannelEntry {
        name: "whatsapp".into(),
        display_name: "WhatsApp".into(),
        channel: arc,
    })
}

#[async_trait]
impl Channel for WhatsAppChannel {
    fn name(&self) -> &'static str {
        "whatsapp"
    }

    fn display_name(&self) -> &'static str {
        "WhatsApp"
    }

    fn bus(&self) -> &MessageBus {
        &self.bus
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    async fn start(self: Arc<Self>) -> ChannelResult<()> {
        self.running.store(true, Ordering::SeqCst);
        info!("WhatsApp channel started");

        // TODO: Implement WhatsApp channel using a Rust WebSocket client to bridge
        // The Python version connects to a Node.js bridge (@whiskeysockets/baileys)
        // Features needed:
        // - WebSocket connection to Node.js bridge
        // - Auth handshake with bridge token
        // - Auto-reconnect on connection loss
        // - Message parsing (phone ID, LID resolution)
        // - Media handling (downloaded by bridge, paths in message)
        // - Voice transcription
        // - Group policy (open/mention)
        // - Message deduplication
        // - Bridge process management (npm start for QR login)
        // - Token persistence

        while self.running.load(Ordering::SeqCst) {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }

        Ok(())
    }

    async fn stop(self: Arc<Self>) -> ChannelResult<()> {
        self.running.store(false, Ordering::SeqCst);
        info!("WhatsApp channel stopped");
        Ok(())
    }

    async fn send(self: Arc<Self>, _msg: OutboundMessage) -> ChannelResult<()> {
        // TODO: Implement WhatsApp message sending via bridge
        warn!("WhatsApp send not yet implemented");
        Err(ChannelError::Other("send not implemented".into()))
    }

    async fn send_delta(
        self: Arc<Self>,
        _chat_id: String,
        _delta: String,
        _metadata: Map<String, serde_json::Value>,
    ) -> ChannelResult<()> {
        // WhatsApp doesn't support streaming
        Ok(())
    }

    async fn login(self: Arc<Self>, _force: bool) -> ChannelResult<bool> {
        // TODO: Implement QR code login via bridge process
        Ok(true)
    }

    fn default_config() -> serde_json::Map<String, serde_json::Value>
    where
        Self: Sized,
    {
        let config = WhatsAppConfig::default();
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
        let config = WhatsAppChannel::default_config();
        assert_eq!(config.get("enabled").and_then(|v| v.as_bool()), Some(false));
    }

    #[test]
    fn test_name() {
        assert_eq!(WhatsAppChannel::name(), "whatsapp");
    }

    #[test]
    fn test_display_name() {
        assert_eq!(WhatsAppChannel::display_name(), "WhatsApp");
    }
}
