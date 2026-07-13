/// Slack channel implementation using Socket Mode.
///
/// Uses the slack-sdk in Python; this is a skeleton implementation with TODOs
/// for the full Rust Slack SDK integration.
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

/// Slack DM policy configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SlackDMConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub policy: String,
    #[serde(default)]
    pub allow_from: Vec<String>,
}

fn default_true() -> bool {
    true
}

impl Default for SlackDMConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            policy: "open".to_string(),
            allow_from: Vec::new(),
        }
    }
}

/// Slack channel configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SlackConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub webhook_path: String,
    #[serde(default)]
    pub bot_token: String,
    #[serde(default)]
    pub app_token: String,
    #[serde(default = "default_true")]
    pub user_token_read_only: bool,
    #[serde(default = "default_true")]
    pub reply_in_thread: bool,
    #[serde(default)]
    pub react_emoji: String,
    #[serde(default)]
    pub done_emoji: String,
    #[serde(default = "default_true")]
    pub include_thread_context: bool,
    #[serde(default = "default_thread_limit")]
    pub thread_context_limit: usize,
    #[serde(default)]
    pub allow_from: Vec<String>,
    #[serde(default)]
    pub group_policy: String,
    #[serde(default)]
    pub group_allow_from: Vec<String>,
    #[serde(default)]
    pub dm: SlackDMConfig,
    #[serde(default)]
    pub transcription: Option<serde_json::Value>,
}

fn default_thread_limit() -> usize {
    20
}

impl Default for SlackConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: "socket".to_string(),
            webhook_path: "/slack/events".to_string(),
            bot_token: String::new(),
            app_token: String::new(),
            user_token_read_only: default_true(),
            reply_in_thread: default_true(),
            react_emoji: "eyes".to_string(),
            done_emoji: "white_check_mark".to_string(),
            include_thread_context: default_true(),
            thread_context_limit: default_thread_limit(),
            allow_from: Vec::new(),
            group_policy: "mention".to_string(),
            group_allow_from: Vec::new(),
            dm: SlackDMConfig::default(),
            transcription: None,
        }
    }
}

/// Slack channel using Socket Mode.
pub struct SlackChannel {
    config: SlackConfig,
    bus: MessageBus,
    running: Arc<AtomicBool>,
    transcription: TranscriptionSettings,
}

impl SlackChannel {
    pub fn from_value(
        value: Value,
        bus: MessageBus,
        transcription: TranscriptionSettings,
    ) -> Result<Self, String> {
        let config: SlackConfig =
            serde_json::from_value(value).map_err(|e| format!("invalid slack config: {}", e))?;
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
    let ch = SlackChannel::from_value(section, bus, transcription)?;
    let arc: Arc<dyn Channel> = Arc::new(ch);
    Ok(ChannelEntry {
        name: "slack".into(),
        display_name: "Slack".into(),
        channel: arc,
    })
}

#[async_trait]
impl Channel for SlackChannel {
    fn name() -> &'static str {
        "slack"
    }

    fn display_name() -> &'static str {
        "Slack"
    }

    fn bus(&self) -> &MessageBus {
        &self.bus
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    async fn start(self: Arc<Self>) -> ChannelResult<()> {
        if self.config.bot_token.is_empty() || self.config.app_token.is_empty() {
            error!("Slack bot_token/app_token not configured");
            return Err(ChannelError::Config(
                "bot_token and app_token required".into(),
            ));
        }

        if self.config.mode != "socket" {
            error!("Unsupported Slack mode: {}", self.config.mode);
            return Err(ChannelError::Config(format!(
                "Unsupported mode: {}",
                self.config.mode
            )));
        }

        self.running.store(true, Ordering::SeqCst);
        info!("Slack channel started (Socket Mode)");

        // TODO: Implement Slack Socket Mode using a Rust Slack SDK
        // The Python version uses slack_sdk with Socket Mode
        // Features needed:
        // - Socket Mode WebSocket connection
        // - Event handling (messages, app_mention, block actions)
        // - Bot user ID resolution via auth_test
        // - Target resolution (channel names, user handles -> IDs)
        // - DM opening for users
        // - File download
        // - Thread context fetching
        // - Reaction management (eyes / done)
        // - Markdown to mrkdwn conversion (including tables)
        // - Button block kit building

        while self.running.load(Ordering::SeqCst) {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }

        Ok(())
    }

    async fn stop(self: Arc<Self>) -> ChannelResult<()> {
        self.running.store(false, Ordering::SeqCst);
        info!("Slack channel stopped");
        Ok(())
    }

    async fn send(self: Arc<Self>, _msg: OutboundMessage) -> ChannelResult<()> {
        // TODO: Implement Slack message sending
        warn!("Slack send not yet implemented");
        Err(ChannelError::Other("send not implemented".into()))
    }

    async fn send_delta(
        self: Arc<Self>,
        _chat_id: String,
        _delta: String,
        _metadata: Map<String, serde_json::Value>,
    ) -> ChannelResult<()> {
        // TODO: Implement Slack message editing for streaming
        warn!("Slack send_delta not yet implemented");
        Ok(())
    }

    async fn login(self: Arc<Self>, _force: bool) -> ChannelResult<bool> {
        Ok(true)
    }

    fn default_config() -> serde_json::Map<String, serde_json::Value>
    where
        Self: Sized,
    {
        let config = SlackConfig::default();
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
        let config = SlackChannel::default_config();
        assert_eq!(config.get("enabled").and_then(|v| v.as_bool()), Some(false));
    }

    #[test]
    fn test_name() {
        assert_eq!(SlackChannel::name(), "slack");
    }

    #[test]
    fn test_display_name() {
        assert_eq!(SlackChannel::display_name(), "Slack");
    }
}
