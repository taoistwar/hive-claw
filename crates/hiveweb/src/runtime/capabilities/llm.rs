//! llm.invoke capability handler (T099 / US4 commit 4)
//!
//! Plugin 调用 LLM 走 Agent 当前 model_preset 的完整 provider fallback chain。
//! 简化版：返回 single-shot chat completion（无 streaming，无 tool_calls）。
//!
//! 真实的 chat 路由 / tool-calling 循环在 orchestrator 已实现；本 capability
//! 是给 **Plugin 内部** 在 host_call 范围内做一次性 LLM 询问（如 summarize、
//! classify 等无 tool 的子任务）。

use providers::{
    ChatRequest, FallbackReason, FallbackTransitionCallback, LLMProvider, LLMResponse,
    LlmCallOptions,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::MySqlPool;
use std::sync::Arc;
use std::time::Duration;

use super::CapabilityFailure;
use crate::runtime::execution_context::RuntimeExecutionContext;
use crate::runtime::llm::{LlmAdapterError, LlmRegistry};
use crate::runtime::llm_audit::{LlmAuditGuard, LlmAuditSource};

#[derive(Debug, Deserialize)]
pub struct LlmInvokeArgs {
    /// 简化：plugin 提供 messages 数组 (OpenAI-style) 或 prompt 字符串
    #[serde(default)]
    pub messages: Vec<Value>,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub temperature: Option<f32>,
}

#[derive(Debug, Serialize)]
pub struct LlmInvokeReply {
    pub content: Option<String>,
    pub finish_reason: String,
    pub usage: serde_json::Map<String, Value>,
    pub actual_model: Option<String>,
    pub fallback_used: bool,
    pub reason: Option<FallbackReason>,
}

trait LlmInvokeRegistry {
    fn resolve_preset_name(&self, preset_name: Option<&str>) -> Result<String, LlmAdapterError>;

    fn resolve_generation_defaults(
        &self,
        preset_name: Option<&str>,
    ) -> Result<(u32, f32), LlmAdapterError>;

    fn build_chain(
        &self,
        preset_name: Option<&str>,
    ) -> Result<(Arc<dyn LLMProvider>, String), LlmAdapterError>;
}

impl LlmInvokeRegistry for LlmRegistry {
    fn resolve_preset_name(&self, preset_name: Option<&str>) -> Result<String, LlmAdapterError> {
        LlmRegistry::resolve(self, preset_name).map(|preset| preset.name.clone())
    }

    fn resolve_generation_defaults(
        &self,
        preset_name: Option<&str>,
    ) -> Result<(u32, f32), LlmAdapterError> {
        LlmRegistry::resolve_generation_defaults(self, preset_name)
    }

    fn build_chain(
        &self,
        preset_name: Option<&str>,
    ) -> Result<(Arc<dyn LLMProvider>, String), LlmAdapterError> {
        LlmRegistry::build_chain(self, preset_name)
    }
}

/// `agent_id` 用于解析当前 agent 的 model_preset（沿用 dispatch ctx）
pub async fn llm_invoke(
    pool: &MySqlPool,
    llm: &Arc<LlmRegistry>,
    execution_context: &RuntimeExecutionContext,
    agent_id: i64,
    args: LlmInvokeArgs,
) -> Result<LlmInvokeReply, CapabilityFailure> {
    validate_args(&args)?;

    // resolve agent.model_preset
    let preset: Option<(Option<String>,)> =
        sqlx::query_as("SELECT model_preset FROM agents WHERE id = ?")
            .bind(agent_id)
            .fetch_optional(pool)
            .await
            .map_err(|_| CapabilityFailure::failed("agent preset lookup failed"))?;

    invoke_for_agent_row(llm.as_ref(), execution_context, agent_id, preset, args).await
}

fn validate_args(args: &LlmInvokeArgs) -> Result<(), CapabilityFailure> {
    if args.messages.is_empty() && args.prompt.is_none() {
        return Err(CapabilityFailure::failed(
            "llm.invoke requires messages or prompt",
        ));
    }
    if args.max_tokens == Some(0) {
        return Err(CapabilityFailure::invalid_arguments());
    }
    Ok(())
}

async fn invoke_for_agent_row<R: LlmInvokeRegistry + ?Sized>(
    registry: &R,
    execution_context: &RuntimeExecutionContext,
    agent_id: i64,
    agent_row: Option<(Option<String>,)>,
    args: LlmInvokeArgs,
) -> Result<LlmInvokeReply, CapabilityFailure> {
    let Some((preset_name,)) = agent_row else {
        return Err(CapabilityFailure::failed("agent not found"));
    };

    invoke_for_preset(
        registry,
        execution_context,
        agent_id,
        preset_name.as_deref(),
        args,
    )
    .await
}

async fn invoke_for_preset<R: LlmInvokeRegistry + ?Sized>(
    registry: &R,
    execution_context: &RuntimeExecutionContext,
    agent_id: i64,
    preset_name: Option<&str>,
    args: LlmInvokeArgs,
) -> Result<LlmInvokeReply, CapabilityFailure> {
    let resolved_preset_name = match registry.resolve_preset_name(preset_name) {
        Ok(name) => name,
        Err(error) => {
            let mut audit = LlmAuditGuard::new(
                execution_context.clone(),
                Some(agent_id),
                preset_name,
                LlmAuditSource::LlmInvoke,
            );
            return Err(finish_registry_failure(&mut audit, error));
        }
    };
    let mut audit = LlmAuditGuard::new(
        execution_context.clone(),
        Some(agent_id),
        Some(&resolved_preset_name),
        LlmAuditSource::LlmInvoke,
    );
    let resolved_preset_name = Some(resolved_preset_name.as_str());

    let defaults = match registry.resolve_generation_defaults(resolved_preset_name) {
        Ok(defaults) => defaults,
        Err(error) => return Err(finish_registry_failure(&mut audit, error)),
    };
    let (provider, model) = match registry.build_chain(resolved_preset_name) {
        Ok(chain) => chain,
        Err(error) => return Err(finish_registry_failure(&mut audit, error)),
    };
    let request = build_request(args, model, defaults);
    let options = plugin_call_options(audit.on_fallback());
    let response = provider.chat_with_options(request, options).await;
    let finished = audit.finish_response(&response);
    debug_assert!(finished, "llm.invoke audit must finish exactly once");

    if response.is_error() {
        return Err(classify_provider_failure(&response));
    }
    Ok(reply_from_response(response))
}

fn build_request(args: LlmInvokeArgs, model: String, defaults: (u32, f32)) -> ChatRequest {
    let messages = if !args.messages.is_empty() {
        args.messages
    } else {
        vec![serde_json::json!({
            "role": "user",
            "content": args.prompt.unwrap_or_default(),
        })]
    };

    ChatRequest {
        model: Some(model),
        messages,
        max_tokens: args.max_tokens.unwrap_or(defaults.0),
        temperature: args.temperature.unwrap_or(defaults.1),
        tools: None,
        tool_choice: None,
        reasoning_effort: None,
    }
}

fn plugin_call_options(callback: FallbackTransitionCallback) -> LlmCallOptions {
    LlmCallOptions::with_timeouts(Duration::from_secs(25), Duration::from_secs(25))
        .with_fallback_callback(callback)
}

fn reply_from_response(response: LLMResponse) -> LlmInvokeReply {
    let usage_value =
        serde_json::to_value(&response.usage).unwrap_or(Value::Object(serde_json::Map::new()));
    let usage_map = usage_value.as_object().cloned().unwrap_or_default();

    LlmInvokeReply {
        content: response.content,
        finish_reason: response.finish_reason,
        usage: usage_map,
        actual_model: response.actual_model,
        fallback_used: response.fallback_used,
        reason: response.reason,
    }
}

fn finish_registry_failure(audit: &mut LlmAuditGuard, error: LlmAdapterError) -> CapabilityFailure {
    if matches!(error, LlmAdapterError::Unknown(_)) {
        let finished = audit.finish_model_preset_unknown();
        debug_assert!(finished, "llm.invoke audit must finish exactly once");
        return CapabilityFailure::model_preset_unknown();
    }

    let response = LLMResponse {
        finish_reason: "error".to_string(),
        error_kind: Some("registry_error".to_string()),
        error_should_retry: Some(false),
        ..Default::default()
    };
    let finished = audit.finish_response(&response);
    debug_assert!(finished, "llm.invoke audit must finish exactly once");
    CapabilityFailure::failed("provider construction failed")
}

fn classify_provider_failure(response: &providers::LLMResponse) -> CapabilityFailure {
    if response
        .error_kind
        .as_deref()
        .is_some_and(|kind| kind.trim().eq_ignore_ascii_case("timeout"))
        || matches!(
            response.error_code.as_deref(),
            Some("node_timeout" | "chain_timeout")
        )
    {
        CapabilityFailure::timeout()
    } else {
        CapabilityFailure::failed("provider invocation failed")
    }
}

#[cfg(test)]
mod tests {
    use super::{
        LlmInvokeArgs, LlmInvokeRegistry, classify_provider_failure, invoke_for_agent_row,
        invoke_for_preset, validate_args,
    };
    use crate::runtime::capabilities::CapabilityFailureKind;
    use crate::runtime::execution_context::RuntimeExecutionContext;
    use crate::runtime::llm::LlmAdapterError;
    use async_trait::async_trait;
    use providers::{ChatRequest, FallbackReason, LLMProvider, LLMResponse, LlmCallOptions};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    struct CapturingProvider {
        calls: AtomicUsize,
        requests: Mutex<Vec<ChatRequest>>,
        options: Mutex<Vec<(Duration, Duration, bool)>>,
        response: LLMResponse,
    }

    impl CapturingProvider {
        fn returning(response: LLMResponse) -> Arc<Self> {
            Arc::new(Self {
                calls: AtomicUsize::new(0),
                requests: Mutex::new(Vec::new()),
                options: Mutex::new(Vec::new()),
                response,
            })
        }
    }

    #[async_trait]
    impl LLMProvider for CapturingProvider {
        fn default_model(&self) -> String {
            "default-must-not-be-requested".to_string()
        }

        async fn chat(&self, _request: ChatRequest) -> LLMResponse {
            panic!("llm.invoke must call chat_with_options, not raw chat or retry wrappers")
        }

        async fn chat_with_options(
            &self,
            request: ChatRequest,
            options: LlmCallOptions,
        ) -> LLMResponse {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.requests.lock().expect("request lock").push(request);
            self.options.lock().expect("options lock").push((
                options.node_timeout,
                options.chain_timeout,
                options.on_fallback.is_some(),
            ));
            self.response.clone()
        }
    }

    struct FakeRegistry {
        provider: Arc<dyn LLMProvider>,
        defaults: (u32, f32),
        unknown: bool,
        build_calls: AtomicUsize,
        resolution_calls: Mutex<Vec<(&'static str, Option<String>)>>,
    }

    impl FakeRegistry {
        fn available(provider: Arc<dyn LLMProvider>, defaults: (u32, f32)) -> Self {
            Self {
                provider,
                defaults,
                unknown: false,
                build_calls: AtomicUsize::new(0),
                resolution_calls: Mutex::new(Vec::new()),
            }
        }

        fn unknown(provider: Arc<dyn LLMProvider>) -> Self {
            Self {
                provider,
                defaults: (999, 0.99),
                unknown: true,
                build_calls: AtomicUsize::new(0),
                resolution_calls: Mutex::new(Vec::new()),
            }
        }
    }

    impl LlmInvokeRegistry for FakeRegistry {
        fn resolve_preset_name(
            &self,
            preset_name: Option<&str>,
        ) -> Result<String, LlmAdapterError> {
            self.resolution_calls
                .lock()
                .expect("resolution calls lock")
                .push(("resolve", preset_name.map(str::to_string)));
            if self.unknown {
                return Err(LlmAdapterError::Unknown(
                    preset_name.unwrap_or("resolved-default").to_string(),
                ));
            }
            Ok(preset_name.unwrap_or("resolved-default").to_string())
        }

        fn resolve_generation_defaults(
            &self,
            preset_name: Option<&str>,
        ) -> Result<(u32, f32), LlmAdapterError> {
            self.resolution_calls
                .lock()
                .expect("resolution calls lock")
                .push(("defaults", preset_name.map(str::to_string)));
            if self.unknown {
                return Err(LlmAdapterError::Unknown(
                    preset_name.unwrap_or("default").to_string(),
                ));
            }
            Ok(self.defaults)
        }

        fn build_chain(
            &self,
            preset_name: Option<&str>,
        ) -> Result<(Arc<dyn LLMProvider>, String), LlmAdapterError> {
            self.resolution_calls
                .lock()
                .expect("resolution calls lock")
                .push(("build", preset_name.map(str::to_string)));
            self.build_calls.fetch_add(1, Ordering::SeqCst);
            if self.unknown {
                return Err(LlmAdapterError::Unknown(
                    preset_name.unwrap_or("default").to_string(),
                ));
            }
            Ok((Arc::clone(&self.provider), "configured-model".to_string()))
        }
    }

    fn tracing_only_context(request_id: &str) -> RuntimeExecutionContext {
        RuntimeExecutionContext::best_effort(Some(request_id.to_string()), Some(17)).for_hook()
    }

    fn success_response() -> LLMResponse {
        LLMResponse {
            content: Some("ok".to_string()),
            finish_reason: "stop".to_string(),
            actual_model: Some("fallback-model".to_string()),
            fallback_used: true,
            reason: Some(FallbackReason::ServerError),
            ..Default::default()
        }
    }

    #[test]
    fn explicit_zero_max_tokens_is_rejected_as_invalid_arguments() {
        let args = LlmInvokeArgs {
            messages: Vec::new(),
            prompt: Some("must not reach a provider".to_string()),
            max_tokens: Some(0),
            temperature: None,
        };

        let failure = validate_args(&args).expect_err("zero max_tokens must fail");

        assert_eq!(failure.kind(), CapabilityFailureKind::InvalidArguments);
    }

    #[tokio::test]
    async fn preset_defaults_and_structured_reply_use_the_canonical_default_preset() {
        let provider = CapturingProvider::returning(success_response());
        let registry =
            FakeRegistry::available(Arc::clone(&provider) as Arc<dyn LLMProvider>, (321, 0.42));

        let reply = invoke_for_agent_row(
            &registry,
            &tracing_only_context("llm-invoke-defaults"),
            9,
            Some((None,)),
            LlmInvokeArgs {
                messages: Vec::new(),
                prompt: Some("summarize".to_string()),
                max_tokens: None,
                temperature: None,
            },
        )
        .await
        .expect("invoke");

        let requests = provider.requests.lock().expect("request lock");
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].max_tokens, 321);
        assert_eq!(requests[0].temperature, 0.42);
        assert_eq!(requests[0].model.as_deref(), Some("configured-model"));
        assert_eq!(requests[0].messages[0]["content"], "summarize");
        drop(requests);
        assert_eq!(reply.actual_model.as_deref(), Some("fallback-model"));
        assert!(reply.fallback_used);
        assert_eq!(reply.reason, Some(FallbackReason::ServerError));
        assert_eq!(
            serde_json::to_value(&reply).expect("serialize reply")["reason"],
            "server_error"
        );
        assert_eq!(
            provider.options.lock().expect("options lock").as_slice(),
            [(Duration::from_secs(25), Duration::from_secs(25), true)]
        );
        assert_eq!(
            registry
                .resolution_calls
                .lock()
                .expect("resolution calls lock")
                .as_slice(),
            [
                ("resolve", None),
                ("defaults", Some("resolved-default".to_string())),
                ("build", Some("resolved-default".to_string())),
            ]
        );
    }

    #[tokio::test]
    async fn missing_agent_row_fails_before_building_or_calling_a_provider() {
        let provider = CapturingProvider::returning(success_response());
        let registry =
            FakeRegistry::available(Arc::clone(&provider) as Arc<dyn LLMProvider>, (321, 0.42));

        let failure = invoke_for_agent_row(
            &registry,
            &tracing_only_context("llm-invoke-missing-agent"),
            404,
            None,
            LlmInvokeArgs {
                messages: Vec::new(),
                prompt: Some("must not run".to_string()),
                max_tokens: None,
                temperature: None,
            },
        )
        .await
        .expect_err("a missing Agent row must fail closed");

        assert_eq!(failure.kind(), CapabilityFailureKind::Failed);
        assert_eq!(registry.build_calls.load(Ordering::SeqCst), 0);
        assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn explicit_generation_values_override_preset_defaults() {
        let provider = CapturingProvider::returning(success_response());
        let registry =
            FakeRegistry::available(Arc::clone(&provider) as Arc<dyn LLMProvider>, (321, 0.42));

        invoke_for_preset(
            &registry,
            &tracing_only_context("llm-invoke-overrides"),
            9,
            Some("agent-preset"),
            LlmInvokeArgs {
                messages: vec![serde_json::json!({"role": "user", "content": "hello"})],
                prompt: None,
                max_tokens: Some(77),
                temperature: Some(0.91),
            },
        )
        .await
        .expect("invoke");

        let requests = provider.requests.lock().expect("request lock");
        assert_eq!(requests[0].max_tokens, 77);
        assert_eq!(requests[0].temperature, 0.91);
    }

    #[tokio::test]
    async fn explicit_unknown_preset_is_typed_and_never_builds_or_calls_default() {
        let provider = CapturingProvider::returning(success_response());
        let registry = FakeRegistry::unknown(Arc::clone(&provider) as Arc<dyn LLMProvider>);

        let failure = invoke_for_preset(
            &registry,
            &tracing_only_context("llm-invoke-unknown"),
            9,
            Some("removed-preset"),
            LlmInvokeArgs {
                messages: Vec::new(),
                prompt: Some("must not run".to_string()),
                max_tokens: None,
                temperature: None,
            },
        )
        .await
        .expect_err("unknown preset must fail closed");

        assert_eq!(failure.kind(), CapabilityFailureKind::ModelPresetUnknown);
        assert_eq!(registry.build_calls.load(Ordering::SeqCst), 0);
        assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn structured_timeout_is_typed_and_still_uses_the_25_second_plugin_budget() {
        let provider = CapturingProvider::returning(LLMResponse {
            finish_reason: "error".to_string(),
            error_kind: Some("timeout".to_string()),
            error_code: Some("chain_timeout".to_string()),
            ..Default::default()
        });
        let registry =
            FakeRegistry::available(Arc::clone(&provider) as Arc<dyn LLMProvider>, (321, 0.42));

        let failure = invoke_for_preset(
            &registry,
            &tracing_only_context("llm-invoke-timeout"),
            9,
            Some("agent-preset"),
            LlmInvokeArgs {
                messages: Vec::new(),
                prompt: Some("timeout".to_string()),
                max_tokens: None,
                temperature: None,
            },
        )
        .await
        .expect_err("timeout");

        assert_eq!(failure.kind(), CapabilityFailureKind::Timeout);
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            provider.options.lock().expect("options lock").as_slice(),
            [(Duration::from_secs(25), Duration::from_secs(25), true)]
        );
    }

    #[test]
    fn provider_content_cannot_impersonate_structured_timeout_metadata() {
        let response = LLMResponse {
            content: Some("upstream said timeout: PROVIDER_TIMEOUT_SENTINEL".into()),
            ..LLMResponse::default()
        };
        assert_eq!(
            classify_provider_failure(&response).kind(),
            CapabilityFailureKind::Failed
        );

        let response = LLMResponse {
            content: Some("safe provider failure".into()),
            error_kind: Some("timeout".into()),
            ..LLMResponse::default()
        };
        assert_eq!(
            classify_provider_failure(&response).kind(),
            CapabilityFailureKind::Timeout
        );
    }
}
