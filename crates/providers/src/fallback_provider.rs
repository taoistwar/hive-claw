//! Provider wrapper that transparently fails over to fallback models on error.
//!
//! Port of `nanobot.providers.fallback_provider`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use log::{debug, info, warn};
use tokio::sync::Mutex;
use tokio::time::Instant as TokioInstant;

use crate::base::{
    ChatRequest, FallbackReason, FallbackTransition, GenerationSettings, LLMProvider, LLMResponse,
    LlmCallOptions, StreamDeltaCallback, prepare_chat_request,
};
use crate::responses::ToolCallDeltaCallback;

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

/// A prebuilt fallback provider at a stable position in the configured chain.
///
/// The same model may occur more than once. Selection is therefore always by
/// vector position, never by model-name lookup.
#[derive(Clone)]
pub struct FallbackTarget {
    model: String,
    provider: Arc<dyn LLMProvider>,
}

impl FallbackTarget {
    pub fn new(model: impl Into<String>, provider: Arc<dyn LLMProvider>) -> Self {
        Self {
            model: model.into(),
            provider,
        }
    }

    pub fn model(&self) -> &str {
        &self.model
    }
}

/// Internal state for the circuit breaker.
struct FallbackState {
    primary_failures: AtomicU32,
    primary_tripped_at: Mutex<Option<Instant>>,
}

enum BoundedAttempt {
    Response(Box<LLMResponse>),
    NodeTimeout,
    ChainTimeout,
}

/// Wrap a primary provider and transparently failover to fallback models.
///
/// When the primary model returns an error and no content has been streamed yet,
/// the wrapper tries each prebuilt fallback target in configuration order.
pub struct FallbackProvider {
    primary: Arc<dyn LLMProvider>,
    fallback_targets: Vec<FallbackTarget>,
    has_fallbacks: bool,
    state: Arc<FallbackState>,
}

impl FallbackProvider {
    pub fn new(primary: Arc<dyn LLMProvider>, fallback_targets: Vec<FallbackTarget>) -> Self {
        let has_fallbacks = !fallback_targets.is_empty();
        Self {
            primary,
            fallback_targets,
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

    async fn call_provider_stream(
        provider: &Arc<dyn LLMProvider>,
        req: ChatRequest,
        on_delta: StreamDeltaCallback,
        on_tool_call_delta: Option<ToolCallDeltaCallback>,
    ) -> LLMResponse {
        if provider.supports_progress_deltas() {
            return provider
                .chat_stream(req, Some(on_delta), on_tool_call_delta)
                .await;
        }

        // The trait's default chat_stream implementation forwards even an
        // error body's text as a delta. Call non-streaming providers directly
        // so a fallbackable error is not exposed as irreversible content.
        let response = provider.chat(req).await;
        if response.finish_reason != "error"
            && let Some(content) = response
                .content
                .as_ref()
                .filter(|content| !content.is_empty())
        {
            on_delta(content.clone());
        }
        response
    }

    async fn call_provider_bounded(
        provider: &Arc<dyn LLMProvider>,
        req: ChatRequest,
        node_timeout: Duration,
        chain_deadline: TokioInstant,
    ) -> BoundedAttempt {
        let now = TokioInstant::now();
        if now >= chain_deadline {
            return BoundedAttempt::ChainTimeout;
        }

        let node_deadline = now + node_timeout;
        let chain_limited = chain_deadline <= node_deadline;
        let attempt_deadline = node_deadline.min(chain_deadline);
        match tokio::time::timeout_at(attempt_deadline, provider.chat(req)).await {
            Ok(response) => BoundedAttempt::Response(Box::new(response)),
            Err(_) if chain_limited => BoundedAttempt::ChainTimeout,
            Err(_) => BoundedAttempt::NodeTimeout,
        }
    }

    async fn call_provider_stream_bounded(
        provider: &Arc<dyn LLMProvider>,
        req: ChatRequest,
        on_delta: StreamDeltaCallback,
        on_tool_call_delta: Option<ToolCallDeltaCallback>,
        node_timeout: Duration,
        chain_deadline: TokioInstant,
    ) -> BoundedAttempt {
        let now = TokioInstant::now();
        if now >= chain_deadline {
            return BoundedAttempt::ChainTimeout;
        }

        let node_deadline = now + node_timeout;
        let chain_limited = chain_deadline <= node_deadline;
        let attempt_deadline = node_deadline.min(chain_deadline);
        match tokio::time::timeout_at(
            attempt_deadline,
            Self::call_provider_stream(provider, req, on_delta, on_tool_call_delta),
        )
        .await
        {
            Ok(response) => BoundedAttempt::Response(Box::new(response)),
            Err(_) if chain_limited => BoundedAttempt::ChainTimeout,
            Err(_) => BoundedAttempt::NodeTimeout,
        }
    }

    fn annotate_response(
        mut response: LLMResponse,
        actual_model: &str,
        fallback_used: bool,
        reason: Option<FallbackReason>,
    ) -> LLMResponse {
        response.actual_model = Some(actual_model.to_string());
        response.fallback_used = fallback_used;
        response.reason = reason;
        response
    }

    fn timeout_response(
        actual_model: &str,
        error_code: &'static str,
        fallback_used: bool,
        reason: Option<FallbackReason>,
    ) -> LLMResponse {
        LLMResponse {
            content: Some("LLM invocation timed out".to_string()),
            finish_reason: "error".to_string(),
            error_kind: Some("timeout".to_string()),
            error_code: Some(error_code.to_string()),
            error_should_retry: Some(false),
            actual_model: Some(actual_model.to_string()),
            fallback_used,
            reason,
            ..Default::default()
        }
    }

    fn circuit_open_response(primary_model: &str) -> LLMResponse {
        LLMResponse {
            content: Some(format!(
                "Primary model '{primary_model}' circuit open and no fallbacks available"
            )),
            finish_reason: "error".to_string(),
            error_kind: Some("circuit_open".to_string()),
            error_code: Some("circuit_open".to_string()),
            error_should_retry: Some(false),
            actual_model: Some(primary_model.to_string()),
            ..Default::default()
        }
    }

    fn fallback_reason(response: &LLMResponse) -> FallbackReason {
        let status = response.error_status_code;
        let kind = response.error_kind.as_deref().unwrap_or("").to_lowercase();
        let error_type = response.error_type.as_deref().unwrap_or("").to_lowercase();
        let code = response.error_code.as_deref().unwrap_or("").to_lowercase();
        let fields = [&kind, &error_type, &code];

        if status == Some(429)
            || fields.iter().any(|value| {
                [
                    "rate_limit",
                    "too_many_requests",
                    "quota",
                    "insufficient_balance",
                ]
                .iter()
                .any(|token| value.contains(token))
            })
        {
            return FallbackReason::RateLimited;
        }
        if status.is_some_and(|value| (500..=599).contains(&value))
            || fields
                .iter()
                .any(|value| value.contains("server_error") || value.contains("overloaded"))
        {
            return FallbackReason::ServerError;
        }
        if fields
            .iter()
            .any(|value| value.contains("tls") || value.contains("certificate"))
        {
            return FallbackReason::TlsError;
        }
        if status == Some(408)
            || fields.iter().any(|value| {
                value.contains("timeout")
                    || value.contains("timed_out")
                    || value.contains("timed out")
            })
        {
            return FallbackReason::ProviderTimeout;
        }
        if fields
            .iter()
            .any(|value| value.contains("connection") || value.contains("network"))
        {
            return FallbackReason::NetworkError;
        }
        FallbackReason::RetryableError
    }

    fn notify_fallback(
        options: &LlmCallOptions,
        from_provider_index: usize,
        to_provider_index: usize,
        from_model: &str,
        to_model: &str,
        reason: FallbackReason,
    ) {
        if let Some(callback) = options.on_fallback.as_ref() {
            callback(FallbackTransition {
                from_provider_index,
                to_provider_index,
                from_model: from_model.to_string(),
                to_model: to_model.to_string(),
                reason,
            });
        }
    }

    async fn reset_primary_circuit(&self) {
        self.state.primary_failures.store(0, Ordering::SeqCst);
        let mut tripped = self.state.primary_tripped_at.lock().await;
        *tripped = None;
    }

    async fn record_primary_failure(&self, primary_model: &str) {
        let failures = self.state.primary_failures.fetch_add(1, Ordering::SeqCst) + 1;
        if failures >= PRIMARY_FAILURE_THRESHOLD {
            let mut tripped = self.state.primary_tripped_at.lock().await;
            *tripped = Some(Instant::now());
            warn!(
                "Primary model '{}' circuit open after {} consecutive failures",
                primary_model, failures,
            );
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

        if status.is_some_and(|s| matches!(s, 400 | 401 | 403 | 404 | 422)) {
            return false;
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

        if status.is_some_and(|s| matches!(s, 408 | 409 | 429) || (500..=599).contains(&s)) {
            return true;
        }

        if FALLBACK_ERROR_KINDS.contains(&kind.as_str()) {
            return true;
        }

        let search_values: [&str; 4] = [&kind, &error_type, &code, &text];
        search_values
            .iter()
            .any(|v| FALLBACK_ERROR_TOKENS.iter().any(|t| v.contains(t)))
    }

    async fn try_with_options_chat(
        &self,
        req: ChatRequest,
        options: &LlmCallOptions,
        chain_deadline: TokioInstant,
    ) -> LLMResponse {
        let primary_model = req
            .model
            .clone()
            .unwrap_or_else(|| self.primary.default_model());
        let primary_is_available = self.primary_available().await;

        let mut next_reason = if primary_is_available {
            match Self::call_provider_bounded(
                &self.primary,
                req.clone(),
                options.node_timeout,
                chain_deadline,
            )
            .await
            {
                BoundedAttempt::Response(response) => {
                    let response = *response;
                    if response.finish_reason != "error" {
                        self.reset_primary_circuit().await;
                        return Self::annotate_response(response, &primary_model, false, None);
                    }
                    if !Self::should_fallback(&response) || !self.has_fallbacks {
                        return Self::annotate_response(response, &primary_model, false, None);
                    }
                    self.record_primary_failure(&primary_model).await;
                    Self::fallback_reason(&response)
                }
                BoundedAttempt::NodeTimeout => {
                    self.record_primary_failure(&primary_model).await;
                    if !self.has_fallbacks {
                        return Self::timeout_response(&primary_model, "node_timeout", false, None);
                    }
                    FallbackReason::NodeTimeout
                }
                BoundedAttempt::ChainTimeout => {
                    return Self::timeout_response(&primary_model, "chain_timeout", false, None);
                }
            }
        } else {
            if !self.has_fallbacks {
                return Self::circuit_open_response(&primary_model);
            }
            FallbackReason::CircuitOpen
        };

        let mut from_provider_index = 0;
        let mut from_model = primary_model;
        let mut from_reason = None;

        for (target_offset, target) in self.fallback_targets.iter().enumerate() {
            if TokioInstant::now() >= chain_deadline {
                return Self::timeout_response(
                    &from_model,
                    "chain_timeout",
                    from_provider_index > 0,
                    from_reason,
                );
            }

            let target_index = target_offset + 1;
            Self::notify_fallback(
                options,
                from_provider_index,
                target_index,
                &from_model,
                target.model(),
                next_reason,
            );
            let target_reason = Some(next_reason);
            let mut target_request = req.clone();
            target_request.model = Some(target.model().to_string());

            match Self::call_provider_bounded(
                &target.provider,
                target_request,
                options.node_timeout,
                chain_deadline,
            )
            .await
            {
                BoundedAttempt::Response(response) => {
                    let response = *response;
                    if response.finish_reason != "error"
                        || !Self::should_fallback(&response)
                        || target_index == self.fallback_targets.len()
                    {
                        return Self::annotate_response(
                            response,
                            target.model(),
                            true,
                            target_reason,
                        );
                    }
                    next_reason = Self::fallback_reason(&response);
                }
                BoundedAttempt::NodeTimeout => {
                    if target_index == self.fallback_targets.len() {
                        return Self::timeout_response(
                            target.model(),
                            "node_timeout",
                            true,
                            target_reason,
                        );
                    }
                    next_reason = FallbackReason::NodeTimeout;
                }
                BoundedAttempt::ChainTimeout => {
                    return Self::timeout_response(
                        target.model(),
                        "chain_timeout",
                        true,
                        target_reason,
                    );
                }
            }

            from_provider_index = target_index;
            from_model = target.model().to_string();
            from_reason = target_reason;
        }

        unreachable!("fallback chain is non-empty before traversal")
    }

    async fn try_with_options_stream(
        &self,
        req: ChatRequest,
        tracking_delta: StreamDeltaCallback,
        tracking_tool_call_delta: ToolCallDeltaCallback,
        has_streamed: &AtomicBool,
        options: &LlmCallOptions,
        chain_deadline: TokioInstant,
    ) -> LLMResponse {
        let primary_model = req
            .model
            .clone()
            .unwrap_or_else(|| self.primary.default_model());
        let primary_is_available = self.primary_available().await;

        let mut next_reason = if primary_is_available {
            match Self::call_provider_stream_bounded(
                &self.primary,
                req.clone(),
                Arc::clone(&tracking_delta),
                Some(tracking_tool_call_delta.clone()),
                options.node_timeout,
                chain_deadline,
            )
            .await
            {
                BoundedAttempt::Response(response) => {
                    let response = *response;
                    if response.finish_reason != "error" {
                        self.reset_primary_circuit().await;
                        return Self::annotate_response(response, &primary_model, false, None);
                    }
                    if has_streamed.load(Ordering::SeqCst)
                        || !Self::should_fallback(&response)
                        || !self.has_fallbacks
                    {
                        return Self::annotate_response(response, &primary_model, false, None);
                    }
                    self.record_primary_failure(&primary_model).await;
                    Self::fallback_reason(&response)
                }
                BoundedAttempt::NodeTimeout => {
                    if has_streamed.load(Ordering::SeqCst) {
                        return Self::timeout_response(&primary_model, "node_timeout", false, None);
                    }
                    self.record_primary_failure(&primary_model).await;
                    if !self.has_fallbacks {
                        return Self::timeout_response(&primary_model, "node_timeout", false, None);
                    }
                    FallbackReason::NodeTimeout
                }
                BoundedAttempt::ChainTimeout => {
                    return Self::timeout_response(&primary_model, "chain_timeout", false, None);
                }
            }
        } else {
            if !self.has_fallbacks {
                return Self::circuit_open_response(&primary_model);
            }
            FallbackReason::CircuitOpen
        };

        let mut from_provider_index = 0;
        let mut from_model = primary_model;
        let mut from_reason = None;

        for (target_offset, target) in self.fallback_targets.iter().enumerate() {
            if TokioInstant::now() >= chain_deadline {
                return Self::timeout_response(
                    &from_model,
                    "chain_timeout",
                    from_provider_index > 0,
                    from_reason,
                );
            }

            let target_index = target_offset + 1;
            Self::notify_fallback(
                options,
                from_provider_index,
                target_index,
                &from_model,
                target.model(),
                next_reason,
            );
            let target_reason = Some(next_reason);
            let mut target_request = req.clone();
            target_request.model = Some(target.model().to_string());

            match Self::call_provider_stream_bounded(
                &target.provider,
                target_request,
                Arc::clone(&tracking_delta),
                Some(tracking_tool_call_delta.clone()),
                options.node_timeout,
                chain_deadline,
            )
            .await
            {
                BoundedAttempt::Response(response) => {
                    let response = *response;
                    if response.finish_reason != "error"
                        || has_streamed.load(Ordering::SeqCst)
                        || !Self::should_fallback(&response)
                        || target_index == self.fallback_targets.len()
                    {
                        return Self::annotate_response(
                            response,
                            target.model(),
                            true,
                            target_reason,
                        );
                    }
                    next_reason = Self::fallback_reason(&response);
                }
                BoundedAttempt::NodeTimeout => {
                    if has_streamed.load(Ordering::SeqCst)
                        || target_index == self.fallback_targets.len()
                    {
                        return Self::timeout_response(
                            target.model(),
                            "node_timeout",
                            true,
                            target_reason,
                        );
                    }
                    next_reason = FallbackReason::NodeTimeout;
                }
                BoundedAttempt::ChainTimeout => {
                    return Self::timeout_response(
                        target.model(),
                        "chain_timeout",
                        true,
                        target_reason,
                    );
                }
            }

            from_provider_index = target_index;
            from_model = target.model().to_string();
            from_reason = target_reason;
        }

        unreachable!("fallback chain is non-empty before traversal")
    }

    async fn try_with_fallback_chat(
        &self,
        req: ChatRequest,
        has_streamed: Option<&AtomicBool>,
    ) -> LLMResponse {
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

            if has_streamed.is_some_and(|flag| flag.load(Ordering::SeqCst)) {
                warn!("Primary model error but content already streamed; skipping failover");
                return response;
            }

            if !Self::should_fallback(&response) {
                warn!(
                    "Primary model '{}' returned non-fallbackable error: {}",
                    primary_model,
                    response
                        .content
                        .as_deref()
                        .unwrap_or("")
                        .chars()
                        .take(120)
                        .collect::<String>(),
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

        for (idx, target) in self.fallback_targets.iter().enumerate() {
            let fallback_model = target.model();

            if has_streamed.is_some_and(|flag| flag.load(Ordering::SeqCst)) {
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
                    self.fallback_targets[idx - 1].model(),
                    fallback_model,
                );
            }

            let mut fallback_request = req.clone();
            fallback_request.model = Some(fallback_model.to_string());
            let fallback_response = target.provider.chat(fallback_request).await;

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
                fallback_model, fallback_content_preview,
            );
        }

        warn!(
            "All {} fallback model(s) failed",
            self.fallback_targets.len(),
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
        req: ChatRequest,
        tracking_delta: StreamDeltaCallback,
        tracking_tool_call_delta: Option<ToolCallDeltaCallback>,
        has_streamed: &AtomicBool,
    ) -> LLMResponse {
        let primary_model = req
            .model
            .clone()
            .unwrap_or_else(|| self.primary.default_model());

        let primary_is_available = self.primary_available().await;

        if primary_is_available {
            let response = Self::call_provider_stream(
                &self.primary,
                req.clone(),
                Arc::clone(&tracking_delta),
                tracking_tool_call_delta.clone(),
            )
            .await;
            if response.finish_reason != "error" {
                self.state.primary_failures.store(0, Ordering::SeqCst);
                let mut tripped = self.state.primary_tripped_at.lock().await;
                *tripped = None;
                return response;
            }

            if has_streamed.load(Ordering::SeqCst) {
                warn!("Primary model error but content already streamed; skipping failover");
                return response;
            }

            if !Self::should_fallback(&response) {
                warn!(
                    "Primary model '{}' returned non-fallbackable error: {}",
                    primary_model,
                    response
                        .content
                        .as_deref()
                        .unwrap_or("")
                        .chars()
                        .take(120)
                        .collect::<String>(),
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

        for (idx, target) in self.fallback_targets.iter().enumerate() {
            let fallback_model = target.model();

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
                    self.fallback_targets[idx - 1].model(),
                    fallback_model,
                );
            }

            let mut fallback_request = req.clone();
            fallback_request.model = Some(fallback_model.to_string());
            let fallback_response = Self::call_provider_stream(
                &target.provider,
                fallback_request,
                Arc::clone(&tracking_delta),
                tracking_tool_call_delta.clone(),
            )
            .await;

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
                fallback_model, fallback_content_preview,
            );
        }

        warn!(
            "All {} fallback model(s) failed",
            self.fallback_targets.len(),
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

    fn supports_progress_deltas(&self) -> bool {
        self.primary.supports_progress_deltas()
    }

    async fn chat(&self, req: ChatRequest) -> LLMResponse {
        if !self.has_fallbacks {
            return self.primary.chat(req).await;
        }
        self.try_with_fallback_chat(req, None).await
    }

    async fn chat_with_options(&self, req: ChatRequest, options: LlmCallOptions) -> LLMResponse {
        let chain_deadline = TokioInstant::now() + options.chain_timeout;
        let req = prepare_chat_request(req, &self.generation());
        self.try_with_options_chat(req, &options, chain_deadline)
            .await
    }

    async fn chat_stream(
        &self,
        req: ChatRequest,
        on_delta: Option<StreamDeltaCallback>,
        on_tool_call_delta: Option<ToolCallDeltaCallback>,
    ) -> LLMResponse {
        if !self.has_fallbacks {
            return self
                .primary
                .chat_stream(req, on_delta, on_tool_call_delta)
                .await;
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

        let tracking_tool_call_delta: ToolCallDeltaCallback = {
            let has_streamed = Arc::clone(&has_streamed);
            let on_tool_call_delta = on_tool_call_delta.clone();
            Arc::new(move |delta: serde_json::Map<String, serde_json::Value>| {
                has_streamed.store(true, Ordering::SeqCst);
                if let Some(ref cb) = on_tool_call_delta {
                    cb(delta);
                }
            })
        };

        self.try_with_fallback_stream(
            req,
            tracking_delta,
            Some(tracking_tool_call_delta),
            &has_streamed,
        )
        .await
    }

    async fn chat_stream_with_options(
        &self,
        req: ChatRequest,
        on_delta: Option<StreamDeltaCallback>,
        on_tool_call_delta: Option<ToolCallDeltaCallback>,
        options: LlmCallOptions,
    ) -> LLMResponse {
        let chain_deadline = TokioInstant::now() + options.chain_timeout;
        let req = prepare_chat_request(req, &self.generation());
        let has_streamed = Arc::new(AtomicBool::new(false));

        let tracking_delta: StreamDeltaCallback = {
            let has_streamed = Arc::clone(&has_streamed);
            let on_delta = on_delta.clone();
            Arc::new(move |text: String| {
                if !text.is_empty() {
                    has_streamed.store(true, Ordering::SeqCst);
                }
                if let Some(ref callback) = on_delta {
                    callback(text);
                }
            })
        };

        let tracking_tool_call_delta: ToolCallDeltaCallback = {
            let has_streamed = Arc::clone(&has_streamed);
            let on_tool_call_delta = on_tool_call_delta.clone();
            Arc::new(move |delta: serde_json::Map<String, serde_json::Value>| {
                has_streamed.store(true, Ordering::SeqCst);
                if let Some(ref callback) = on_tool_call_delta {
                    callback(delta);
                }
            })
        };

        self.try_with_options_stream(
            req,
            tracking_delta,
            tracking_tool_call_delta,
            &has_streamed,
            &options,
            chain_deadline,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::{FallbackProvider, FallbackTarget};
    use crate::base::{
        ChatRequest, FallbackReason, FallbackTransition, LLMProvider, LLMResponse, LlmCallOptions,
        StreamDeltaCallback, ToolCallDeltaCallback,
    };
    use async_trait::async_trait;
    use serde_json::Map;
    use std::future::pending;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    #[derive(Clone)]
    enum Behavior {
        Respond(LLMResponse),
        Pending,
        PartialText(String, LLMResponse),
        PartialTool(LLMResponse),
    }

    struct FakeProvider {
        model: String,
        behavior: Behavior,
        calls: Arc<AtomicUsize>,
        supports_streaming: bool,
    }

    struct CapturingProvider {
        requests: Arc<Mutex<Vec<ChatRequest>>>,
    }

    #[async_trait]
    impl LLMProvider for CapturingProvider {
        fn default_model(&self) -> String {
            "capturing".to_string()
        }

        async fn chat(&self, req: ChatRequest) -> LLMResponse {
            self.requests.lock().expect("request lock").push(req);
            success("ok")
        }
    }

    impl FakeProvider {
        fn with_behavior(
            model: &str,
            behavior: Behavior,
            supports_streaming: bool,
        ) -> (Arc<dyn LLMProvider>, Arc<AtomicUsize>) {
            let calls = Arc::new(AtomicUsize::new(0));
            (
                Arc::new(Self {
                    model: model.to_string(),
                    behavior,
                    calls: Arc::clone(&calls),
                    supports_streaming,
                }),
                calls,
            )
        }

        async fn respond(&self) -> LLMResponse {
            match &self.behavior {
                Behavior::Respond(response)
                | Behavior::PartialText(_, response)
                | Behavior::PartialTool(response) => response.clone(),
                Behavior::Pending => pending().await,
            }
        }
    }

    #[async_trait]
    impl LLMProvider for FakeProvider {
        fn default_model(&self) -> String {
            self.model.clone()
        }

        fn supports_progress_deltas(&self) -> bool {
            self.supports_streaming
        }

        async fn chat(&self, _req: ChatRequest) -> LLMResponse {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.respond().await
        }

        async fn chat_stream(
            &self,
            _req: ChatRequest,
            on_delta: Option<StreamDeltaCallback>,
            on_tool_call_delta: Option<ToolCallDeltaCallback>,
        ) -> LLMResponse {
            self.calls.fetch_add(1, Ordering::SeqCst);
            match &self.behavior {
                Behavior::PartialText(text, response) => {
                    if let Some(callback) = on_delta {
                        callback(text.clone());
                    }
                    response.clone()
                }
                Behavior::PartialTool(response) => {
                    if let Some(callback) = on_tool_call_delta {
                        callback(Map::new());
                    }
                    response.clone()
                }
                Behavior::Respond(response) => {
                    if response.finish_reason != "error"
                        && let Some((callback, content)) = on_delta.zip(response.content.clone())
                    {
                        callback(content);
                    }
                    response.clone()
                }
                Behavior::Pending => pending().await,
            }
        }
    }

    fn success(content: &str) -> LLMResponse {
        LLMResponse {
            content: Some(content.to_string()),
            finish_reason: "stop".to_string(),
            ..Default::default()
        }
    }

    fn server_error() -> LLMResponse {
        LLMResponse {
            content: Some("provider unavailable".to_string()),
            finish_reason: "error".to_string(),
            error_status_code: Some(503),
            error_type: Some("server_error".to_string()),
            ..Default::default()
        }
    }

    fn request(model: &str) -> ChatRequest {
        ChatRequest {
            model: Some(model.to_string()),
            messages: vec![serde_json::json!({"role": "user", "content": "hello"})],
            max_tokens: 17,
            temperature: 0.25,
            ..Default::default()
        }
    }

    fn observed_options(transitions: &Arc<Mutex<Vec<FallbackTransition>>>) -> LlmCallOptions {
        observed_options_with_timeouts(
            transitions,
            Duration::from_secs(25),
            Duration::from_secs(45),
        )
    }

    fn observed_options_with_timeouts(
        transitions: &Arc<Mutex<Vec<FallbackTransition>>>,
        node_timeout: Duration,
        chain_timeout: Duration,
    ) -> LlmCallOptions {
        let transitions = Arc::clone(transitions);
        LlmCallOptions::with_timeouts(node_timeout, chain_timeout).with_fallback_callback(Arc::new(
            move |transition| {
                transitions
                    .lock()
                    .expect("transition lock")
                    .push(transition);
            },
        ))
    }

    #[test]
    fn default_and_plugin_call_budgets_are_explicit() {
        let defaults = LlmCallOptions::default();
        assert_eq!(defaults.node_timeout, Duration::from_secs(25));
        assert_eq!(defaults.chain_timeout, Duration::from_secs(45));

        let plugin =
            LlmCallOptions::with_timeouts(Duration::from_secs(25), Duration::from_secs(25));
        assert_eq!(plugin.node_timeout, Duration::from_secs(25));
        assert_eq!(plugin.chain_timeout, Duration::from_secs(25));
    }

    #[tokio::test]
    async fn options_api_preserves_role_alternation_preprocessing() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider = Arc::new(CapturingProvider {
            requests: Arc::clone(&requests),
        });
        let req = ChatRequest {
            messages: vec![
                serde_json::json!({"role": "user", "content": "one"}),
                serde_json::json!({"role": "user", "content": "two"}),
            ],
            ..Default::default()
        };

        provider
            .chat_with_options(req.clone(), LlmCallOptions::default())
            .await;
        let chain = FallbackProvider::new(provider, Vec::new());
        chain
            .chat_with_options(req, LlmCallOptions::default())
            .await;

        let requests = requests.lock().expect("request lock");
        assert_eq!(requests.len(), 2);
        for request in requests.iter() {
            assert_eq!(request.messages.len(), 1);
            assert_eq!(request.messages[0]["content"].as_str(), Some("one\n\ntwo"));
        }
    }

    #[tokio::test]
    async fn primary_success_reports_structured_metadata_without_fallback() {
        let (primary, primary_calls) =
            FakeProvider::with_behavior("primary", Behavior::Respond(success("ok")), false);
        let chain = FallbackProvider::new(primary, Vec::new());

        let response = chain
            .chat_with_options(request("primary"), LlmCallOptions::default())
            .await;

        assert_eq!(response.content.as_deref(), Some("ok"));
        assert_eq!(response.actual_model.as_deref(), Some("primary"));
        assert!(!response.fallback_used);
        assert_eq!(response.reason, None);
        assert_eq!(primary_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn non_stream_same_model_targets_use_ordinals_and_report_final_metadata() {
        let (primary, primary_calls) =
            FakeProvider::with_behavior("same-model", Behavior::Respond(server_error()), false);
        let (first, first_calls) =
            FakeProvider::with_behavior("same-model", Behavior::Respond(server_error()), false);
        let (second, second_calls) = FakeProvider::with_behavior(
            "same-model",
            Behavior::Respond(success("fallback ok")),
            false,
        );
        let chain = FallbackProvider::new(
            primary,
            vec![
                FallbackTarget::new("same-model", first),
                FallbackTarget::new("same-model", second),
            ],
        );
        let transitions = Arc::new(Mutex::new(Vec::new()));

        let response = chain
            .chat_with_options(request("same-model"), observed_options(&transitions))
            .await;

        assert_eq!(response.content.as_deref(), Some("fallback ok"));
        assert_eq!(response.actual_model.as_deref(), Some("same-model"));
        assert!(response.fallback_used);
        assert_eq!(response.reason, Some(FallbackReason::ServerError));
        assert_eq!(primary_calls.load(Ordering::SeqCst), 1);
        assert_eq!(first_calls.load(Ordering::SeqCst), 1);
        assert_eq!(second_calls.load(Ordering::SeqCst), 1);
        let transitions = transitions.lock().expect("transition lock");
        assert_eq!(transitions.len(), 2);
        assert_eq!(transitions[0].from_provider_index, 0);
        assert_eq!(transitions[0].to_provider_index, 1);
        assert_eq!(transitions[1].from_provider_index, 1);
        assert_eq!(transitions[1].to_provider_index, 2);
        assert_eq!(transitions[0].reason, FallbackReason::ServerError);
        assert_eq!(transitions[1].reason, FallbackReason::ServerError);
    }

    #[tokio::test]
    async fn stream_same_model_targets_report_metadata_and_only_final_content() {
        let (primary, _) =
            FakeProvider::with_behavior("same-model", Behavior::Respond(server_error()), true);
        let (first, _) =
            FakeProvider::with_behavior("same-model", Behavior::Respond(server_error()), true);
        let (second, _) = FakeProvider::with_behavior(
            "same-model",
            Behavior::Respond(success("stream ok")),
            true,
        );
        let chain = FallbackProvider::new(
            primary,
            vec![
                FallbackTarget::new("same-model", first),
                FallbackTarget::new("same-model", second),
            ],
        );
        let transitions = Arc::new(Mutex::new(Vec::new()));
        let deltas = Arc::new(Mutex::new(Vec::new()));
        let on_delta: StreamDeltaCallback = {
            let deltas = Arc::clone(&deltas);
            Arc::new(move |delta| deltas.lock().expect("delta lock").push(delta))
        };

        let response = chain
            .chat_stream_with_options(
                request("same-model"),
                Some(on_delta),
                None,
                observed_options(&transitions),
            )
            .await;

        assert_eq!(response.actual_model.as_deref(), Some("same-model"));
        assert!(response.fallback_used);
        assert_eq!(response.reason, Some(FallbackReason::ServerError));
        assert_eq!(deltas.lock().expect("delta lock").as_slice(), ["stream ok"]);
        assert_eq!(transitions.lock().expect("transition lock").len(), 2);
    }

    #[tokio::test]
    async fn transition_observer_fires_before_a_cancellable_fallback_call() {
        let (primary, _) =
            FakeProvider::with_behavior("primary", Behavior::Respond(server_error()), false);
        let (fallback, fallback_calls) =
            FakeProvider::with_behavior("fallback", Behavior::Pending, false);
        let chain = Arc::new(FallbackProvider::new(
            primary,
            vec![FallbackTarget::new("fallback", fallback)],
        ));
        let transitions = Arc::new(Mutex::new(Vec::new()));
        let options = observed_options(&transitions);

        let task = {
            let chain = Arc::clone(&chain);
            tokio::spawn(async move { chain.chat_with_options(request("primary"), options).await })
        };
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if fallback_calls.load(Ordering::SeqCst) == 1 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("fallback call should start");

        assert_eq!(fallback_calls.load(Ordering::SeqCst), 1);
        assert_eq!(transitions.lock().expect("transition lock").len(), 1);
        task.abort();
        let _ = task.await;
        assert_eq!(transitions.lock().expect("transition lock").len(), 1);
    }

    #[tokio::test]
    async fn node_timeout_switches_to_the_next_provider_with_typed_reason() {
        let (primary, primary_calls) =
            FakeProvider::with_behavior("primary", Behavior::Pending, false);
        let (fallback, fallback_calls) =
            FakeProvider::with_behavior("fallback", Behavior::Respond(success("ok")), false);
        let chain = FallbackProvider::new(primary, vec![FallbackTarget::new("fallback", fallback)]);
        let transitions = Arc::new(Mutex::new(Vec::new()));

        let response = chain
            .chat_with_options(
                request("primary"),
                observed_options_with_timeouts(
                    &transitions,
                    Duration::from_millis(20),
                    Duration::from_millis(200),
                ),
            )
            .await;

        assert_eq!(response.content.as_deref(), Some("ok"));
        assert_eq!(response.actual_model.as_deref(), Some("fallback"));
        assert!(response.fallback_used);
        assert_eq!(response.reason, Some(FallbackReason::NodeTimeout));
        assert_eq!(primary_calls.load(Ordering::SeqCst), 1);
        assert_eq!(fallback_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            transitions.lock().expect("transition lock")[0].reason,
            FallbackReason::NodeTimeout
        );
    }

    #[tokio::test]
    async fn chain_deadline_returns_typed_timeout_and_never_starts_another_provider() {
        let (primary, _) =
            FakeProvider::with_behavior("primary", Behavior::Respond(server_error()), false);
        let (first, first_calls) = FakeProvider::with_behavior("first", Behavior::Pending, false);
        let (second, second_calls) = FakeProvider::with_behavior(
            "second",
            Behavior::Respond(success("must not run")),
            false,
        );
        let chain = FallbackProvider::new(
            primary,
            vec![
                FallbackTarget::new("first", first),
                FallbackTarget::new("second", second),
            ],
        );
        let transitions = Arc::new(Mutex::new(Vec::new()));

        let response = chain
            .chat_with_options(
                request("primary"),
                observed_options_with_timeouts(
                    &transitions,
                    Duration::from_millis(200),
                    Duration::from_millis(40),
                ),
            )
            .await;

        assert!(response.is_error());
        assert_eq!(response.error_kind.as_deref(), Some("timeout"));
        assert_eq!(response.error_code.as_deref(), Some("chain_timeout"));
        assert_eq!(response.actual_model.as_deref(), Some("first"));
        assert!(response.fallback_used);
        assert_eq!(first_calls.load(Ordering::SeqCst), 1);
        assert_eq!(second_calls.load(Ordering::SeqCst), 0);
        assert_eq!(transitions.lock().expect("transition lock").len(), 1);
    }

    #[tokio::test]
    async fn partial_text_stream_never_falls_back() {
        let (primary, _) = FakeProvider::with_behavior(
            "primary",
            Behavior::PartialText("partial".to_string(), server_error()),
            true,
        );
        let (fallback, fallback_calls) =
            FakeProvider::with_behavior("fallback", Behavior::Respond(success("wrong")), true);
        let chain = FallbackProvider::new(primary, vec![FallbackTarget::new("fallback", fallback)]);
        let deltas = Arc::new(Mutex::new(Vec::new()));
        let callback: StreamDeltaCallback = {
            let deltas = Arc::clone(&deltas);
            Arc::new(move |delta| deltas.lock().expect("delta lock").push(delta))
        };

        let response = chain
            .chat_stream_with_options(
                request("primary"),
                Some(callback),
                None,
                LlmCallOptions::default(),
            )
            .await;

        assert!(response.is_error());
        assert_eq!(response.actual_model.as_deref(), Some("primary"));
        assert!(!response.fallback_used);
        assert_eq!(fallback_calls.load(Ordering::SeqCst), 0);
        assert_eq!(deltas.lock().expect("delta lock").as_slice(), ["partial"]);
    }

    #[tokio::test]
    async fn partial_tool_delta_stream_never_falls_back() {
        let (primary, _) =
            FakeProvider::with_behavior("primary", Behavior::PartialTool(server_error()), true);
        let (fallback, fallback_calls) =
            FakeProvider::with_behavior("fallback", Behavior::Respond(success("wrong")), true);
        let chain = FallbackProvider::new(primary, vec![FallbackTarget::new("fallback", fallback)]);
        let tool_delta_count = Arc::new(AtomicUsize::new(0));
        let callback: ToolCallDeltaCallback = {
            let tool_delta_count = Arc::clone(&tool_delta_count);
            Arc::new(move |_| {
                tool_delta_count.fetch_add(1, Ordering::SeqCst);
            })
        };

        let response = chain
            .chat_stream_with_options(
                request("primary"),
                None,
                Some(callback),
                LlmCallOptions::default(),
            )
            .await;

        assert!(response.is_error());
        assert_eq!(response.actual_model.as_deref(), Some("primary"));
        assert!(!response.fallback_used);
        assert_eq!(fallback_calls.load(Ordering::SeqCst), 0);
        assert_eq!(tool_delta_count.load(Ordering::SeqCst), 1);
    }
}
