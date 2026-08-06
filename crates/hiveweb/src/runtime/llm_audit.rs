//! Typed, exactly-once audit helpers for HiveWeb LLM calls.
//!
//! Call sites create one [`LlmAuditGuard`], attach [`LlmAuditGuard::on_fallback`]
//! to the provider options, and finish the guard with the final structured
//! response. Dropping an unfinished guard records a static cancellation error.
//! Raw provider errors are deliberately absent from this API.

use crate::runtime::RuntimeExecutionContext;
use crate::services::runtime_audit::{self, AuditRecord};
use providers::{FallbackReason, FallbackTransition, LLMResponse};
use serde_json::{Map, Value, json};
use std::sync::Arc;
use std::time::Instant;

trait LlmAuditSink: Send + Sync {
    fn record(&self, execution_context: &RuntimeExecutionContext, record: AuditRecord<'_>);
}

struct RuntimeAuditSink;

impl LlmAuditSink for RuntimeAuditSink {
    fn record(&self, execution_context: &RuntimeExecutionContext, record: AuditRecord<'_>) {
        runtime_audit::record(execution_context, record);
    }
}

/// Fixed runtime surface that initiated an LLM call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmAuditSource {
    Orchestrator,
    LlmInvoke,
    GenerateAnswer,
    ToolTest { tool_id: i64 },
    SkillTest { skill_id: i64 },
    BuiltinGameInfo,
}

impl LlmAuditSource {
    fn name(self) -> &'static str {
        match self {
            Self::Orchestrator => "orchestrator",
            Self::LlmInvoke => "llm_invoke",
            Self::GenerateAnswer => "generate_answer",
            Self::ToolTest { .. } => "tool_test",
            Self::SkillTest { .. } => "skill_test",
            Self::BuiltinGameInfo => "builtin_game_info",
        }
    }

    fn add_to_summary(self, summary: &mut Map<String, Value>) {
        summary.insert("source".into(), Value::String(self.name().into()));
        match self {
            Self::ToolTest { tool_id } => {
                summary.insert("tool_id".into(), json!(tool_id));
            }
            Self::SkillTest { skill_id } => {
                summary.insert("skill_id".into(), json!(skill_id));
            }
            _ => {}
        }
    }
}

/// Application-level fallback reasons. These are distinct from provider
/// transitions and therefore can never construct an `llm_fallback` event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmLocalFallbackReason {
    ProviderUnavailable,
    InvocationFailed,
    EmptyResponse,
}

impl LlmLocalFallbackReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::ProviderUnavailable => "provider_unavailable",
            Self::InvocationFailed => "invocation_failed",
            Self::EmptyResponse => "empty_response",
        }
    }
}

/// Exactly-once terminal audit guard for one LLM call.
pub struct LlmAuditGuard {
    execution_context: RuntimeExecutionContext,
    agent_id: Option<i64>,
    model_preset: Option<String>,
    source: LlmAuditSource,
    started_at: Instant,
    finished: bool,
    sink: Arc<dyn LlmAuditSink>,
}

impl LlmAuditGuard {
    pub fn new(
        execution_context: RuntimeExecutionContext,
        agent_id: Option<i64>,
        model_preset: Option<&str>,
        source: LlmAuditSource,
    ) -> Self {
        Self::new_with_sink(
            execution_context,
            agent_id,
            model_preset,
            source,
            Arc::new(RuntimeAuditSink),
        )
    }

    fn new_with_sink(
        execution_context: RuntimeExecutionContext,
        agent_id: Option<i64>,
        model_preset: Option<&str>,
        source: LlmAuditSource,
        sink: Arc<dyn LlmAuditSink>,
    ) -> Self {
        Self {
            execution_context,
            agent_id,
            model_preset: model_preset.map(str::to_string),
            source,
            started_at: Instant::now(),
            finished: false,
            sink,
        }
    }

    #[cfg(test)]
    fn with_sink(
        execution_context: RuntimeExecutionContext,
        agent_id: Option<i64>,
        model_preset: Option<&str>,
        source: LlmAuditSource,
        sink: Arc<dyn LlmAuditSink>,
    ) -> Self {
        Self::new_with_sink(execution_context, agent_id, model_preset, source, sink)
    }

    /// Synchronous provider-transition callback suitable for
    /// `LlmCallOptions::with_fallback_callback`.
    pub fn on_fallback(&self) -> Arc<dyn Fn(FallbackTransition) + Send + Sync + 'static> {
        let execution_context = self.execution_context.clone();
        let agent_id = self.agent_id;
        let model_preset = self.model_preset.clone();
        let started_at = self.started_at;
        let sink = Arc::clone(&self.sink);
        Arc::new(move |transition| {
            let mut summary = Map::new();
            if let Some(model_preset) = &model_preset {
                summary.insert("model_preset".into(), Value::String(model_preset.clone()));
            }
            summary.insert(
                "from_provider_ordinal".into(),
                json!(transition.from_provider_index),
            );
            summary.insert(
                "to_provider_ordinal".into(),
                json!(transition.to_provider_index),
            );
            summary.insert("from_model".into(), Value::String(transition.from_model));
            summary.insert("to_model".into(), Value::String(transition.to_model));
            summary.insert(
                "reason".into(),
                Value::String(transition.reason.as_str().into()),
            );
            sink.record(
                &execution_context,
                AuditRecord {
                    agent_id,
                    plugin_id: None,
                    function_id: None,
                    capability: None,
                    event_type: "llm_fallback",
                    outcome: "success",
                    elapsed_ms: elapsed_ms(started_at),
                    error_message: None,
                    payload_summary: Some(Value::Object(summary)),
                },
            );
        })
    }

    /// Finish based on the provider's structured response metadata.
    pub fn finish_response(&mut self, response: &LLMResponse) -> bool {
        if !response.is_error() {
            return self.finish_success(response);
        }
        if response.error_kind.as_deref() == Some("timeout")
            || matches!(
                response.error_code.as_deref(),
                Some("node_timeout" | "chain_timeout")
            )
        {
            self.finish_timeout_with_response(Some(response))
        } else {
            self.finish_error(response)
        }
    }

    pub fn finish_success(&mut self, response: &LLMResponse) -> bool {
        self.finish_once("success", None, Some(response), None)
    }

    pub fn finish_error(&mut self, response: &LLMResponse) -> bool {
        let reason = response
            .reason
            .map(FallbackReason::as_str)
            .unwrap_or("provider_error");
        self.finish_once(
            "error",
            Some("LLM invocation failed"),
            Some(response),
            Some(reason),
        )
    }

    pub fn finish_timeout(&mut self) -> bool {
        self.finish_timeout_with_response(None)
    }

    fn finish_timeout_with_response(&mut self, response: Option<&LLMResponse>) -> bool {
        let reason = match response.and_then(|response| response.error_code.as_deref()) {
            Some("node_timeout") => "node_timeout",
            Some("chain_timeout") => "chain_timeout",
            _ if response.is_some() => "provider_timeout",
            _ => "chain_timeout",
        };
        self.finish_once(
            "timeout",
            Some("LLM invocation timed out"),
            response,
            Some(reason),
        )
    }

    /// Finish a registry-resolution failure without misclassifying it as a
    /// cancellation in `Drop`.
    pub fn finish_model_preset_unknown(&mut self) -> bool {
        self.finish_once(
            "error",
            Some("LLM model preset is unavailable"),
            None,
            Some("model_preset_unknown"),
        )
    }

    fn finish_once(
        &mut self,
        outcome: &'static str,
        error_message: Option<&'static str>,
        response: Option<&LLMResponse>,
        default_reason: Option<&'static str>,
    ) -> bool {
        if self.finished {
            return false;
        }
        self.finished = true;

        let actual_model = response.and_then(|response| response.actual_model.as_deref());
        let fallback_used = response.is_some_and(|response| response.fallback_used);
        let reason = default_reason.or_else(|| {
            response
                .and_then(|response| response.reason.as_ref())
                .copied()
                .map(FallbackReason::as_str)
        });
        let summary = terminal_summary(
            self.model_preset.as_deref(),
            actual_model,
            fallback_used,
            reason,
            self.source,
        );
        self.sink.record(
            &self.execution_context,
            AuditRecord {
                agent_id: self.agent_id,
                plugin_id: None,
                function_id: None,
                capability: None,
                event_type: "llm_invoke",
                outcome,
                elapsed_ms: elapsed_ms(self.started_at),
                error_message,
                payload_summary: Some(summary),
            },
        );
        true
    }
}

impl Drop for LlmAuditGuard {
    fn drop(&mut self) {
        let _ = self.finish_once(
            "error",
            Some("LLM invocation cancelled"),
            None,
            Some("cancelled"),
        );
    }
}

/// Record an application-generated local fallback. This event can only emit
/// `actual_model=null` and `fallback_used=false`.
pub fn record_local_fallback(
    execution_context: &RuntimeExecutionContext,
    agent_id: Option<i64>,
    model_preset: Option<&str>,
    source: LlmAuditSource,
    reason: LlmLocalFallbackReason,
) {
    record_local_fallback_with_sink(
        execution_context,
        agent_id,
        model_preset,
        source,
        reason,
        &RuntimeAuditSink,
    );
}

fn record_local_fallback_with_sink(
    execution_context: &RuntimeExecutionContext,
    agent_id: Option<i64>,
    model_preset: Option<&str>,
    source: LlmAuditSource,
    reason: LlmLocalFallbackReason,
    sink: &dyn LlmAuditSink,
) {
    sink.record(
        execution_context,
        AuditRecord {
            agent_id,
            plugin_id: None,
            function_id: None,
            capability: None,
            event_type: "llm_local_fallback",
            outcome: "success",
            elapsed_ms: None,
            error_message: None,
            payload_summary: Some(terminal_summary(
                model_preset,
                None,
                false,
                Some(reason.as_str()),
                source,
            )),
        },
    );
}

fn terminal_summary(
    model_preset: Option<&str>,
    actual_model: Option<&str>,
    fallback_used: bool,
    reason: Option<&str>,
    source: LlmAuditSource,
) -> Value {
    let mut summary = Map::new();
    if let Some(model_preset) = model_preset {
        summary.insert(
            "model_preset".into(),
            Value::String(model_preset.to_string()),
        );
    }
    summary.insert(
        "actual_model".into(),
        actual_model.map_or(Value::Null, |model| Value::String(model.to_string())),
    );
    summary.insert("fallback_used".into(), Value::Bool(fallback_used));
    if let Some(reason) = reason {
        summary.insert("reason".into(), Value::String(reason.to_string()));
    }
    source.add_to_summary(&mut summary);
    Value::Object(summary)
}

fn elapsed_ms(started_at: Instant) -> Option<i32> {
    Some(i32::try_from(started_at.elapsed().as_millis()).unwrap_or(i32::MAX))
}

#[cfg(test)]
mod tests {
    use super::{
        LlmAuditGuard, LlmAuditSink, LlmAuditSource, LlmLocalFallbackReason,
        record_local_fallback_with_sink,
    };
    use crate::runtime::RuntimeExecutionContext;
    use crate::services::runtime_audit::AuditRecord;
    use providers::{FallbackReason, FallbackTransition, LLMResponse};
    use serde_json::{Value, json};
    use std::sync::{Arc, Mutex};

    #[derive(Debug, Clone, PartialEq)]
    struct CapturedAudit {
        agent_id: Option<i64>,
        event_type: String,
        outcome: String,
        error_message: Option<String>,
        payload_summary: Option<Value>,
    }

    #[derive(Default)]
    struct CaptureSink(Mutex<Vec<CapturedAudit>>);

    impl CaptureSink {
        fn records(&self) -> Vec<CapturedAudit> {
            self.0.lock().expect("capture sink poisoned").clone()
        }
    }

    impl LlmAuditSink for CaptureSink {
        fn record(&self, _execution_context: &RuntimeExecutionContext, record: AuditRecord<'_>) {
            self.0
                .lock()
                .expect("capture sink poisoned")
                .push(CapturedAudit {
                    agent_id: record.agent_id,
                    event_type: record.event_type.to_string(),
                    outcome: record.outcome.to_string(),
                    error_message: record.error_message.map(str::to_string),
                    payload_summary: record.payload_summary,
                });
        }
    }

    fn response(
        actual_model: Option<&str>,
        fallback_used: bool,
        reason: Option<FallbackReason>,
    ) -> LLMResponse {
        LLMResponse {
            actual_model: actual_model.map(str::to_string),
            fallback_used,
            reason,
            content: Some("safe response".into()),
            finish_reason: "stop".into(),
            ..LLMResponse::default()
        }
    }

    #[test]
    fn fallback_callback_records_every_transition_synchronously() {
        let sink = Arc::new(CaptureSink::default());
        let mut guard = LlmAuditGuard::with_sink(
            RuntimeExecutionContext::best_effort(Some("request-llm-1".into()), Some(11)),
            Some(42),
            Some("resilient"),
            LlmAuditSource::ToolTest { tool_id: 7 },
            sink.clone(),
        );
        let callback = guard.on_fallback();

        callback(FallbackTransition {
            from_provider_index: 0,
            to_provider_index: 1,
            from_model: "primary-model".into(),
            to_model: "fallback/model-v1".into(),
            reason: FallbackReason::ServerError,
        });
        callback(FallbackTransition {
            from_provider_index: 1,
            to_provider_index: 2,
            from_model: "fallback/model-v1".into(),
            to_model: "fallback/model-v2".into(),
            reason: FallbackReason::RateLimited,
        });
        assert!(guard.finish_success(&response(
            Some("fallback/model-v2"),
            true,
            Some(FallbackReason::RateLimited),
        )));

        let records = sink.records();
        assert_eq!(records.len(), 3);
        assert_eq!(
            records[0].payload_summary,
            Some(json!({
                "model_preset": "resilient",
                "from_provider_ordinal": 0,
                "to_provider_ordinal": 1,
                "from_model": "primary-model",
                "to_model": "fallback/model-v1",
                "reason": "server_error"
            }))
        );
        assert_eq!(
            records[1].payload_summary,
            Some(json!({
                "model_preset": "resilient",
                "from_provider_ordinal": 1,
                "to_provider_ordinal": 2,
                "from_model": "fallback/model-v1",
                "to_model": "fallback/model-v2",
                "reason": "rate_limited"
            }))
        );
        assert_eq!(records[0].event_type, "llm_fallback");
        assert_eq!(records[1].event_type, "llm_fallback");
        assert_eq!(
            records[2].payload_summary,
            Some(json!({
                "model_preset": "resilient",
                "actual_model": "fallback/model-v2",
                "fallback_used": true,
                "reason": "rate_limited",
                "source": "tool_test",
                "tool_id": 7
            }))
        );
    }

    #[test]
    fn explicit_finish_is_exactly_once_for_success_error_and_typed_timeout() {
        let sink = Arc::new(CaptureSink::default());

        {
            let mut guard = LlmAuditGuard::with_sink(
                RuntimeExecutionContext::best_effort(None, None),
                Some(1),
                None,
                LlmAuditSource::Orchestrator,
                sink.clone(),
            );
            let success = response(Some("primary-model"), false, None);
            assert!(guard.finish_success(&success));
            assert!(!guard.finish_error(&success));
            assert!(!guard.finish_timeout());
        }

        {
            let mut guard = LlmAuditGuard::with_sink(
                RuntimeExecutionContext::best_effort(None, None),
                Some(2),
                Some("resilient"),
                LlmAuditSource::BuiltinGameInfo,
                sink.clone(),
            );
            let failed = LLMResponse {
                actual_model: Some("fallback/model-v2".into()),
                fallback_used: true,
                reason: Some(FallbackReason::RetryableError),
                finish_reason: "error".into(),
                ..LLMResponse::default()
            };
            assert!(guard.finish_error(&failed));
            assert!(!guard.finish_error(&failed));
        }

        {
            let mut guard = LlmAuditGuard::with_sink(
                RuntimeExecutionContext::best_effort(None, None),
                Some(3),
                Some("resilient"),
                LlmAuditSource::LlmInvoke,
                sink.clone(),
            );
            assert!(guard.finish_timeout());
            assert!(!guard.finish_timeout());
        }

        let records = sink.records();
        assert_eq!(records.len(), 3);
        assert_eq!(
            records
                .iter()
                .map(|record| record.outcome.as_str())
                .collect::<Vec<_>>(),
            vec!["success", "error", "timeout"]
        );
        assert_eq!(
            records[1].payload_summary.as_ref().unwrap()["reason"],
            "retryable_error"
        );
        assert_eq!(
            records[2].payload_summary.as_ref().unwrap()["reason"],
            "chain_timeout"
        );
    }

    #[test]
    fn unfinished_guard_drop_records_one_cancelled_error() {
        let sink = Arc::new(CaptureSink::default());
        {
            let _guard = LlmAuditGuard::with_sink(
                RuntimeExecutionContext::best_effort(Some("request-cancel".into()), Some(9)),
                Some(4),
                Some("resilient"),
                LlmAuditSource::SkillTest { skill_id: 19 },
                sink.clone(),
            );
        }

        let records = sink.records();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].event_type, "llm_invoke");
        assert_eq!(records[0].outcome, "error");
        assert_eq!(
            records[0].payload_summary,
            Some(json!({
                "model_preset": "resilient",
                "actual_model": null,
                "fallback_used": false,
                "reason": "cancelled",
                "source": "skill_test",
                "skill_id": 19
            }))
        );
    }

    #[test]
    fn production_finish_helpers_classify_typed_timeout_and_preset_error() {
        let sink = Arc::new(CaptureSink::default());
        {
            let mut guard = LlmAuditGuard::with_sink(
                RuntimeExecutionContext::best_effort(None, None),
                Some(6),
                Some("resilient"),
                LlmAuditSource::LlmInvoke,
                sink.clone(),
            );
            let timeout = LLMResponse {
                actual_model: Some("primary-model".into()),
                fallback_used: false,
                reason: Some(FallbackReason::ProviderTimeout),
                finish_reason: "error".into(),
                error_kind: Some("timeout".into()),
                ..LLMResponse::default()
            };
            assert!(guard.finish_response(&timeout));
        }
        {
            let mut guard = LlmAuditGuard::with_sink(
                RuntimeExecutionContext::best_effort(None, None),
                Some(7),
                Some("missing-preset"),
                LlmAuditSource::Orchestrator,
                sink.clone(),
            );
            assert!(guard.finish_model_preset_unknown());
        }

        let records = sink.records();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].outcome, "timeout");
        assert_eq!(
            records[0].payload_summary.as_ref().unwrap()["reason"],
            "provider_timeout"
        );
        assert_eq!(records[1].outcome, "error");
        assert_eq!(
            records[1].payload_summary.as_ref().unwrap()["reason"],
            "model_preset_unknown"
        );
    }

    #[test]
    fn local_fallback_is_typed_and_cannot_impersonate_provider_fallback() {
        let sink = Arc::new(CaptureSink::default());
        record_local_fallback_with_sink(
            &RuntimeExecutionContext::best_effort(Some("request-local".into()), Some(13)),
            Some(5),
            Some("resilient"),
            LlmAuditSource::GenerateAnswer,
            LlmLocalFallbackReason::InvocationFailed,
            sink.as_ref(),
        );

        let records = sink.records();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].event_type, "llm_local_fallback");
        assert_eq!(records[0].outcome, "success");
        assert_eq!(
            records[0].payload_summary,
            Some(json!({
                "model_preset": "resilient",
                "actual_model": null,
                "fallback_used": false,
                "reason": "invocation_failed",
                "source": "generate_answer"
            }))
        );
    }
}
