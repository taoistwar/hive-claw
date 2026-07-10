//! Channel manager — coordinates chat channels and routes outbound
//! messages (Rust port of `nanobot.channels.manager`).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use log::{error, info, warn};
use serde_json::Value;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tokio::time::sleep;

use bus::{MessageBus, OutboundMessage};
use config::schema::Config;
use utils::restart::{consume_restart_notice_from_env, format_restart_completed_message};

use crate::base::{Channel, STREAM_DELTA_KEY, STREAM_END_KEY, TranscriptionSettings};
use crate::registry::{ChannelEntry, build_enabled_channels};

/// Retry delays (seconds) for outbound sends — exponential backoff,
/// matching the Python `_SEND_RETRY_DELAYS = (1, 2, 4)` constant.
const SEND_RETRY_DELAYS: &[u64] = &[1, 2, 4];

/// Public status row returned by [`ChannelManager::get_status`].
#[derive(Debug, Clone, serde::Serialize)]
pub struct ChannelStatus {
    pub enabled: bool,
    pub running: bool,
}

/// Coordinates chat channels and routes messages between them and the
/// agent (via the [`MessageBus`]).
pub struct ChannelManager {
    config: Arc<Config>,
    bus: MessageBus,
    channels: RwLock<HashMap<String, Arc<dyn Channel>>>,
    /// Background tasks (per-channel `start()` plus the dispatcher).
    tasks: RwLock<Vec<JoinHandle<()>>>,
    dispatch_task: RwLock<Option<JoinHandle<()>>>,
}

impl ChannelManager {
    /// Build a new manager. The constructor *initialises* (but does not
    /// start) every enabled channel, mirroring `ChannelManager.__init__`.
    pub fn new(config: Arc<Config>, bus: MessageBus) -> Result<Arc<Self>, String> {
        let mgr = Arc::new(Self {
            config: config.clone(),
            bus: bus.clone(),
            channels: RwLock::new(HashMap::new()),
            tasks: RwLock::new(Vec::new()),
            dispatch_task: RwLock::new(None),
        });
        mgr.init_channels()?;
        Ok(mgr)
    }

    fn init_channels(&self) -> Result<(), String> {
        let provider = &self.config.channels.transcription_provider;
        let key = self.resolve_transcription_key(provider);
        let api_base = self.resolve_transcription_base(provider);
        let language = self.config.channels.transcription_language.clone();

        let settings = TranscriptionSettings {
            provider: provider.clone(),
            api_key: key,
            api_base,
            language,
        };

        let entries = build_enabled_channels(&self.config, &self.bus, &settings)?;

        // Publish to the manager's channel map.
        let mut guard = self
            .channels
            .try_write()
            .expect("init_channels runs before any other task");
        for ChannelEntry {
            name,
            channel,
            display_name,
        } in entries
        {
            info!("{display_name} channel enabled");
            guard.insert(name, channel);
        }

        // Validate that no enabled channel was configured with an
        // explicitly empty `allowFrom` list (which would deny all).
        for name in guard.keys() {
            if let Some(section) = self.config.channels.extras.get(name) {
                let allow = section
                    .get("allowFrom")
                    .or_else(|| section.get("allow_from"));
                if matches!(allow, Some(Value::Array(a)) if a.is_empty()) {
                    return Err(format!(
                        "Error: \"{name}\" has empty allowFrom (denies all). \
                         Set [\"*\"] to allow everyone, or add specific user IDs."
                    ));
                }
            }
        }
        Ok(())
    }

    fn resolve_transcription_key(&self, provider: &str) -> String {
        let providers = &self.config.providers;
        let key = if provider == "openai" {
            providers.openai.api_key.as_deref()
        } else {
            providers.groq.api_key.as_deref()
        };
        key.unwrap_or("").to_string()
    }

    fn resolve_transcription_base(&self, provider: &str) -> String {
        let providers = &self.config.providers;
        let base = if provider == "openai" {
            providers.openai.api_base.as_deref()
        } else {
            providers.groq.api_base.as_deref()
        };
        base.unwrap_or("").to_string()
    }

    /// Start every enabled channel and the outbound dispatcher.
    pub async fn start_all(self: Arc<Self>) {
        let channels = self.channels.read().await;
        if channels.is_empty() {
            warn!("No channels enabled");
            return;
        }

        // Start outbound dispatcher.
        let dispatcher_handle = tokio::spawn({
            let this = Arc::clone(&self);
            async move {
                this.dispatch_outbound().await;
            }
        });
        *self.dispatch_task.write().await = Some(dispatcher_handle);

        // Start individual channels.
        let mut tasks = Vec::with_capacity(channels.len());
        for (name, channel) in channels.iter() {
            info!("Starting {name} channel...");
            let channel = Arc::clone(channel);
            let name = name.clone();
            tasks.push(tokio::spawn(async move {
                if let Err(e) = channel.start().await {
                    error!("Failed to start channel {name}: {e}");
                }
            }));
        }

        drop(channels);
        *self.tasks.write().await = tasks;

        self.notify_restart_done_if_needed();
    }

    fn notify_restart_done_if_needed(&self) {
        let Some(notice) = consume_restart_notice_from_env() else {
            return;
        };
        // Skip if the target channel isn't enabled in this process.
        let exists = self
            .channels
            .try_read()
            .ok()
            .map(|g| g.contains_key(&notice.channel))
            .unwrap_or(false);
        if !exists {
            return;
        }
        let bus = self.bus.clone();
        tokio::spawn(async move {
            let msg = OutboundMessage {
                channel: notice.channel.clone(),
                chat_id: notice.chat_id.clone(),
                content: format_restart_completed_message(&notice.started_at_raw),
                ..Default::default()
            };
            bus.publish_outbound(msg).await;
        });
    }

    /// Stop every channel and the dispatcher.
    pub async fn stop_all(&self) {
        info!("Stopping all channels...");

        if let Some(handle) = self.dispatch_task.write().await.take() {
            handle.abort();
            let _ = handle.await;
        }

        let channels = self.channels.read().await;
        for (name, ch) in channels.iter() {
            match Arc::clone(ch).stop().await {
                Ok(()) => info!("Stopped {name} channel"),
                Err(e) => error!("Error stopping {name}: {e}"),
            }
        }
        drop(channels);

        let mut tasks = self.tasks.write().await;
        for h in tasks.drain(..) {
            h.abort();
        }
    }

    async fn dispatch_outbound(self: Arc<Self>) {
        info!("Outbound dispatcher started");

        // Buffer for deltas that didn't match the current coalescing
        // group — replayed before pulling more from the bus.
        let mut pending: Vec<OutboundMessage> = Vec::new();

        loop {
            let msg = if let Some(m) = pending.pop() {
                m
            } else {
                match tokio::time::timeout(Duration::from_secs(1), self.bus.consume_outbound())
                    .await
                {
                    Ok(Some(m)) => m,
                    Ok(None) => break,  // bus closed
                    Err(_) => continue, // poll-tick timeout
                }
            };

            // Filter progress / tool-hint messages per config.
            if msg
                .metadata
                .get("_progress")
                .map(is_truthy)
                .unwrap_or(false)
            {
                let is_tool_hint = msg
                    .metadata
                    .get("_tool_hint")
                    .map(is_truthy)
                    .unwrap_or(false);
                if is_tool_hint && !self.config.channels.send_tool_hints {
                    continue;
                }
                if !is_tool_hint && !self.config.channels.send_progress {
                    continue;
                }
            }
            if msg
                .metadata
                .get("_retry_wait")
                .map(is_truthy)
                .unwrap_or(false)
            {
                continue;
            }

            // Coalesce consecutive _stream_delta entries for the same
            // (channel, chat_id) target.
            let msg = if msg
                .metadata
                .get(STREAM_DELTA_KEY)
                .map(is_truthy)
                .unwrap_or(false)
                && !msg
                    .metadata
                    .get(STREAM_END_KEY)
                    .map(is_truthy)
                    .unwrap_or(false)
            {
                let (merged, extras) = self.coalesce_stream_deltas(msg).await;
                pending.extend(extras);
                merged
            } else {
                msg
            };

            let channel = self.channels.read().await.get(&msg.channel).cloned();
            match channel {
                Some(ch) => self.send_with_retry(ch, msg).await,
                None => warn!("Unknown channel: {}", msg.channel),
            }
        }
    }

    async fn coalesce_stream_deltas(
        &self,
        first: OutboundMessage,
    ) -> (OutboundMessage, Vec<OutboundMessage>) {
        let target_key = (first.channel.clone(), first.chat_id.clone());
        let mut combined = first.content.clone();
        let mut metadata = first.metadata.clone();
        let mut non_matching: Vec<OutboundMessage> = Vec::new();

        // Drain any deltas that are immediately ready (without blocking
        // the dispatcher). We can't peek at the unbounded queue, so we
        // try a 0-duration timeout to yield once to other tasks.
        loop {
            let next =
                match tokio::time::timeout(Duration::from_millis(0), self.bus.consume_outbound())
                    .await
                {
                    Ok(Some(m)) => m,
                    Ok(None) => break,
                    Err(_) => break,
                };

            let same_target = (next.channel.clone(), next.chat_id.clone()) == target_key;
            let is_delta = next
                .metadata
                .get(STREAM_DELTA_KEY)
                .map(is_truthy)
                .unwrap_or(false);
            let is_end = next
                .metadata
                .get(STREAM_END_KEY)
                .map(is_truthy)
                .unwrap_or(false);
            let already_ended = metadata.get(STREAM_END_KEY).map(is_truthy).unwrap_or(false);

            if same_target && is_delta && !already_ended {
                combined.push_str(&next.content);
                if is_end {
                    metadata.insert(STREAM_END_KEY.into(), Value::Bool(true));
                    break;
                }
            } else {
                non_matching.push(next);
                break;
            }
        }

        let merged = OutboundMessage {
            channel: first.channel,
            chat_id: first.chat_id,
            content: combined,
            metadata,
            ..first
        };
        (merged, non_matching)
    }

    async fn send_with_retry(&self, channel: Arc<dyn Channel>, msg: OutboundMessage) {
        let max_attempts = self.config.channels.send_max_retries.max(1) as usize;

        for attempt in 0..max_attempts {
            match self.send_once(Arc::clone(&channel), msg.clone()).await {
                Ok(()) => return,
                Err(e) => {
                    if attempt == max_attempts - 1 {
                        error!(
                            "Failed to send to {} after {} attempts: {}",
                            msg.channel, max_attempts, e
                        );
                        return;
                    }
                    let delay = SEND_RETRY_DELAYS[attempt.min(SEND_RETRY_DELAYS.len() - 1)];
                    warn!(
                        "Send to {} failed (attempt {}/{}): {}, retrying in {}s",
                        msg.channel,
                        attempt + 1,
                        max_attempts,
                        e,
                        delay
                    );
                    sleep(Duration::from_secs(delay)).await;
                }
            }
        }
    }

    async fn send_once(
        &self,
        channel: Arc<dyn Channel>,
        msg: OutboundMessage,
    ) -> Result<(), crate::base::ChannelError> {
        let is_delta = msg
            .metadata
            .get(STREAM_DELTA_KEY)
            .map(is_truthy)
            .unwrap_or(false);
        let is_end = msg
            .metadata
            .get(STREAM_END_KEY)
            .map(is_truthy)
            .unwrap_or(false);

        if is_delta || is_end {
            let chat_id = msg.chat_id.clone();
            let content = msg.content.clone();
            let mut meta = serde_json::Map::new();
            for (k, v) in msg.metadata.iter() {
                meta.insert(k.clone(), v.clone());
            }
            channel.send_delta(chat_id, content, meta).await
        } else if !msg
            .metadata
            .get("_streamed")
            .map(is_truthy)
            .unwrap_or(false)
        {
            channel.send(msg).await
        } else {
            Ok(())
        }
    }

    /// Lookup a channel by name.
    pub async fn get_channel(&self, name: &str) -> Option<Arc<dyn Channel>> {
        self.channels.read().await.get(name).cloned()
    }

    /// Status of every channel.
    pub async fn get_status(&self) -> HashMap<String, ChannelStatus> {
        let channels = self.channels.read().await;
        channels
            .iter()
            .map(|(name, ch)| {
                (
                    name.clone(),
                    ChannelStatus {
                        enabled: true,
                        running: ch.is_running(),
                    },
                )
            })
            .collect()
    }

    /// Names of every enabled channel.
    pub async fn enabled_channels(&self) -> Vec<String> {
        self.channels.read().await.keys().cloned().collect()
    }
}

fn is_truthy(v: &Value) -> bool {
    match v {
        Value::Bool(b) => *b,
        Value::Null => false,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(true),
        Value::String(s) => !s.is_empty() && s != "false" && s != "0",
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}
