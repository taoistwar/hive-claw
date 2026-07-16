//! Base channel interface for chat platforms (Rust port of
//! `nanobot.channels.base`).

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use log::warn;
use serde_json::Value;

use bus::{InboundMessage, MessageBus, OutboundMessage};
use providers::transcription::{
    GroqTranscriptionProvider, OpenAITranscriptionProvider, TranscriptionProvider,
};

/// Stream-related metadata keys (mirrors the Python contract).
pub const STREAM_DELTA_KEY: &str = "_stream_delta";
pub const STREAM_END_KEY: &str = "_stream_end";
pub const STREAM_ID_KEY: &str = "_stream_id";
pub const WANTS_STREAM_KEY: &str = "_wants_stream";

/// Cross-channel transcription configuration injected by `ChannelManager`.
///
/// Each channel keeps its own copy so the [`Channel::transcribe_audio`]
/// helper can spin up the right backend without going back to the
/// manager.
#[derive(Debug, Clone, Default)]
pub struct TranscriptionSettings {
    pub provider: String,
    pub api_key: String,
    pub api_base: String,
    pub language: Option<String>,
}

/// Information every concrete channel exposes to the manager / status
/// endpoints.
#[derive(Debug, Clone)]
pub struct ChannelInfo {
    pub name: &'static str,
    pub display_name: &'static str,
}

/// Result returned by [`Channel::send`] and friends.
pub type ChannelResult<T = ()> = Result<T, ChannelError>;

/// Errors a channel implementation can surface.
#[derive(Debug, thiserror::Error)]
pub enum ChannelError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("invalid configuration: {0}")]
    Config(String),
    #[error("authentication required")]
    NotAuthenticated,
    #[error("network/transient error: {0}")]
    Transient(String),
    #[error("non-retryable error: {0}")]
    Permanent(String),
    #[error("{0}")]
    Other(String),
}

impl ChannelError {
    /// True when the channel manager should retry per the configured
    /// exponential backoff.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            ChannelError::Io(_) | ChannelError::Http(_) | ChannelError::Transient(_)
        )
    }
}

/// Async trait implemented by every chat channel.
///
/// Roughly equivalent to the Python `BaseChannel` ABC. The default
/// helpers (permission check, transcription helper, `_handle_message`)
/// live as concrete methods on the trait so subclasses don't have to
/// re-implement them.
#[async_trait]
pub trait Channel: Send + Sync {
    /// Static channel name (e.g. `"weixin"`).
    fn name() -> &'static str
    where
        Self: Sized;

    /// Human-readable name (e.g. `"WeChat"`).
    fn display_name() -> &'static str
    where
        Self: Sized;

    /// Reference to the message bus.
    fn bus(&self) -> &MessageBus;

    /// `True` once the channel's `start()` loop is up and running.
    fn is_running(&self) -> bool;

    /// Streaming configuration knob (mirrors `cfg.streaming`).
    fn streaming_enabled(&self) -> bool {
        false
    }

    /// Begin the long-running listen loop. Implementations should:
    ///
    /// 1. Connect to the chat platform.
    /// 2. Listen for inbound messages.
    /// 3. Forward them to the bus via [`Channel::handle_inbound`].
    async fn start(self: Arc<Self>) -> ChannelResult<()>;

    /// Tear down the channel and any background work.
    async fn stop(self: Arc<Self>) -> ChannelResult<()>;

    /// Send an outbound message. Network failures should bubble up as
    /// retryable errors so the channel manager can apply its retry
    /// policy.
    async fn send(self: Arc<Self>, msg: OutboundMessage) -> ChannelResult<()>;

    /// Override to enable streaming text deltas.
    async fn send_delta(
        self: Arc<Self>,
        _chat_id: String,
        _delta: String,
        _metadata: serde_json::Map<String, Value>,
    ) -> ChannelResult<()> {
        // Default no-op (Python contract: subclasses opt in by overriding).
        Ok(())
    }

    /// `True` when streaming is enabled in config AND the subclass
    /// actually overrides [`Channel::send_delta`]. Subclasses that
    /// implement streaming should override this to return
    /// `self.streaming_enabled()`.
    fn supports_streaming(&self) -> bool {
        false
    }

    /// Optional interactive login (e.g. QR code scan). Defaults to
    /// "already authenticated".
    async fn login(self: Arc<Self>, _force: bool) -> ChannelResult<bool> {
        Ok(true)
    }

    /// Return default config for onboard. Override in concrete channel
    /// implementations to auto-populate config.json (matches Python's
    /// `BaseChannel.default_config`).
    fn default_config() -> serde_json::Map<String, Value>
    where
        Self: Sized,
    {
        serde_json::Map::new()
    }
}

/// Default permission check shared between channels (matches Python's
/// `is_allowed`).
///
/// * empty list  → deny all and emit a warning
/// * `["*"]`     → allow everyone
/// * otherwise   → string-equal match against `sender_id`
pub fn is_allowed(channel_name: &str, allow_list: &[String], sender_id: &str) -> bool {
    if allow_list.is_empty() {
        warn!("{channel_name}: allow_from is empty — all access denied");
        return false;
    }
    if allow_list.iter().any(|s| s == "*") {
        return true;
    }
    allow_list.iter().any(|s| s == sender_id)
}

/// Whisper-compatible audio transcription helper used by every channel.
/// Returns an empty string on any failure.
pub async fn transcribe_audio(
    name: &str,
    settings: &TranscriptionSettings,
    file_path: impl AsRef<Path>,
) -> String {
    if settings.api_key.is_empty() {
        return String::new();
    }
    let lang = settings.language.as_deref();
    let api_base = if settings.api_base.is_empty() {
        None
    } else {
        Some(settings.api_base.as_str())
    };
    let key = Some(settings.api_key.as_str());

    let provider: Box<dyn TranscriptionProvider> = if settings.provider == "openai" {
        Box::new(OpenAITranscriptionProvider::new(key, api_base, lang))
    } else {
        Box::new(GroqTranscriptionProvider::new(key, api_base, lang))
    };

    // Wrap in catch_unwind-style soft error: any provider failure → "".
    let path: &Path = file_path.as_ref();
    let result = provider.transcribe(path).await;
    if result.is_empty() {
        log::debug!("{name}: transcription returned empty text");
    }
    result
}

/// Default routine that forwards an inbound message to the bus after
/// permission check. Mirrors `BaseChannel._handle_message`.
#[allow(clippy::too_many_arguments)]
pub async fn handle_inbound(
    bus: &MessageBus,
    channel_name: &str,
    allow_list: &[String],
    supports_streaming: bool,
    sender_id: impl Into<String>,
    chat_id: impl Into<String>,
    content: impl Into<String>,
    media: Vec<String>,
    metadata: Option<serde_json::Map<String, Value>>,
    session_key: Option<String>,
) {
    let sender_id = sender_id.into();
    let chat_id = chat_id.into();
    if !is_allowed(channel_name, allow_list, &sender_id) {
        warn!(
            "Access denied for sender {sender_id} on channel {channel_name}. \
             Add them to allowFrom list in config to grant access."
        );
        return;
    }

    let mut meta = metadata.unwrap_or_default();
    if supports_streaming {
        meta.insert(WANTS_STREAM_KEY.into(), Value::Bool(true));
    }

    let mut metadata_map: std::collections::HashMap<String, Value> =
        std::collections::HashMap::new();
    for (k, v) in meta {
        metadata_map.insert(k, v);
    }

    let msg = InboundMessage {
        channel: channel_name.to_string(),
        sender_id,
        chat_id,
        content: content.into(),
        media,
        metadata: metadata_map,
        session_key_override: session_key,
        ..Default::default()
    };
    bus.publish_inbound(msg).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allow_empty_denies() {
        assert!(!is_allowed("x", &[], "u1"));
    }

    #[test]
    fn allow_star_passes_all() {
        let list = vec!["*".to_string()];
        assert!(is_allowed("x", &list, "anyone"));
    }

    #[test]
    fn allow_specific_matches() {
        let list = vec!["a".to_string(), "b".to_string()];
        assert!(is_allowed("x", &list, "a"));
        assert!(!is_allowed("x", &list, "c"));
    }
}
