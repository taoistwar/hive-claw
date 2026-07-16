/// DingTalk (钉钉) channel using Stream Mode.
///
/// Stream Mode connects via WebSocket to receive events and uses HTTP API to send messages.
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use log::{debug, error, info, warn};
use serde::{Deserialize, Serialize};
use serde_json::Map;

use bus::MessageBus;
use bus::OutboundMessage;
use serde_json::Value;

use crate::base::{Channel, ChannelError, ChannelResult, TranscriptionSettings, handle_inbound};
use crate::registry::ChannelEntry;

/// DingTalk channel configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DingTalkConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub client_id: String,
    #[serde(default)]
    pub client_secret: String,
    #[serde(default)]
    pub allow_from: Vec<String>,
}

impl Default for DingTalkConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            client_id: String::new(),
            client_secret: String::new(),
            allow_from: Vec::new(),
        }
    }
}

/// DingTalk stream mode channel.
pub struct DingTalkChannel {
    config: DingTalkConfig,
    bus: MessageBus,
    running: Arc<AtomicBool>,
    transcription: TranscriptionSettings,
    access_token: tokio::sync::Mutex<Option<(String, f64)>>,
}

impl DingTalkChannel {
    pub fn from_value(
        value: Value,
        bus: MessageBus,
        transcription: TranscriptionSettings,
    ) -> Result<Self, String> {
        let config: DingTalkConfig =
            serde_json::from_value(value).map_err(|e| format!("invalid dingtalk config: {}", e))?;
        Ok(Self {
            config,
            bus,
            running: Arc::new(AtomicBool::new(false)),
            transcription,
            access_token: tokio::sync::Mutex::new(None),
        })
    }

    async fn get_access_token(&self) -> Result<String, String> {
        let mut token_guard = self.access_token.lock().await;
        if let Some((token, expires_at)) = token_guard.as_ref() {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs_f64())
                .unwrap_or(0.0);
            if now < *expires_at - 60.0 {
                return Ok(token.clone());
            }
        }

        warn!("DingTalk access token fetch not yet implemented");
        Err("access token not available".to_string())
    }
}

#[async_trait]
impl Channel for DingTalkChannel {
    fn name() -> &'static str {
        "dingtalk"
    }

    fn display_name() -> &'static str {
        "DingTalk"
    }

    fn bus(&self) -> &MessageBus {
        &self.bus
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    async fn start(self: Arc<Self>) -> ChannelResult<()> {
        if self.config.client_id.is_empty() || self.config.client_secret.is_empty() {
            error!("DingTalk client_id/client_secret not configured");
            return Err(ChannelError::Config(
                "client_id and client_secret required".into(),
            ));
        }

        self.running.store(true, Ordering::SeqCst);
        info!("DingTalk channel started (stream mode)");

        while self.running.load(Ordering::SeqCst) {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }

        Ok(())
    }

    async fn stop(self: Arc<Self>) -> ChannelResult<()> {
        self.running.store(false, Ordering::SeqCst);
        info!("DingTalk channel stopped");
        Ok(())
    }

    async fn send(self: Arc<Self>, _msg: OutboundMessage) -> ChannelResult<()> {
        warn!("DingTalk send not yet implemented");
        Err(ChannelError::Other("send not implemented".into()))
    }

    async fn send_delta(
        self: Arc<Self>,
        _chat_id: String,
        _delta: String,
        _metadata: Map<String, serde_json::Value>,
    ) -> ChannelResult<()> {
        warn!("DingTalk send_delta not yet implemented");
        Ok(())
    }

    async fn login(self: Arc<Self>, _force: bool) -> ChannelResult<bool> {
        Ok(true)
    }

    fn default_config() -> serde_json::Map<String, serde_json::Value>
    where
        Self: Sized,
    {
        let config = DingTalkConfig::default();
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
    let ch = DingTalkChannel::from_value(section, bus, transcription)?;
    let arc: Arc<dyn Channel> = Arc::new(ch);
    Ok(ChannelEntry {
        name: "dingtalk".into(),
        display_name: "DingTalk".into(),
        channel: arc,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = DingTalkChannel::default_config();
        assert_eq!(config.get("enabled").and_then(|v| v.as_bool()), Some(false));
    }

    #[test]
    fn test_name() {
        assert_eq!(DingTalkChannel::name(), "dingtalk");
    }

    #[test]
    fn test_display_name() {
        assert_eq!(DingTalkChannel::display_name(), "DingTalk");
    }
}
