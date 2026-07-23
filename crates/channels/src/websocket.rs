/// Generic WebSocket server channel.
///
/// Nanobot acts as a WebSocket server serving connected clients.
/// This is a highly complex channel with many features - skeleton implementation with TODOs.
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use log::{info, warn};
use serde::{Deserialize, Serialize};
use serde_json::Map;

use bus::MessageBus;
use bus::OutboundMessage;
use serde_json::Value;

use crate::base::{Channel, ChannelError, ChannelResult, TranscriptionSettings};
use crate::registry::ChannelEntry;

/// WebSocket server channel configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WebSocketConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub token: String,
    #[serde(default)]
    pub token_issue_path: String,
    #[serde(default)]
    pub token_issue_secret: String,
    #[serde(default = "default_token_ttl")]
    pub token_ttl_s: u64,
    #[serde(default = "default_true")]
    pub websocket_requires_token: bool,
    #[serde(default)]
    pub allow_from: Vec<String>,
    #[serde(default = "default_true")]
    pub streaming: bool,
    #[serde(default = "default_max_bytes")]
    pub max_message_bytes: usize,
    #[serde(default = "default_ping_interval")]
    pub ping_interval_s: f64,
    #[serde(default = "default_ping_timeout")]
    pub ping_timeout_s: f64,
    #[serde(default)]
    pub ssl_certfile: String,
    #[serde(default)]
    pub ssl_keyfile: String,
    #[serde(default)]
    pub transcription: Option<serde_json::Value>,
}

fn default_port() -> u16 {
    8765
}
fn default_true() -> bool {
    true
}
fn default_token_ttl() -> u64 {
    300
}
fn default_max_bytes() -> usize {
    37_748_736
}
fn default_ping_interval() -> f64 {
    20.0
}
fn default_ping_timeout() -> f64 {
    20.0
}

impl Default for WebSocketConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            host: "127.0.0.1".to_string(),
            port: default_port(),
            path: "/".to_string(),
            token: String::new(),
            token_issue_path: String::new(),
            token_issue_secret: String::new(),
            token_ttl_s: default_token_ttl(),
            websocket_requires_token: default_true(),
            allow_from: vec!["*".to_string()],
            streaming: default_true(),
            max_message_bytes: default_max_bytes(),
            ping_interval_s: default_ping_interval(),
            ping_timeout_s: default_ping_timeout(),
            ssl_certfile: String::new(),
            ssl_keyfile: String::new(),
            transcription: None,
        }
    }
}

/// WebSocket server channel.
pub struct WebSocketChannel {
    #[expect(
        dead_code,
        reason = "reserved for staged WebSocket server transport integration"
    )]
    config: WebSocketConfig,
    bus: MessageBus,
    running: Arc<AtomicBool>,
    #[expect(
        dead_code,
        reason = "reserved for staged inbound media transcription integration"
    )]
    transcription: TranscriptionSettings,
}

impl WebSocketChannel {
    pub fn from_value(
        value: Value,
        bus: MessageBus,
        transcription: TranscriptionSettings,
    ) -> Result<Self, String> {
        let config: WebSocketConfig = serde_json::from_value(value)
            .map_err(|e| format!("invalid websocket config: {}", e))?;
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
    let ch = WebSocketChannel::from_value(section, bus, transcription)?;
    let arc: Arc<dyn Channel> = Arc::new(ch);
    Ok(ChannelEntry {
        name: "websocket".into(),
        display_name: "WebSocket".into(),
        channel: arc,
    })
}

#[async_trait]
impl Channel for WebSocketChannel {
    fn name() -> &'static str {
        "websocket"
    }

    fn display_name() -> &'static str {
        "WebSocket"
    }

    fn bus(&self) -> &MessageBus {
        &self.bus
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    async fn start(self: Arc<Self>) -> ChannelResult<()> {
        self.running.store(true, Ordering::SeqCst);
        info!("WebSocket channel started");

        // TODO: Implement full WebSocket server channel
        // The Python version uses the websockets library with extensive features
        // This is one of the most complex channels. Features needed:
        //
        // Core:
        // - WebSocket server with TLS support
        // - Token-based authentication (static + issued tokens)
        // - Client connection management
        // - Subscription bookkeeping (chat_id -> connections fan-out)
        // - Envelope parsing (new_chat, attach, message types)
        // - Media ingestion from data URLs
        //
        // HTTP routes co-located with WS:
        // - Bootstrap endpoint (/webui/bootstrap)
        // - Session management (list, messages, delete)
        // - Settings API (read, update, provider, web-search, image-generation)
        // - Sidebar state API
        // - Commands API
        // - Signed media fetch endpoint
        // - Static SPA serving
        //
        // Outbound:
        // - Message fan-out to subscribers
        // - Delta streaming
        // - Reasoning delta/end
        // - Turn end signals
        // - Goal state/status updates
        // - Session update notifications
        // - Runtime model update broadcasts
        // - File edit events
        // - Signed media URL generation
        //
        // Security:
        // - HMAC-signed media URLs
        // - Token issue with TTL
        // - Allowlist authorization
        // - Path traversal prevention

        while self.running.load(Ordering::SeqCst) {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }

        Ok(())
    }

    async fn stop(self: Arc<Self>) -> ChannelResult<()> {
        self.running.store(false, Ordering::SeqCst);
        info!("WebSocket channel stopped");
        Ok(())
    }

    async fn send(self: Arc<Self>, _msg: OutboundMessage) -> ChannelResult<()> {
        // TODO: Implement WebSocket message fan-out
        warn!("WebSocket send not yet implemented");
        Err(ChannelError::Other("send not implemented".into()))
    }

    async fn send_delta(
        self: Arc<Self>,
        _chat_id: String,
        _delta: String,
        _metadata: Map<String, serde_json::Value>,
    ) -> ChannelResult<()> {
        // TODO: Implement WebSocket delta streaming
        warn!("WebSocket send_delta not yet implemented");
        Ok(())
    }

    async fn login(self: Arc<Self>, _force: bool) -> ChannelResult<bool> {
        Ok(true)
    }

    fn default_config() -> serde_json::Map<String, serde_json::Value>
    where
        Self: Sized,
    {
        let config = WebSocketConfig::default();
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
        let config = WebSocketChannel::default_config();
        assert_eq!(config.get("enabled").and_then(|v| v.as_bool()), Some(false));
    }

    #[test]
    fn test_name() {
        assert_eq!(WebSocketChannel::name(), "websocket");
    }

    #[test]
    fn test_display_name() {
        assert_eq!(WebSocketChannel::display_name(), "WebSocket");
    }
}
