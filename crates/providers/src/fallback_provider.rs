//! Provider wrapper that transparently fails over to fallback models on error.
//!
//! Port of `nanobot.providers.fallback_provider`.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use log::{debug, info, warn};
use tokio::sync::Mutex;

use crate::base::{ChatRequest, LLMProvider, StreamDeltaCallback};
use crate::base::{GenerationSettings, LLMResponse};

/// Circuit breaker tuned to match OpenAICompatProvider's Responses API breaker.
const PRIMARY_FAILURE_THRESHOLD: u32 = 3;
const PRIMARY_COOLDOWN_S: f64 = 60.0;

static FALLBACK_ERROR_KINDS: &[&str] = &[
    "timeout",
    "connection",
    "server_error",
    "rate_limit",
    "overloaded",
];

static NON_FALLBACK_ERROR_KINDS: &[&str] = &[
    "authentication",
    "auth",
    "permission",
    "content_filter",
    "refusal",
    "context_length",
    "invalid_request",
];

static FALLBACK_ERROR_TOKENS: &[&str] = &[
    "rate_limit",
    "rate limit",
    "too_many_requests",
    "too many requests",
    "overloaded",
    "server_error",
    "server error",
    "temporarily unavailable",
    "timeout",
    "timed out",
    "connection",
    "insufficient_quota",
    "insufficient quota",
    "quota_exceeded",
    "quota exceeded",
    "quota_exhausted",
    "quota exhausted",
    "billing_hard_limit",
    "insufficient_balance",
    "balance",
    "out of credits",
];

/// Preset describing a fallback model configuration.
#[derive(Debug, Clone)]
pub struct FallbackPreset {
    pub model: String,
    pub max_tokens: u32,
    pub temperature: f32,
    pub reasoning_effort: Option<String>,
}

/// Factory function type that creates an LLMProvider from a preset.
pub type ProviderFactory = Arc<dyn Fn(&FallbackPreset) -> Arc<dyn LLMProvider> + Send + Sync>;

/// Internal state for the circuit breaker.
struct FallbackState {
    primary_failures: AtomicU32,
    primary_tripped_at: Mutex<Option<Instant>>,
}

/// Wrap a primary provider and transparently failover to fallback models.
///
/// When the primary model returns an error and no content has been streamed yet,
/// the wrapper tries each fallback model in order. Each fallback model may
/// reside on a different provider — a factory callable creates the underlying
/// provider on-the-fly.
pub struct FallbackProvider {
    primary: Arc<dyn LLMProvider>,
    fallback_presets: Vec<FallbackPreset>,
    provider_factory: ProviderFactory,
    has_fallbacks: bool,
    state: Arc<FallbackState>,
}

impl FallbackProvider {
    pub fn new(
        primary: Arc<dyn LLMProvider>,
        fallback_presets: Vec<FallbackPreset>,
        provider_factory: ProviderFactory,
    ) -> Self {
        let has_fallbacks = !fallback_presets.is_empty();
        Self {
            primary,
            fallback_presets,
            provider_factory,
            has_fallbacks,
            state: Arc::new(FallbackState {
                primary_failures: AtomicU32::new(0),
                primary_tripped_at: Mutex::new(None),
            }),
        }
    }

    async fn primary_available(&self) -> bool {
        let guard = self.state.primary_tripped_at.lock().await;
        match *guard {
            None => true,
            Some(tripped_at) => {
                let elapsed = tripped_at.elapsed().as_secs_f64();
                elapsed >= PRIMARY_COOLDOWN_S
            }
        }
    }

    fn should_fallback(response: &LLMResponse) -> bool {
        if response.error_should_retry == Some(false) {
            return false;
        }

        let status = response.error_status_code;
        let kind = response.error_kind.as_deref().unwrap_or("").to_lowercase();
        let error_type = response.error_type.as_deref().unwrap_or("").to_lowercase();
        let code = response.error_code.as_deref().unwrap_or("").to_lowercase();
        let text = response.content.as_deref().unwrap_or("").to_lowercase();

        if let Some(s) = status {
            if matches!(s, 400 | 401 | 403 | 404 | 422) {
                return false;
            }
        }

        if NON_FALLBACK_ERROR_KINDS.contains(&kind.as_str()) {
            return false;
        }

        let search_values: [&str; 3] = [&kind, &error_type, &code];
        if search_values
            .iter()
            .any(|v| NON_FALLBACK_ERROR_KINDS.iter().any(|t| v.contains(t)))
        {
            return false;
        }

        if response.error_should_retry == Some(true) {
            return true;
        }

        if let Some(s) = status {
            if matches!(s, 408 | 409 | 429) || (500..=599).contains(&s) {
                return true;
            }
        }

        if FALLBACK_ERROR_KINDS.contains(&kind.as_str()) {
            return true;
        }

        let search_values: [&str; 4] = [&kind, &error_type, &code, &text];
        search_values
            .iter()
            .any(|v| FALLBACK_ERROR_TOKENS.iter().any(|t| v.contains(t)))
    }

    async fn try_with_fallback_chat(&self, mut req: ChatRequest, has_streamed: Option<&AtomicBool>) -> LLMResponse {
        let primary_model = req
            .model
            .clone()
            .unwrap_or_else(|| self.primary.default_model());

        let primary_is_available = self.primary_available().await;

        if primary_is_available {
            let response = self.primary.chat(req.clone()).await;
            if response.finish_reason != "error" {
                self.state.primary_failures.store(0, Ordering::SeqCst);
                let mut tripped = self.state.primary_tripped_at.lock().await;
                *tripped = None;
                return response;
            }

            if let Some(flag) = has_streamed {
                if flag.load(Ordering::SeqCst) {
                    warn!(
                        "Primary model error but content already streamed; skipping failover"
                    );
                    return response;
                }
            }

            if !Self::should_fallback(&response) {
                warn!(
                    "Primary model '{}' returned non-fallbackable error: {}",
                    primary_model,
                    response.content.as_deref().unwrap_or("").chars().take(120).collect::<String>(),
                );
                return response;
            }

            let failures = self.state.primary_failures.fetch_add(1, Ordering::SeqCst) + 1;
            if failures >= PRIMARY_FAILURE_THRESHOLD {
                let mut tripped = self.state.primary_tripped_at.lock().await;
                *tripped = Some(Instant::now());
                warn!(
                    "Primary model '{}' circuit open after {} consecutive failures",
                    primary_model, failures,
                );
            }
        } else {
            debug!("Primary model '{}' circuit open; skipping", primary_model);
        }

        let mut last_response: Option<LLMResponse> = None;
        let primary_skipped = !self.primary_available().await;

        for (idx, fallback) in self.fallback_presets.iter().enumerate() {
            let fallback_model = &fallback.model;

            if let Some(flag) = has_streamed {
                if flag.load(Ordering::SeqCst) {
                    break;
                }
            }

            if idx == 0 && primary_skipped {
                info!(
                    "Primary model '{}' circuit open, trying fallback '{}'",
                    primary_model, fallback_model,
                );
            } else if idx == 0 {
                info!(
                    "Primary model '{}' failed, trying fallback '{}'",
                    primary_model, fallback_model,
                );
            } else {
                info!(
                    "Fallback '{}' also failed, trying next fallback '{}'",
                    self.fallback_presets[idx - 1].model, fallback_model,
                );
            }

            let provider = (self.provider_factory)(fallback);

            let original_model = req.model.clone();
            let original_max_tokens = req.max_tokens;
            let original_temperature = req.temperature;
            let original_reasoning_effort = req.reasoning_effort.clone();

            req.model = Some(fallback_model.clone());
            req.max_tokens = fallback.max_tokens;
            req.temperature = fallback.temperature;
            if fallback.reasoning_effort.is_none() {
                req.reasoning_effort = None;
            } else {
                req.reasoning_effort = fallback.reasoning_effort.clone();
            }

            let fallback_response = provider.chat(req.clone()).await;

            req.model = original_model;
            req.max_tokens = original_max_tokens;
            req.temperature = original_temperature;
            req.reasoning_effort = original_reasoning_effort;

            let fallback_content_preview = fallback_response
                .content
                .as_deref()
                .unwrap_or("")
                .chars()
                .take(120)
                .collect::<String>();

            if fallback_response.finish_reason != "error" {
                info!(
                    "Fallback '{}' succeeded after primary '{}' failed",
                    fallback_model, primary_model,
                );
                return fallback_response;
            }

            last_response = Some(fallback_response);
            warn!(
                "Fallback '{}' also failed: {}",
                fallback_model,
                fallback_content_preview,
            );
        }

        warn!(
            "All {} fallback model(s) failed",
            self.fallback_presets.len(),
        );

        if let Some(resp) = last_response {
            return resp;
        }

        LLMResponse {
            content: Some(format!(
                "Primary model '{}' circuit open and no fallbacks available",
                primary_model
            )),
            finish_reason: "error".into(),
            ..Default::default()
        }
    }

    async fn try_with_fallback_stream(
        &self,
        mut req: ChatRequest,
        tracking_delta: StreamDeltaCallback,
        has_streamed: &AtomicBool,
    ) -> LLMResponse {
        let primary_model = req
            .model
            .clone()
            .unwrap_or_else(|| self.primary.default_model());

        let primary_is_available = self.primary_available().await;

        if primary_is_available {
            let response = self.primary.chat_stream(req.clone(), Some(Arc::clone(&tracking_delta))).await;
            if response.finish_reason != "error" {
                self.state.primary_failures.store(0, Ordering::SeqCst);
                let mut tripped = self.state.primary_tripped_at.lock().await;
                *tripped = None;
                return response;
            }

            if has_streamed.load(Ordering::SeqCst) {
                warn!(
                    "Primary model error but content already streamed; skipping failover"
                );
                return response;
            }

            if !Self::should_fallback(&response) {
                warn!(
                    "Primary model '{}' returned non-fallbackable error: {}",
                    primary_model,
                    response.content.as_deref().unwrap_or("").chars().take(120).collect::<String>(),
                );
                return response;
            }

            let failures = self.state.primary_failures.fetch_add(1, Ordering::SeqCst) + 1;
            if failures >= PRIMARY_FAILURE_THRESHOLD {
                let mut tripped = self.state.primary_tripped_at.lock().await;
                *tripped = Some(Instant::now());
                warn!(
                    "Primary model '{}' circuit open after {} consecutive failures",
                    primary_model, failures,
                );
            }
        } else {
            debug!("Primary model '{}' circuit open; skipping", primary_model);
        }

        let mut last_response: Option<LLMResponse> = None;
        let primary_skipped = !self.primary_available().await;

        for (idx, fallback) in self.fallback_presets.iter().enumerate() {
            let fallback_model = &fallback.model;

            if has_streamed.load(Ordering::SeqCst) {
                break;
            }

            if idx == 0 && primary_skipped {
                info!(
                    "Primary model '{}' circuit open, trying fallback '{}'",
                    primary_model, fallback_model,
                );
            } else if idx == 0 {
                info!(
                    "Primary model '{}' failed, trying fallback '{}'",
                    primary_model, fallback_model,
                );
            } else {
                info!(
                    "Fallback '{}' also failed, trying next fallback '{}'",
                    self.fallback_presets[idx - 1].model, fallback_model,
                );
            }

            let provider = (self.provider_factory)(fallback);

            let original_model = req.model.clone();
            let original_max_tokens = req.max_tokens;
            let original_temperature = req.temperature;
            let original_reasoning_effort = req.reasoning_effort.clone();

            req.model = Some(fallback_model.clone());
            req.max_tokens = fallback.max_tokens;
            req.temperature = fallback.temperature;
            if fallback.reasoning_effort.is_none() {
                req.reasoning_effort = None;
            } else {
                req.reasoning_effort = fallback.reasoning_effort.clone();
            }

            let fallback_response = provider.chat_stream(req.clone(), Some(Arc::clone(&tracking_delta))).await;

            req.model = original_model;
            req.max_tokens = original_max_tokens;
            req.temperature = original_temperature;
            req.reasoning_effort = original_reasoning_effort;

            let fallback_content_preview = fallback_response
                .content
                .as_deref()
                .unwrap_or("")
                .chars()
                .take(120)
                .collect::<String>();

            if fallback_response.finish_reason != "error" {
                info!(
                    "Fallback '{}' succeeded after primary '{}' failed",
                    fallback_model, primary_model,
                );
                return fallback_response;
            }

            last_response = Some(fallback_response);
            warn!(
                "Fallback '{}' also failed: {}",
                fallback_model,
                fallback_content_preview,
            );
        }

        warn!(
            "All {} fallback model(s) failed",
            self.fallback_presets.len(),
        );

        if let Some(resp) = last_response {
            return resp;
        }

        LLMResponse {
            content: Some(format!(
                "Primary model '{}' circuit open and no fallbacks available",
                primary_model
            )),
            finish_reason: "error".into(),
            ..Default::default()
        }
    }
}

#[async_trait]
impl LLMProvider for FallbackProvider {
    fn default_model(&self) -> String {
        self.primary.default_model()
    }

    fn generation(&self) -> GenerationSettings {
        self.primary.generation()
    }

    async fn chat(&self, req: ChatRequest) -> LLMResponse {
        if !self.has_fallbacks {
            return self.primary.chat(req).await;
        }
        self.try_with_fallback_chat(req, None).await
    }

    async fn chat_stream(
        &self,
        req: ChatRequest,
        on_delta: Option<StreamDeltaCallback>,
    ) -> LLMResponse {
        if !self.has_fallbacks {
            return self.primary.chat_stream(req, on_delta).await;
        }

        let has_streamed = Arc::new(AtomicBool::new(false));

        let tracking_delta: StreamDeltaCallback = {
            let has_streamed = Arc::clone(&has_streamed);
            let on_delta = on_delta.clone();
            Arc::new(move |text: String| {
                if !text.is_empty() {
                    has_streamed.store(true, Ordering::SeqCst);
                }
                if let Some(ref cb) = on_delta {
                    cb(text);
                }
            })
        };

        self.try_with_fallback_stream(req, tracking_delta, &has_streamed).await
    }
}
