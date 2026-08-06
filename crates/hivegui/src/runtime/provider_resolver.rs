//! US4 [P] LLM provider resolver with sorted fallback, env-var token
//! priority, and typed error envelopes.
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
//!   * **Sorted fallback** — providers are tried in stable
//!     insertion order (the store-side priority column is the
//!     authoritative order; v1 falls back to row id).
//!   * **Transient errors fall back** — 429 / 5xx / network /
//!     timeout are all bucketed as `TransportOutcome::TransientFailure`.
//!   * **Auth / parameter / cancel do not fall back** — the
//!     corresponding `ProviderErrorKind` is returned with the
//!     provider name so the UI can show a precise message.

#![warn(missing_docs)]

use std::sync::Arc;

use thiserror::Error;

use crate::datasource::llm_provider_store::{LlmProviderStore, LlmProviderTokenInput};

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

/// Trait abstraction over the HTTP layer. The default
/// implementation is a real HTTP client; tests inject a mock
/// transport that returns a scripted sequence of outcomes.
pub trait ProviderTransport: Send + Sync {
    /// Send `request` to the provider and return the outcome.
    fn call(&self, request: TransportRequest) -> Result<TransportOutcome, TransportError>;
}

/// Caller-supplied chat call request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderCallRequest {
    model: String,
    message: String,
    cancel_now: bool,
}

impl ProviderCallRequest {
    /// Convenience constructor for a single-user-message request.
    pub fn single_message(model: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            message: message.into(),
            cancel_now: false,
        }
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

/// Resolver that maps the local LLM provider store into a
/// sorted fallback chain. The resolver is constructed against a
/// transport so production wiring can use a real HTTP client
/// while tests inject a scripted mock.
#[derive(Clone)]
pub struct ProviderResolver {
    store: LlmProviderStore,
    transport: Arc<dyn ProviderTransport>,
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
        Ok(Self { store, transport })
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
        let providers = self.store.list_all().await.map_err(|e| ProviderError {
            kind: ProviderErrorKind::Backend(format!("list providers: {e}")),
        })?;

        if providers.is_empty() {
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
                    provider_name: providers[0].name().to_string(),
                },
            });
        }

        let mut attempts = Vec::new();
        for provider in &providers {
            let token = match resolve_token(provider, self.store.crypto()) {
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

            let outcome = self
                .transport
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
