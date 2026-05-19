//! `LLMProvider` trait — async abstraction over chat completion backends.
//!
//! Port of `nanobot.providers.base.LLMProvider`. Heavy helpers (message
//! sanitization, retry policy) are implemented here so concrete backends only
//! need to provide `chat`.

use std::time::Duration;

use async_trait::async_trait;
use log::warn;
use serde_json::Value;
use tokio::time::sleep;

use crate::retry::{is_transient_response, pick_delay};
use crate::types::{GenerationSettings, LLMResponse, ToolChoice};

/// Request passed to [`LLMProvider::chat`].
///
/// `messages` and `tools` follow OpenAI's JSON schema shape but are kept
/// as `serde_json::Value` so providers can adapt to their own wire formats.
#[derive(Debug, Clone, Default)]
pub struct ChatRequest {
    pub messages: Vec<Value>,
    pub tools: Option<Vec<Value>>,
    pub model: Option<String>,
    pub max_tokens: u32,
    pub temperature: f32,
    pub reasoning_effort: Option<String>,
    pub tool_choice: Option<ToolChoice>,
}

/// Mode for the built-in retry policy (mirrors Python `"standard" | "persistent"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryMode {
    Standard,
    Persistent,
}

impl RetryMode {
    pub fn from_str(s: &str) -> Self {
        if s == "persistent" {
            Self::Persistent
        } else {
            Self::Standard
        }
    }
}

/// Callback invoked during retry backoff (for UI/heartbeat).
pub type RetryWaitCallback =
    std::sync::Arc<dyn Fn(String) -> futures::future::BoxFuture<'static, ()> + Send + Sync>;

/// Callback invoked on each streaming content delta.
pub type StreamDeltaCallback = std::sync::Arc<dyn Fn(String) + Send + Sync>;

const PERSISTENT_MAX_DELAY: u64 = 60;
const PERSISTENT_IDENTICAL_ERROR_LIMIT: u32 = 10;
const RETRY_HEARTBEAT_CHUNK: u64 = 30;

/// Base trait every LLM backend implements.
#[async_trait]
pub trait LLMProvider: Send + Sync {
    /// Provider's default model when none is passed explicitly.
    fn default_model(&self) -> String;

    /// Generation defaults (temperature, max_tokens, ...).
    fn generation(&self) -> GenerationSettings {
        GenerationSettings::default()
    }

    /// Single chat completion call. Concrete providers implement this.
    async fn chat(&self, req: ChatRequest) -> LLMResponse;

    /// Streaming chat. Default: fallback to `chat` and emit one delta.
    async fn chat_stream(
        &self,
        req: ChatRequest,
        on_delta: Option<StreamDeltaCallback>,
    ) -> LLMResponse {
        let response = self.chat(req).await;
        if let Some(cb) = on_delta {
            if let Some(text) = response.content.clone() {
                if !text.is_empty() {
                    cb(text);
                }
            }
        }
        response
    }

    /// Wrapper with retry policy on transient errors.
    async fn chat_with_retry(
        &self,
        mut req: ChatRequest,
        mode: RetryMode,
        on_retry_wait: Option<RetryWaitCallback>,
    ) -> LLMResponse {
        let defaults = self.generation();
        if req.max_tokens == 0 {
            req.max_tokens = defaults.max_tokens;
        }
        if !req.temperature.is_finite() {
            req.temperature = defaults.temperature;
        }
        if req.reasoning_effort.is_none() {
            req.reasoning_effort = defaults.reasoning_effort.clone();
        }

        let delays: [u64; 3] = [1, 2, 4];
        let persistent = mode == RetryMode::Persistent;
        let mut attempt: u32 = 0;
        let mut last_error_key: Option<String> = None;
        let mut identical_count: u32 = 0;
        let mut last_response: Option<LLMResponse>;

        loop {
            attempt += 1;
            let response = self.chat(req.clone()).await;
            if response.finish_reason != "error" {
                return response;
            }

            let key = response
                .content
                .as_deref()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            if !key.is_empty() && Some(&key) == last_error_key.as_ref() {
                identical_count = identical_count.saturating_add(1);
            } else {
                last_error_key = if key.is_empty() { None } else { Some(key) };
                identical_count = if last_error_key.is_some() { 1 } else { 0 };
            }
            last_response = Some(response.clone());

            if !is_transient_response(&response) {
                return response;
            }

            if persistent && identical_count >= PERSISTENT_IDENTICAL_ERROR_LIMIT {
                warn!(
                    "Stopping persistent retry after {identical_count} identical transient errors"
                );
                if let Some(cb) = on_retry_wait.as_ref() {
                    (cb)(format!(
                        "Persistent retry stopped after {identical_count} identical errors."
                    ))
                    .await;
                }
                return response;
            }

            if !persistent && attempt as usize > delays.len() {
                warn!("LLM request failed after {attempt} retries, giving up");
                if let Some(cb) = on_retry_wait.as_ref() {
                    (cb)(format!(
                        "Model request failed after {attempt} retries, giving up."
                    ))
                    .await;
                }
                break;
            }

            let base_delay = delays[(attempt as usize - 1).min(delays.len() - 1)];
            let mut delay = pick_delay(&response).unwrap_or(base_delay as f64);
            if persistent {
                delay = delay.min(PERSISTENT_MAX_DELAY as f64);
            }

            warn!(
                "LLM transient error (attempt {attempt}), retrying in {}s",
                delay.round() as i64
            );
            sleep_with_heartbeat(delay, attempt, persistent, on_retry_wait.as_ref()).await;
        }

        last_response.unwrap_or_else(|| LLMResponse::error("LLM request failed"))
    }
}

async fn sleep_with_heartbeat(
    delay: f64,
    attempt: u32,
    persistent: bool,
    on_retry_wait: Option<&RetryWaitCallback>,
) {
    let mut remaining = delay.max(0.0);
    while remaining > 0.0 {
        if let Some(cb) = on_retry_wait {
            let kind = if persistent {
                "persistent retry"
            } else {
                "retry"
            };
            (cb)(format!(
                "Model request failed, {kind} in {}s (attempt {attempt}).",
                remaining.round().max(1.0) as i64
            ))
            .await;
        }
        let chunk = remaining.min(RETRY_HEARTBEAT_CHUNK as f64);
        sleep(Duration::from_secs_f64(chunk)).await;
        remaining -= chunk;
    }
}
