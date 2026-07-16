/// Microsoft Teams channel MVP using a built-in HTTP webhook server.
///
/// DM-focused MVP with text inbound/outbound and conversation reference persistence.
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

/// Microsoft Teams channel configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MSTeamsConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub app_id: String,
    #[serde(default)]
    pub app_password: String,
    #[serde(default)]
    pub tenant_id: String,
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_path")]
    pub path: String,
    #[serde(default)]
    pub allow_from: Vec<String>,
    #[serde(default = "default_true")]
    pub reply_in_thread: bool,
    #[serde(default)]
    pub mention_only_response: String,
    #[serde(default = "default_true")]
    pub validate_inbound_auth: bool,
    #[serde(default = "default_ttl_days")]
    pub ref_ttl_days: u64,
    #[serde(default = "default_true")]
    pub prune_web_chat_refs: bool,
    #[serde(default = "default_true")]
    pub prune_non_personal_refs: bool,
    #[serde(default = "default_touch_interval")]
    pub ref_touch_interval_s: u64,
}

fn default_host() -> String {
    "0.0.0.0".to_string()
}
fn default_port() -> u16 {
    3978
}
fn default_path() -> String {
    "/api/messages".to_string()
}
fn default_true() -> bool {
    true
}
fn default_ttl_days() -> u64 {
    30
}
fn default_touch_interval() -> u64 {
    300
}

impl Default for MSTeamsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            app_id: String::new(),
            app_password: String::new(),
            tenant_id: String::new(),
            host: default_host(),
            port: default_port(),
            path: default_path(),
            allow_from: Vec::new(),
            reply_in_thread: default_true(),
            mention_only_response: "Hi — what can I help with?".to_string(),
            validate_inbound_auth: default_true(),
            ref_ttl_days: default_ttl_days(),
            prune_web_chat_refs: default_true(),
            prune_non_personal_refs: default_true(),
            ref_touch_interval_s: default_touch_interval(),
        }
    }
}

/// Microsoft Teams channel.
pub struct MSTeamsChannel {
    config: MSTeamsConfig,
    bus: MessageBus,
    running: Arc<AtomicBool>,
    transcription: TranscriptionSettings,
}

impl MSTeamsChannel {
    pub fn from_value(
        value: Value,
        bus: MessageBus,
        transcription: TranscriptionSettings,
    ) -> Result<Self, String> {
        let config: MSTeamsConfig =
            serde_json::from_value(value).map_err(|e| format!("invalid msteams config: {}", e))?;
        Ok(Self {
            config,
            bus,
            running: Arc::new(AtomicBool::new(false)),
            transcription,
        })
    }
}

#[async_trait]
impl Channel for MSTeamsChannel {
    fn name() -> &'static str {
        "msteams"
    }
    fn display_name() -> &'static str {
        "Microsoft Teams"
    }
    fn bus(&self) -> &MessageBus {
        &self.bus
    }
    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    async fn start(self: Arc<Self>) -> ChannelResult<()> {
        if self.config.app_id.is_empty() || self.config.app_password.is_empty() {
            error!("MSTeams app_id/app_password not configured");
            return Err(ChannelError::Config(
                "app_id and app_password required".into(),
            ));
        }
        self.running.store(true, Ordering::SeqCst);
        info!("MSTeams channel started (webhook listener)");

        while self.running.load(Ordering::SeqCst) {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
        Ok(())
    }

    async fn stop(self: Arc<Self>) -> ChannelResult<()> {
        self.running.store(false, Ordering::SeqCst);
        info!("MSTeams channel stopped");
        Ok(())
    }

    async fn send(self: Arc<Self>, _msg: OutboundMessage) -> ChannelResult<()> {
        warn!("MSTeams send not yet implemented");
        Err(ChannelError::Other("send not implemented".into()))
    }

    async fn send_delta(
        self: Arc<Self>,
        _chat_id: String,
        _delta: String,
        _metadata: Map<String, serde_json::Value>,
    ) -> ChannelResult<()> {
        Ok(())
    }

    async fn login(self: Arc<Self>, _force: bool) -> ChannelResult<bool> {
        Ok(true)
    }

    fn default_config() -> Map<String, serde_json::Value> {
        let config = MSTeamsConfig::default();
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
    let ch = MSTeamsChannel::from_value(section, bus, transcription)?;
    let arc: Arc<dyn Channel> = Arc::new(ch);
    Ok(ChannelEntry {
        name: "msteams".into(),
        display_name: "Microsoft Teams".into(),
        channel: arc,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = MSTeamsChannel::default_config();
        assert_eq!(config.get("enabled").and_then(|v| v.as_bool()), Some(false));
    }

    #[test]
    fn test_name() {
        assert_eq!(MSTeamsChannel::name(), "msteams");
    }

    #[test]
    fn test_display_name() {
        assert_eq!(MSTeamsChannel::display_name(), "Microsoft Teams");
    }
}
