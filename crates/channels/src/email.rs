/// Email channel implementation.
///
/// Uses IMAP polling for receiving and SMTP for sending.
/// This is a skeleton implementation with TODOs for the full email integration.
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

/// Email channel configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EmailConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub imap_host: String,
    #[serde(default = "default_imap_port")]
    pub imap_port: u16,
    #[serde(default)]
    pub imap_username: String,
    #[serde(default)]
    pub imap_password: String,
    #[serde(default = "default_true")]
    pub imap_use_ssl: bool,
    #[serde(default)]
    pub smtp_host: String,
    #[serde(default = "default_smtp_port")]
    pub smtp_port: u16,
    #[serde(default)]
    pub smtp_username: String,
    #[serde(default)]
    pub smtp_password: String,
    #[serde(default = "default_true")]
    pub smtp_use_tls: bool,
    #[serde(default)]
    pub from_address: String,
    #[serde(default = "default_poll_interval")]
    pub poll_interval_s: u64,
    #[serde(default)]
    pub allow_from: Vec<String>,
}

fn default_imap_port() -> u16 {
    993
}
fn default_smtp_port() -> u16 {
    587
}
fn default_true() -> bool {
    true
}
fn default_poll_interval() -> u64 {
    30
}

impl Default for EmailConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            imap_host: String::new(),
            imap_port: default_imap_port(),
            imap_username: String::new(),
            imap_password: String::new(),
            imap_use_ssl: default_true(),
            smtp_host: String::new(),
            smtp_port: default_smtp_port(),
            smtp_username: String::new(),
            smtp_password: String::new(),
            smtp_use_tls: default_true(),
            from_address: String::new(),
            poll_interval_s: default_poll_interval(),
            allow_from: Vec::new(),
        }
    }
}

/// Email channel using IMAP/SMTP.
pub struct EmailChannel {
    config: EmailConfig,
    bus: MessageBus,
    running: Arc<AtomicBool>,
    transcription: TranscriptionSettings,
}

impl EmailChannel {
    pub fn from_value(
        value: Value,
        bus: MessageBus,
        transcription: TranscriptionSettings,
    ) -> Result<Self, String> {
        let config: EmailConfig =
            serde_json::from_value(value).map_err(|e| format!("invalid email config: {}", e))?;
        Ok(Self {
            config,
            bus,
            running: Arc::new(AtomicBool::new(false)),
            transcription,
        })
    }
}

#[async_trait]
impl Channel for EmailChannel {
    fn name() -> &'static str {
        "email"
    }
    fn display_name() -> &'static str {
        "Email"
    }
    fn bus(&self) -> &MessageBus {
        &self.bus
    }
    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    async fn start(self: Arc<Self>) -> ChannelResult<()> {
        if self.config.imap_host.is_empty() || self.config.smtp_host.is_empty() {
            error!("Email IMAP/SMTP not configured");
            return Err(ChannelError::Config(
                "imap_host and smtp_host required".into(),
            ));
        }
        self.running.store(true, Ordering::SeqCst);
        info!("Email channel started (IMAP polling + SMTP)");

        let poll_interval = std::time::Duration::from_secs(self.config.poll_interval_s);
        while self.running.load(Ordering::SeqCst) {
            tokio::time::sleep(poll_interval).await;
        }
        Ok(())
    }

    async fn stop(self: Arc<Self>) -> ChannelResult<()> {
        self.running.store(false, Ordering::SeqCst);
        info!("Email channel stopped");
        Ok(())
    }

    async fn send(self: Arc<Self>, _msg: OutboundMessage) -> ChannelResult<()> {
        warn!("Email send not yet implemented");
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
        let config = EmailConfig::default();
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
    let ch = EmailChannel::from_value(section, bus, transcription)?;
    let arc: Arc<dyn Channel> = Arc::new(ch);
    Ok(ChannelEntry {
        name: "email".into(),
        display_name: "Email".into(),
        channel: arc,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = EmailChannel::default_config();
        assert_eq!(config.get("enabled").and_then(|v| v.as_bool()), Some(false));
    }

    #[test]
    fn test_name() {
        assert_eq!(EmailChannel::name(), "email");
    }

    #[test]
    fn test_display_name() {
        assert_eq!(EmailChannel::display_name(), "Email");
    }
}
