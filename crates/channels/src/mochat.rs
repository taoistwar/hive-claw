/// Mochat channel implementation.
///
/// Uses Socket.IO + HTTP polling fallback in Python.
/// This is a skeleton implementation with TODOs for the full integration.
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use log::{error, info, warn};
use serde::{Deserialize, Serialize};
use serde_json::Map;

use bus::MessageBus;
use bus::OutboundMessage;
use serde_json::Value;

use crate::base::{Channel, ChannelError, ChannelResult, TranscriptionSettings, handle_inbound};
use crate::registry::ChannelEntry;

/// Mochat channel configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MochatConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub server_url: String,
    #[serde(default)]
    pub token: String,
    #[serde(default)]
    pub allow_from: Vec<String>,
}

impl Default for MochatConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            server_url: String::new(),
            token: String::new(),
            allow_from: Vec::new(),
        }
    }
}

/// Mochat channel using Socket.IO.
pub struct MochatChannel {
    config: MochatConfig,
    bus: MessageBus,
    running: Arc<AtomicBool>,
    transcription: TranscriptionSettings,
}

impl MochatChannel {
    pub fn from_value(
        value: Value,
        bus: MessageBus,
        transcription: TranscriptionSettings,
    ) -> Result<Self, String> {
        let config: MochatConfig =
            serde_json::from_value(value).map_err(|e| format!("invalid mochat config: {}", e))?;
        Ok(Self {
            config,
            bus,
            running: Arc::new(AtomicBool::new(false)),
            transcription,
        })
    }
}

#[async_trait]
impl Channel for MochatChannel {
    fn name() -> &'static str {
        "mochat"
    }
    fn display_name() -> &'static str {
        "Mochat"
    }
    fn bus(&self) -> &MessageBus {
        &self.bus
    }
    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    async fn start(self: Arc<Self>) -> ChannelResult<()> {
        if self.config.server_url.is_empty() {
            error!("Mochat server_url not configured");
            return Err(ChannelError::Config("server_url required".into()));
        }
        self.running.store(true, Ordering::SeqCst);
        info!("Mochat channel started");

        while self.running.load(Ordering::SeqCst) {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
        Ok(())
    }

    async fn stop(self: Arc<Self>) -> ChannelResult<()> {
        self.running.store(false, Ordering::SeqCst);
        info!("Mochat channel stopped");
        Ok(())
    }

    async fn send(self: Arc<Self>, _msg: OutboundMessage) -> ChannelResult<()> {
        warn!("Mochat send not yet implemented");
        Err(ChannelError::Other("send not implemented".into()))
    }

    async fn send_delta(
        self: Arc<Self>,
        _chat_id: String,
        _delta: String,
        _metadata: Map<String, serde_json::Value>,
    ) -> ChannelResult<()> {
        warn!("Mochat send_delta not yet implemented");
        Ok(())
    }

    async fn login(self: Arc<Self>, _force: bool) -> ChannelResult<bool> {
        Ok(true)
    }

    fn default_config() -> Map<String, serde_json::Value> {
        let config = MochatConfig::default();
        let value = serde_json::to_value(&config).unwrap_or(serde_json::Value::Object(Map::new()));
        match value {
            serde_json::Value::Object(map) => map,
            _ => Map::new(),
        }
    }
}

pub(crate) fn build(
    section: Value,
    bus: MessageBus,
    transcription: TranscriptionSettings,
) -> Result<ChannelEntry, String> {
    let ch = MochatChannel::from_value(section, bus, transcription)?;
    let arc: Arc<dyn Channel> = Arc::new(ch);
    Ok(ChannelEntry {
        name: "mochat".into(),
        display_name: "Mochat".into(),
        channel: arc,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = MochatChannel::default_config();
        assert_eq!(config.get("enabled").and_then(|v| v.as_bool()), Some(false));
    }

    #[test]
    fn test_name() {
        assert_eq!(MochatChannel::name(), "mochat");
    }

    #[test]
    fn test_display_name() {
        assert_eq!(MochatChannel::display_name(), "Mochat");
    }
}
