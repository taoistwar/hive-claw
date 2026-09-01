//! US4 local LLM provider resolution and typed failure envelopes.
//!
//! Source of truth: `specs/011-hivegui-standalone-mode/tasks.md` §T052.
//!
//! Public boundary the T048 Red test drives:
//!   - [`ProviderResolver::new`] / [`ProviderResolver::call`]
//!   - [`ProviderTransport`] (trait injection for deterministic tests)
//!   - [`ProviderCallRequest`] (with `with_cancel_now`)
//!   - [`ProviderCallOutcome`]
//!   - [`ProviderError`] / [`ProviderErrorKind`]
//!   - [`ProviderAttempt`]
//!   - [`TransportRequest`] / [`TransportOutcome`] / [`TransportError`]
//!
//! The resolver's behaviour is bounded by the spec:
//!   * **Token priority** — `LlmProviderTokenInput::Env` always
//!     shadows `LlmProviderTokenInput::Literal`; if the env-var
//!     is unset, the call surfaces `Backend(env-not-set)`.
//!   * **Sorted production fallback** — Preset Models are resolved by
//!     `(priority,id)`, then mapped to their Provider rows.
//!   * **Transient errors fall back** — 429 / 5xx / network /
//!     timeout are all bucketed as `TransportOutcome::TransientFailure`.
//!   * **Auth / parameter / cancel do not fall back** — the
//!     corresponding `ProviderErrorKind` is returned with the
//!     provider name so the UI can show a precise message.

#![warn(missing_docs)]

use std::{
    collections::BTreeSet,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use hive_runtime_core::execution::{
    EventSink, ExecutionContext, PermissionSnapshot, RuntimeEventKind,
};
use providers::{ChatRequest, FallbackTarget, LLMProvider, LlmCallOptions};
use serde_json::{Value, json};
use sqlx::SqlitePool;
use thiserror::Error;

use crate::agent::local_agent::{
    LocalAgentDecisionFuture, LocalAgentDecisionModel, LocalAgentError, TurnSnapshot,
};
use crate::agent::session::{AgentMessage, CancelToken};
use crate::datasource::llm_provider_store::{LlmProviderStore, LlmProviderTokenInput};
use crate::datasource::llm_store::LlmStore;
use crate::datasource::{Crypto, Store};

/// Transport-agnostic request sent to a single provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportRequest {
    provider_name: String,
    model: String,
    message: String,
    base_url: String,
    token: String,
}

impl TransportRequest {
    /// Borrow the provider name.
    pub fn provider_name(&self) -> &str {
        &self.provider_name
    }
    /// Borrow the model identifier.
    pub fn model(&self) -> &str {
        &self.model
    }
    /// Borrow the user message.
    pub fn message(&self) -> &str {
        &self.message
    }
    /// Borrow the base URL.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }
    /// Borrow the bearer token.
    pub fn token(&self) -> &str {
        &self.token
    }
}

/// Transport-agnostic outcome of a single provider call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportOutcome {
    /// Provider returned a successful response with the given content.
    Success {
        /// Assistant text from the provider.
        content: String,
    },
    /// Provider returned a transient failure (5xx, 429, network, timeout).
    /// The resolver MUST fall back to the next provider.
    TransientFailure,
    /// Provider returned 401 / 403 — credential rejected. The
    /// resolver MUST NOT fall back; the surface is the typed
    /// `ProviderErrorKind::Auth`.
    AuthFailure,
    /// Provider returned 400 — bad parameters. The resolver MUST
    /// NOT fall back; the surface is `ProviderErrorKind::Param`.
    ParamFailure,
    /// Caller cancelled the request before completion. The
    /// resolver MUST NOT fall back.
    Cancelled,
}

/// Transport-layer error envelope. The resolver converts each
/// `TransportOutcome` into the matching `ProviderErrorKind`; the
/// `TransportError` is only used for catastrophic transport
/// failures (e.g. a buggy custom transport).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("transport error: {0}")]
pub struct TransportError(pub String);

/// Historical deterministic transport seam retained for the approved T048
/// tests. Production calls use [`ProviderResolver::from_local_config`] and the
/// workspace `providers` implementations instead.
pub trait ProviderTransport: Send + Sync {
    /// Send `request` to the provider and return the outcome.
    fn call(&self, request: TransportRequest) -> Result<TransportOutcome, TransportError>;
}

/// Caller-supplied chat call request.
#[derive(Debug, Clone)]
pub struct ProviderCallRequest {
    model: String,
    message: String,
    preset_name: Option<String>,
    chat_request: Option<ChatRequest>,
    node_timeout: Duration,
    chain_timeout: Duration,
    cancel_now: bool,
}

impl ProviderCallRequest {
    /// Convenience constructor for a single-user-message request.
    pub fn single_message(model: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            message: message.into(),
            preset_name: None,
            chat_request: None,
            node_timeout: Duration::from_secs(25),
            chain_timeout: Duration::from_secs(45),
            cancel_now: false,
        }
    }

    /// Build a production request resolved through one local Preset.
    pub fn for_preset(preset_name: impl Into<String>, chat_request: ChatRequest) -> Self {
        Self {
            model: String::new(),
            message: String::new(),
            preset_name: Some(preset_name.into()),
            chat_request: Some(chat_request),
            node_timeout: Duration::from_secs(25),
            chain_timeout: Duration::from_secs(45),
            cancel_now: false,
        }
    }

    /// Override the per-model and whole-chain wall-clock budgets.
    pub fn with_timeouts(mut self, node_timeout: Duration, chain_timeout: Duration) -> Self {
        self.node_timeout = node_timeout;
        self.chain_timeout = chain_timeout;
        self
    }

    /// Mark this request as already cancelled. The resolver
    /// short-circuits to `ProviderErrorKind::Cancelled` without
    /// touching the transport layer.
    pub fn with_cancel_now(mut self) -> Self {
        self.cancel_now = true;
        self
    }

    /// Borrow the model identifier.
    pub fn model(&self) -> &str {
        &self.model
    }
    /// Borrow the user message.
    pub fn message(&self) -> &str {
        &self.message
    }
    /// Whether the caller pre-cancelled this request.
    pub fn cancel_now(&self) -> bool {
        self.cancel_now
    }
}

/// Successful call result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderCallOutcome {
    /// The resolver returned a successful response from the
    /// provider named here.
    Success {
        /// Provider name that satisfied the call.
        provider_name: String,
        /// Assistant content returned by the provider.
        content: String,
    },
}

/// Stable failure envelope returned by [`ProviderResolver::call`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("provider resolver error: {kind:?}")]
pub struct ProviderError {
    /// Failure kind.
    pub kind: ProviderErrorKind,
}

impl ProviderError {
    /// Borrow the failure kind.
    pub fn kind(&self) -> &ProviderErrorKind {
        &self.kind
    }
}

/// Failure modes for the provider resolver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderErrorKind {
    /// Local Preset/Model/Provider configuration is invalid.
    InvalidInput {
        /// Stable configuration field path.
        field: String,
        /// Stable reason without the rejected value.
        reason: String,
    },
    /// All configured providers failed with transient errors.
    /// The `attempts` list records the provider names and the
    /// error that ended each attempt.
    Exhausted {
        /// Attempt history, in the order the resolver tried them.
        attempts: Vec<ProviderAttempt>,
    },
    /// A provider rejected the credentials. The resolver does
    /// not fall back; the surface records the offending provider.
    Auth {
        /// Provider that returned the auth error.
        provider_name: String,
    },
    /// A provider rejected the parameters. The resolver does
    /// not fall back.
    Param {
        /// Provider that returned the param error.
        provider_name: String,
    },
    /// The caller cancelled the call.
    Cancelled {
        /// Provider that was in-flight when the cancel fired.
        provider_name: String,
    },
    /// The transport layer returned an error that the resolver
    /// could not classify (custom transport bug).
    Backend(String),
}

/// One fallback attempt. The struct is deliberately tiny — only
/// the provider name is exposed so the UI / log layer can
/// describe the attempt chain without leaking the underlying
/// transport error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderAttempt {
    provider_name: String,
}

impl ProviderAttempt {
    /// Build an attempt record.
    pub fn new(provider_name: impl Into<String>) -> Self {
        Self {
            provider_name: provider_name.into(),
        }
    }

    /// Borrow the provider name.
    pub fn provider_name(&self) -> &str {
        &self.provider_name
    }
}

/// Resolver with a legacy deterministic test mode and a production local
/// Preset mode. The production mode delegates all vendor HTTP and parsing to
/// the workspace `providers` crate.
#[derive(Clone)]
pub struct ProviderResolver {
    legacy: Option<(LlmProviderStore, Arc<dyn ProviderTransport>)>,
    local: Option<(SqlitePool, Crypto)>,
}

/// Production decision source for the desktop-local Agent loop.
///
/// The model resolves only the Agent's persisted local Preset (or the unique
/// default Preset), sends an immutable turn snapshot through the workspace
/// provider implementation, and returns the provider's strict JSON decision.
/// It owns no remote product URL or fallback client.
#[derive(Clone)]
pub struct LocalProviderDecisionModel {
    pool: SqlitePool,
    crypto: Crypto,
    execution_id: String,
    session_id: String,
    sink: Arc<dyn EventSink>,
    cancel_token: Option<CancelToken>,
}

impl LocalProviderDecisionModel {
    /// Bind one Agent turn to an opened desktop-local Store.
    ///
    /// `execution_id` and `session_id` must be non-empty. Invalid identifiers,
    /// missing Presets, Provider failures, and cancellation surface as stable
    /// [`LocalAgentError`] values without exposing provider response text.
    pub fn from_store(
        store: &Store,
        execution_id: impl Into<String>,
        session_id: impl Into<String>,
        sink: Arc<dyn EventSink>,
    ) -> Self {
        Self {
            pool: store.pool().clone(),
            crypto: store.crypto().clone(),
            execution_id: execution_id.into(),
            session_id: session_id.into(),
            sink,
            cancel_token: None,
        }
    }

    /// Attach the active local turn's cooperative cancellation token. When
    /// signalled, the in-flight Provider HTTP future is dropped and no
    /// fallback or late decision is accepted.
    pub fn with_cancel_token(mut self, cancel_token: CancelToken) -> Self {
        self.cancel_token = Some(cancel_token);
        self
    }
}

impl LocalAgentDecisionModel for LocalProviderDecisionModel {
    fn decide(
        &self,
        snapshot: TurnSnapshot,
        messages: Vec<AgentMessage>,
        tool_observation: Option<Value>,
    ) -> LocalAgentDecisionFuture {
        let pool = self.pool.clone();
        let crypto = self.crypto.clone();
        let execution_id = self.execution_id.clone();
        let session_id = self.session_id.clone();
        let sink = self.sink.clone();
        let cancel_token = self.cancel_token.clone();
        Box::pin(async move {
            if execution_id.is_empty() || session_id.is_empty() {
                return Err(LocalAgentError::AgentRejected(
                    "execution identity is invalid",
                ));
            }
            let preset = match snapshot
                .agent
                .model_preset()
                .map(str::trim)
                .filter(|preset| !preset.is_empty())
            {
                Some(preset) => preset.to_string(),
                None => LlmStore::new(pool.clone(), crypto.clone())
                    .default_preset_name()
                    .await
                    .map_err(|_| LocalAgentError::AgentRejected("model preset unavailable"))?
                    .ok_or(LocalAgentError::AgentRejected("model preset unavailable"))?,
            };
            let permissions = PermissionSnapshot::new(
                snapshot
                    .capability_names
                    .iter()
                    .cloned()
                    .collect::<BTreeSet<_>>(),
            );
            let context = ExecutionContext::new(
                execution_id,
                session_id,
                snapshot.agent.identifier(),
                permissions,
                sink,
            )
            .map_err(|_| LocalAgentError::AgentRejected("execution identity is invalid"))?;
            let request = ProviderCallRequest::for_preset(
                preset,
                build_agent_chat_request(&snapshot, messages, tool_observation),
            );
            let resolver = ProviderResolver::from_local_config(pool, crypto)
                .map_err(|_| LocalAgentError::AgentRejected("local provider unavailable"))?;
            if cancel_token.as_ref().is_some_and(CancelToken::is_cancelled) {
                return Err(LocalAgentError::AgentRejected(
                    "local provider call cancelled",
                ));
            }
            let provider_call = resolver.call_streaming(&request, &context);
            tokio::pin!(provider_call);
            let outcome = match cancel_token {
                Some(cancel_token) => tokio::select! {
                    result = &mut provider_call => result,
                    _ = wait_for_agent_cancellation(&cancel_token) => {
                        context.cancel_with_reason("agent_stop");
                        provider_call.await
                    }
                },
                None => provider_call.await,
            };
            match outcome {
                Ok(ProviderCallOutcome::Success { content, .. }) if !content.trim().is_empty() => {
                    Ok(content)
                }
                Ok(ProviderCallOutcome::Success { .. }) => {
                    Err(LocalAgentError::AgentRejected("model decision is empty"))
                }
                Err(error) => Err(map_agent_provider_error(error.kind())),
            }
        })
    }
}

async fn wait_for_agent_cancellation(cancel_token: &CancelToken) {
    while !cancel_token.is_cancelled() {
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}

fn build_agent_chat_request(
    snapshot: &TurnSnapshot,
    messages: Vec<AgentMessage>,
    tool_observation: Option<Value>,
) -> ChatRequest {
    let tools = snapshot
        .tools
        .iter()
        .map(|tool| {
            json!({
                "tool_id": tool.id,
                "identifier": tool.identifier,
                "name": tool.name,
                "description": tool.description,
                "input_schema": serde_json::from_str::<Value>(&tool.input_schema)
                    .unwrap_or_else(|_| Value::String(tool.input_schema.clone())),
            })
        })
        .collect::<Vec<_>>();
    let skills = snapshot
        .content
        .skills()
        .iter()
        .map(|skill| json!({"identifier": skill.identifier, "content": skill.content}))
        .collect::<Vec<_>>();
    let capabilities = snapshot
        .content
        .capabilities()
        .iter()
        .map(|capability| json!({"name": capability.name, "is_dangerous": capability.is_dangerous}))
        .collect::<Vec<_>>();
    let direct_children = snapshot
        .direct_children
        .iter()
        .map(|child| child.identifier())
        .collect::<Vec<_>>();
    let protocol = json!({
        "protocol": "hivegui-local-agent-decision-v1",
        "agent_system_prompt": snapshot.content.system_prompt(),
        "skills": skills,
        "capabilities": capabilities,
        "tools": tools,
        "direct_children": direct_children,
        "rules": [
            "Return exactly one JSON object and no markdown.",
            "Allowed forms are reply, tool_call, and route_child.",
            "A tool_call must use a listed numeric tool_id and schema-valid input.",
            "A route_child must use a listed direct child identifier.",
            "All execution is desktop-local; never invent an unlisted route."
        ],
        "wire_examples": [
            {"action":"reply","content":"..."},
            {"action":"tool_call","tool_id":1,"input":{}},
            {"action":"route_child","identifier":"..."}
        ]
    });
    let mut chat_messages = vec![json!({
        "role": "system",
        "content": protocol.to_string()
    })];
    let last_is_tool = messages
        .last()
        .is_some_and(|message| message.speaker() == "tool");
    for message in messages {
        let (role, content) = match message.speaker() {
            "assistant" => ("assistant", message.body().to_string()),
            "tool" => ("user", format!("LOCAL_TOOL_OBSERVATION {}", message.body())),
            _ => ("user", message.body().to_string()),
        };
        chat_messages.push(json!({"role": role, "content": content}));
    }
    if !last_is_tool && let Some(observation) = tool_observation {
        chat_messages.push(json!({
            "role": "user",
            "content": format!("LOCAL_TOOL_OBSERVATION {}", observation)
        }));
    }
    ChatRequest {
        messages: chat_messages,
        ..Default::default()
    }
}

fn map_agent_provider_error(kind: &ProviderErrorKind) -> LocalAgentError {
    let reason = match kind {
        ProviderErrorKind::InvalidInput { .. } => "local provider configuration is invalid",
        ProviderErrorKind::Exhausted { .. } | ProviderErrorKind::Backend(_) => {
            "local provider unavailable"
        }
        ProviderErrorKind::Auth { .. } => "local provider authentication failed",
        ProviderErrorKind::Param { .. } => "local provider rejected the request",
        ProviderErrorKind::Cancelled { .. } => "local provider call cancelled",
    };
    LocalAgentError::AgentRejected(reason)
}

impl ProviderResolver {
    /// Build a resolver against the given store + transport.
    pub fn new(
        store: LlmProviderStore,
        transport: Arc<dyn ProviderTransport>,
    ) -> Result<Self, ProviderError> {
        if Arc::strong_count(&transport) == 0 {
            return Err(ProviderError {
                kind: ProviderErrorKind::Backend("transport handle is dead".into()),
            });
        }
        Ok(Self {
            legacy: Some((store, transport)),
            local: None,
        })
    }

    /// Build the production resolver over canonical local Preset, Model and
    /// Provider rows. Vendor clients are constructed exclusively by the
    /// workspace `providers` crate.
    pub fn from_local_config(pool: SqlitePool, crypto: Crypto) -> Result<Self, ProviderError> {
        Ok(Self {
            legacy: None,
            local: Some((pool, crypto)),
        })
    }

    /// Execute `request` against the configured providers. The
    /// resolver walks the providers in insertion order and stops
    /// at the first success. Transient failures fall through to
    /// the next provider; auth / param / cancel surface
    /// immediately.
    pub async fn call(
        &self,
        request: &ProviderCallRequest,
    ) -> Result<ProviderCallOutcome, ProviderError> {
        let (store, transport) = self.legacy.as_ref().ok_or_else(|| ProviderError {
            kind: ProviderErrorKind::Backend("legacy transport is not configured".into()),
        })?;
        let provider_records = store.list_all().await.map_err(|_| ProviderError {
            kind: ProviderErrorKind::Backend("list providers failed".into()),
        })?;

        if provider_records.is_empty() {
            return Err(ProviderError {
                kind: ProviderErrorKind::Backend("no providers configured".into()),
            });
        }

        if request.cancel_now() {
            // The caller pre-cancelled. Surface as Cancelled with
            // the first candidate provider's name so the UI can
            // attribute the abort to a specific entry. The
            // contract is "the call would have been attempted
            // against this provider".
            return Err(ProviderError {
                kind: ProviderErrorKind::Cancelled {
                    provider_name: provider_records[0].name().to_string(),
                },
            });
        }

        let mut attempts = Vec::new();
        for provider in &provider_records {
            let token = match resolve_token(provider, store.crypto()) {
                Ok(token) => token,
                Err(message) => {
                    return Err(ProviderError {
                        kind: ProviderErrorKind::Backend(format!(
                            "provider '{}' token: {message}",
                            provider.name()
                        )),
                    });
                }
            };

            let transport_request = TransportRequest {
                provider_name: provider.name().to_string(),
                model: request.model().to_string(),
                message: request.message().to_string(),
                base_url: provider.base_url().to_string(),
                token,
            };

            let outcome = transport
                .call(transport_request)
                .map_err(|e| ProviderError {
                    kind: ProviderErrorKind::Backend(format!("transport: {e}")),
                })?;

            match outcome {
                TransportOutcome::Success { content } => {
                    return Ok(ProviderCallOutcome::Success {
                        provider_name: provider.name().to_string(),
                        content,
                    });
                }
                TransportOutcome::TransientFailure => {
                    attempts.push(ProviderAttempt::new(provider.name()));
                    continue;
                }
                TransportOutcome::AuthFailure => {
                    return Err(ProviderError {
                        kind: ProviderErrorKind::Auth {
                            provider_name: provider.name().to_string(),
                        },
                    });
                }
                TransportOutcome::ParamFailure => {
                    return Err(ProviderError {
                        kind: ProviderErrorKind::Param {
                            provider_name: provider.name().to_string(),
                        },
                    });
                }
                TransportOutcome::Cancelled => {
                    return Err(ProviderError {
                        kind: ProviderErrorKind::Cancelled {
                            provider_name: provider.name().to_string(),
                        },
                    });
                }
            }
        }

        Err(ProviderError {
            kind: ProviderErrorKind::Exhausted { attempts },
        })
    }

    /// Resolve a Preset into its priority-ordered workspace provider chain,
    /// forward provider deltas to `context`, and observe cancellation while
    /// the HTTP future is in flight.
    pub async fn call_streaming(
        &self,
        request: &ProviderCallRequest,
        context: &ExecutionContext,
    ) -> Result<ProviderCallOutcome, ProviderError> {
        let (pool, crypto) = self.local.as_ref().ok_or_else(|| ProviderError {
            kind: ProviderErrorKind::Backend("local provider config is not configured".into()),
        })?;
        let preset_name = request
            .preset_name
            .as_deref()
            .ok_or_else(|| ProviderError {
                kind: ProviderErrorKind::InvalidInput {
                    field: "model_preset".into(),
                    reason: "required".into(),
                },
            })?;
        if request.node_timeout.is_zero() || request.chain_timeout < request.node_timeout {
            return Err(ProviderError {
                kind: ProviderErrorKind::InvalidInput {
                    field: "timeout".into(),
                    reason: "node_must_not_exceed_chain".into(),
                },
            });
        }

        let rows = load_local_chain(pool, preset_name).await?;
        let provider_names = rows
            .iter()
            .map(|row| row.provider_name.clone())
            .collect::<Vec<_>>();
        let models = rows
            .iter()
            .map(|row| row.model_name.clone())
            .collect::<Vec<_>>();
        let mut built = Vec::with_capacity(rows.len());
        for row in &rows {
            let token = resolve_local_token(row, crypto)?;
            let backend = providers::find_by_name(&row.provider_category)
                .map(|spec| spec.backend)
                .ok_or_else(|| ProviderError {
                    kind: ProviderErrorKind::InvalidInput {
                        field: "provider".into(),
                        reason: "unsupported_category".into(),
                    },
                })?;
            let provider = providers::build_provider(
                backend,
                providers::ProviderBuildConfig {
                    model: row.model_name.clone(),
                    api_key: Some(token),
                    api_base: Some(row.base_url.clone()),
                    ..Default::default()
                },
            )
            .map_err(|_| ProviderError {
                kind: ProviderErrorKind::InvalidInput {
                    field: "provider".into(),
                    reason: "build_failed".into(),
                },
            })?;
            built.push(provider);
        }

        let primary = built[0].clone();
        let fallback_targets = built
            .into_iter()
            .enumerate()
            .skip(1)
            .map(|(index, provider)| FallbackTarget::new(models[index].clone(), provider))
            .collect::<Vec<_>>();
        let chain: Arc<dyn LLMProvider> =
            Arc::new(providers::FallbackProvider::new(primary, fallback_targets));

        let current_provider_index = Arc::new(AtomicUsize::new(0));
        let fallback_index = current_provider_index.clone();
        let fallback_emitter = context.event_emitter();
        let options = LlmCallOptions::with_timeouts(request.node_timeout, request.chain_timeout)
            .with_fallback_callback(Arc::new(move |transition| {
                fallback_index.store(transition.to_provider_index, Ordering::SeqCst);
                let _ = fallback_emitter.emit(RuntimeEventKind::FallbackUsed {
                    from_model: transition.from_model,
                    to_model: transition.to_model,
                    reason: transition.reason.as_str().to_string(),
                });
            }));
        let token_emitter = context.event_emitter();
        let token_agent_id = context.agent_id().to_string();
        let on_delta = Arc::new(move |text: String| {
            if !text.is_empty() {
                let _ = token_emitter.emit(RuntimeEventKind::Token {
                    text,
                    agent_id: token_agent_id.clone(),
                });
            }
        });

        let mut chat_request = request.chat_request.clone().ok_or_else(|| ProviderError {
            kind: ProviderErrorKind::InvalidInput {
                field: "request".into(),
                reason: "missing_chat".into(),
            },
        })?;
        chat_request.model = Some(rows[0].model_name.clone());
        chat_request.max_tokens = rows[0].max_tokens.max(1) as u32;
        chat_request.temperature = rows[0].temperature as f32;

        let started = Instant::now();
        if request.cancel_now || context.is_cancelled() {
            context.record_segment_ms("llm_ms", elapsed_ms(started));
            return Err(cancelled(&provider_names, 0));
        }
        let provider_call =
            chain.chat_stream_with_options(chat_request, Some(on_delta), None, options);
        tokio::pin!(provider_call);
        let response = tokio::select! {
            biased;
            _ = wait_for_cancellation(context) => {
                let index = current_provider_index.load(Ordering::SeqCst);
                context.record_segment_ms("llm_ms", elapsed_ms(started));
                return Err(cancelled(&provider_names, index));
            }
            response = &mut provider_call => response,
        };
        context.record_segment_ms("llm_ms", elapsed_ms(started));

        let provider_index = current_provider_index
            .load(Ordering::SeqCst)
            .min(provider_names.len().saturating_sub(1));
        if response.finish_reason != "error" {
            return Ok(ProviderCallOutcome::Success {
                provider_name: provider_names[provider_index].clone(),
                content: response.content.unwrap_or_default(),
            });
        }

        let kind = response.error_kind.as_deref().unwrap_or_default();
        let status = response.error_status_code.unwrap_or_default();
        if matches!(status, 401 | 403) || matches!(kind, "auth" | "authentication" | "permission") {
            return Err(ProviderError {
                kind: ProviderErrorKind::Auth {
                    provider_name: provider_names[provider_index].clone(),
                },
            });
        }
        if status == 400 || matches!(kind, "invalid_request" | "context_length") {
            return Err(ProviderError {
                kind: ProviderErrorKind::Param {
                    provider_name: provider_names[provider_index].clone(),
                },
            });
        }
        Err(ProviderError {
            kind: ProviderErrorKind::Exhausted {
                attempts: provider_names
                    .iter()
                    .take(provider_index + 1)
                    .map(ProviderAttempt::new)
                    .collect(),
            },
        })
    }
}

#[derive(Debug, sqlx::FromRow)]
struct LocalProviderRow {
    model_name: String,
    provider_name: String,
    provider_category: String,
    base_url: String,
    token_encrypted: Option<Vec<u8>>,
    token_env: String,
    max_tokens: i32,
    temperature: f64,
}

async fn load_local_chain(
    pool: &SqlitePool,
    preset_name: &str,
) -> Result<Vec<LocalProviderRow>, ProviderError> {
    let rows = sqlx::query_as::<_, LocalProviderRow>(
        "SELECT m.name AS model_name, p.name AS provider_name, \
                p.category AS provider_category, p.base_url, p.token_encrypted, p.token_env, \
                preset.max_tokens, preset.temperature \
         FROM llm_presets preset \
         JOIN models m ON m.preset_id = preset.id \
         JOIN llm_providers p ON p.id = m.provider_id \
         WHERE preset.name = ? \
         ORDER BY m.priority ASC, m.id ASC",
    )
    .bind(preset_name)
    .fetch_all(pool)
    .await
    .map_err(|_| ProviderError {
        kind: ProviderErrorKind::Backend("load local provider chain failed".into()),
    })?;
    if rows.is_empty() {
        return Err(ProviderError {
            kind: ProviderErrorKind::InvalidInput {
                field: "model_preset".into(),
                reason: "not_found_or_empty".into(),
            },
        });
    }
    Ok(rows)
}

fn resolve_local_token(row: &LocalProviderRow, crypto: &Crypto) -> Result<String, ProviderError> {
    if !row.token_env.is_empty() {
        return std::env::var(&row.token_env).map_err(|_| ProviderError {
            kind: ProviderErrorKind::InvalidInput {
                field: "provider".into(),
                reason: "token_unavailable".into(),
            },
        });
    }
    let encrypted = row
        .token_encrypted
        .as_deref()
        .ok_or_else(|| ProviderError {
            kind: ProviderErrorKind::InvalidInput {
                field: "provider".into(),
                reason: "token_unavailable".into(),
            },
        })?;
    let plaintext = crypto.decrypt(encrypted).map_err(|_| ProviderError {
        kind: ProviderErrorKind::InvalidInput {
            field: "provider".into(),
            reason: "token_unavailable".into(),
        },
    })?;
    String::from_utf8(plaintext).map_err(|_| ProviderError {
        kind: ProviderErrorKind::InvalidInput {
            field: "provider".into(),
            reason: "token_unavailable".into(),
        },
    })
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}

fn cancelled(provider_names: &[String], index: usize) -> ProviderError {
    ProviderError {
        kind: ProviderErrorKind::Cancelled {
            provider_name: provider_names[index.min(provider_names.len().saturating_sub(1))]
                .clone(),
        },
    }
}

async fn wait_for_cancellation(context: &ExecutionContext) {
    while !context.is_cancelled() {
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}

/// Resolve the bearer token. The env-var branch always shadows
/// the stored ciphertext when the caller supplied an env-var
/// name. The literal branch uses the device key (held by the
/// store) to decrypt the stored ciphertext; the plaintext
/// reaches the transport layer through the resolver's call to
/// [`LlmProviderRecord::resolve_token`].
fn resolve_token(
    provider: &crate::datasource::llm_provider_store::LlmProviderRecord,
    crypto: &crate::datasource::Crypto,
) -> Result<String, String> {
    match provider.token_input() {
        LlmProviderTokenInput::Env(name) => {
            let name_ref = name.as_str();
            std::env::var(&name)
                .map_err(|_| format!("environment variable '{name_ref}' is not set"))
        }
        LlmProviderTokenInput::Literal(_) => provider
            .resolve_token(crypto)?
            .ok_or_else(|| "provider has no resolvable token".to_string()),
    }
}
