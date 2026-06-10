/// Matrix (Element) channel implementation.
///
/// Uses the nio SDK in Python; this is a skeleton implementation with TODOs
/// for the full ruma/matrix-sdk integration.

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

/// Matrix channel configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MatrixConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub homeserver_url: String,
    #[serde(default)]
    pub user_id: String,
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub access_token: String,
    #[serde(default)]
    pub device_id: String,
    #[serde(default)]
    pub allow_from: Vec<String>,
    #[serde(default)]
    pub e2ee: bool,
    #[serde(default)]
    pub group_policy: String,
}

impl Default for MatrixConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            homeserver_url: String::new(),
            user_id: String::new(),
            password: String::new(),
            access_token: String::new(),
            device_id: String::new(),
            allow_from: Vec::new(),
            e2ee: false,
            group_policy: "mention".to_string(),
        }
    }
}

/// Matrix channel using the Matrix SDK.
pub struct MatrixChannel {
    config: MatrixConfig,
    bus: MessageBus,
    running: Arc<AtomicBool>,
    transcription: TranscriptionSettings,
}

impl MatrixChannel {
    pub fn from_value(
        value: Value,
        bus: MessageBus,
        transcription: TranscriptionSettings,
    ) -> Result<Self, String> {
        let config: MatrixConfig = serde_json::from_value(value)
            .map_err(|e| format!("invalid matrix config: {}", e))?;
        Ok(Self {
            config,
            bus,
            running: Arc::new(AtomicBool::new(false)),
            transcription,
        })
    }
}

#[async_trait]
impl Channel for MatrixChannel {
    fn name() -> &'static str { "matrix" }
    fn display_name() -> &'static str { "Matrix" }
    fn bus(&self) -> &MessageBus { &self.bus }
    fn is_running(&self) -> bool { self.running.load(Ordering::SeqCst) }

    async fn start(self: Arc<Self>) -> ChannelResult<()> {
        if self.config.homeserver_url.is_empty() || self.config.user_id.is_empty() {
            error!("Matrix homeserver_url/user_id not configured");
            return Err(ChannelError::Config("homeserver_url and user_id required".into()));
        }
        self.running.store(true, Ordering::SeqCst);
        info!("Matrix channel started");

        while self.running.load(Ordering::SeqCst) {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
        Ok(())
    }

    async fn stop(self: Arc<Self>) -> ChannelResult<()> {
        self.running.store(false, Ordering::SeqCst);
        info!("Matrix channel stopped");
        Ok(())
    }

    async fn send(self: Arc<Self>, _msg: OutboundMessage) -> ChannelResult<()> {
        warn!("Matrix send not yet implemented");
        Err(ChannelError::Other("send not implemented".into()))
    }

    async fn send_delta(
        self: Arc<Self>,
        _chat_id: String,
        _delta: String,
        _metadata: Map<String, serde_json::Value>,
    ) -> ChannelResult<()> {
        warn!("Matrix send_delta not yet implemented");
        Ok(())
    }

    async fn login(self: Arc<Self>, _force: bool) -> ChannelResult<bool> { Ok(true) }

    fn default_config() -> Map<String, serde_json::Value> {
        let config = MatrixConfig::default();
        let value = serde_json::to_value(&config).unwrap_or(serde_json::Value::Object(Map::new()));
        match value { serde_json::Value::Object(map) => map, _ => Map::new() }
    }
}

pub(crate) fn build(
    section: Value,
    bus: MessageBus,
    transcription: TranscriptionSettings,
) -> Result<ChannelEntry, String> {
    let ch = MatrixChannel::from_value(section, bus, transcription)?;
    let arc: Arc<dyn Channel> = Arc::new(ch);
    Ok(ChannelEntry {
        name: "matrix".into(),
        display_name: "Matrix".into(),
        channel: arc,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = MatrixChannel::default_config();
        assert_eq!(config.get("enabled").and_then(|v| v.as_bool()), Some(false));
    }

    #[test]
    fn test_name() {
        assert_eq!(MatrixChannel::name(), "matrix");
    }

    #[test]
    fn test_display_name() {
        assert_eq!(MatrixChannel::display_name(), "Matrix");
    }
}
