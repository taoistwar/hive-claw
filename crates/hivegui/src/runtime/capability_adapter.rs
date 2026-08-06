//! US7 [P] Capability dispatch adapter — separates capability
//! *metadata* (the `capabilities` table rows: name + description +
//! is_dangerous) from the *actual local handler* registry. The
//! registry is the single source of truth for "what is executable
//! in this HiveGUI process".
//!
//! Source of truth: `specs/011-hivegui-standalone-mode/tasks.md`
//! §T067 / §T070. The public boundary the T067 Red test drives:
//!
//!   - [`CapabilityAdapter`]
//!   - [`HandlerRegistry`]
//!   - [`AdapterErrorCode`]
//!   - [`DispatchAuditEvent`]
//!   - [`CapabilityDispatchError`]
//!   - [`AuditSink`]
//!
//! The dispatcher's stable order of checks is:
//!
//! ```text
//! parse → unknown → unauthorized → handler-existence → argument-shape
//! ```
//!
//! A "later" failure must never mask an "earlier" failure. The
//! order is part of the contract, not a coincidence of the
//! implementation; every test in `runtime_capability_catalog.rs`
//! enforces at least one boundary.

#![warn(missing_docs)]

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Stable wire code for a capability-dispatch failure. The
/// numeric value is part of the public contract: it is what
/// `DesktopHostDispatcher` emits on the wire and what the UI
/// inspects to drive a stable error message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdapterErrorCode {
    /// Envelope failed to parse as a `CallEnvelope` JSON object.
    InvalidEnvelope,
    /// Capability name is not in the catalog.
    Unknown,
    /// Capability is not in the caller's permission snapshot.
    Unauthorized,
    /// Capability exists as a metadata row only; no local handler
    /// is registered. The dispatcher MUST NOT execute the call.
    MetadataOnly,
    /// Capability exists but its handler is missing — the registry
    /// registered a name without a callable body. Treated as
    /// `MetadataOnly` for downstream consumers.
    HandlerMissing,
    /// Capability handler exists but the JSON `args` value does
    /// not match the expected shape (or the registry refused
    /// shape-specific validation).
    ArgumentShape,
    /// The handler ran and returned a typed error envelope.
    HandlerError,
}

impl AdapterErrorCode {
    /// Stable wire code emitted by the dispatcher.
    pub fn wire(self) -> u16 {
        match self {
            Self::InvalidEnvelope => 4001,
            Self::Unknown => 4045,
            Self::Unauthorized => 4030,
            Self::MetadataOnly => 4050,
            Self::HandlerMissing => 4051,
            Self::ArgumentShape => 4010,
            Self::HandlerError => 4020,
        }
    }

    /// Parse a wire code observed on the JSON envelope. Unknown
    /// values fall back to [`AdapterErrorCode::HandlerError`]
    /// (the most conservative "later" failure class) so callers
    /// that switch on the typed enum never panic on a forward
    /// compatible code that does not exist yet.
    pub fn from_wire(code: u64) -> Self {
        match code {
            4001 => Self::InvalidEnvelope,
            4045 => Self::Unknown,
            4030 => Self::Unauthorized,
            4050 => Self::MetadataOnly,
            4051 => Self::HandlerMissing,
            4010 => Self::ArgumentShape,
            4020 => Self::HandlerError,
            _ => Self::HandlerError,
        }
    }

    /// Short human-readable label. The T067 contract uses this in
    /// the audit-event `outcome` field.
    pub fn label(self) -> &'static str {
        match self {
            Self::InvalidEnvelope => "invalid_envelope",
            Self::Unknown => "unknown",
            Self::Unauthorized => "unauthorized",
            Self::MetadataOnly => "metadata_only",
            Self::HandlerMissing => "metadata_only",
            Self::ArgumentShape => "argument_shape",
            Self::HandlerError => "handler_error",
        }
    }
}

/// The local handler for a single capability. The closure receives
/// the parsed `args` JSON value and returns either a successful
/// result or a stable error message. The error string is forwarded
/// to the audit sink and is redacted of any token-shaped
/// material before it leaves the adapter.
pub type CapabilityHandler = Arc<dyn Fn(&Value) -> Result<Value, String> + Send + Sync>;

/// The handler registry. The constructor is intentionally
/// restricted to two helpers used by the T067 Red test:
/// [`HandlerRegistry::for_test_with_metadata_only`] (registers a
/// name without a callable body) and
/// [`HandlerRegistry::for_test_with_handler`] (registers a name
/// with a closure). Production code wires the registry at
/// process start; tests use the helpers to seed the catalog.
#[derive(Clone, Default)]
pub struct HandlerRegistry {
    /// Capability name → handler. `None` means the row is
    /// metadata-only and must surface `MetadataOnly`.
    handlers: HashMap<String, Option<CapabilityHandler>>,
}

impl HandlerRegistry {
    /// Merge another registry into `self`. Used by
    /// `desktop_host::register_known` so the legacy catalog
    /// can compose with adapter-only entries without
    /// exposing the inner `HashMap` to callers.
    pub fn merge_from(&mut self, other: HandlerRegistry) {
        for (name, handler) in other.handlers {
            self.handlers.insert(name, handler);
        }
    }
}

impl std::fmt::Debug for HandlerRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HandlerRegistry")
            .field("known", &self.handlers.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl HandlerRegistry {
    /// Register a single capability backed by a real handler.
    /// Used by every T067 dispatch test that wants the handler
    /// path to run end-to-end.
    pub fn for_test_with_handler<F>(name: &str, handler: F) -> Self
    where
        F: Fn(&Value) -> Result<Value, String> + Send + Sync + 'static,
    {
        Self::for_test_with_handler_arc(name, Arc::new(handler))
    }

    /// Internal constructor accepting an `Arc<dyn Fn>` so callers
    /// that already hold a boxed handler (e.g. production wiring
    /// in `desktop_host.rs`) can register without re-boxing.
    pub fn for_test_with_handler_arc(name: &str, handler: CapabilityHandler) -> Self {
        let mut handlers = HashMap::new();
        handlers.insert(name.to_string(), Some(handler));
        Self { handlers }
    }

    /// Internal constructor accepting a metadata-only name.
    pub fn for_test_with_metadata_only_arc(name: &str) -> Self {
        let mut handlers = HashMap::new();
        handlers.insert(name.to_string(), None);
        Self { handlers }
    }

    /// Register a metadata-only capability: the name is in the
    /// catalog but no local handler is bound. T067.1 / T067.2
    /// use this to assert that the dispatcher refuses to
    /// execute the call.
    pub fn for_test_with_metadata_only<I, S>(names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let handlers = names
            .into_iter()
            .map(Into::into)
            .map(|n| (n, None))
            .collect();
        Self { handlers }
    }

    /// Register both a metadata-only row and a separate
    /// handler-backed row. Used by T067.1 to prove the same
    /// capability is either metadata-only OR handler-backed,
    /// never both.
    pub fn for_test_with_mixed(entries: &[(&'static str, Option<CapabilityHandler>)]) -> Self {
        let mut handlers = HashMap::new();
        for (name, handler) in entries {
            handlers.insert((*name).to_string(), handler.clone());
        }
        Self { handlers }
    }

    /// Returns `true` if the name is in the catalog (metadata-only
    /// OR handler-backed).
    pub fn contains(&self, name: &str) -> bool {
        self.handlers.contains_key(name)
    }

    /// Returns `true` if a callable handler is registered for the
    /// given name. `false` for metadata-only rows.
    pub fn has_handler(&self, name: &str) -> bool {
        matches!(self.handlers.get(name), Some(Some(_)))
    }

    /// Borrow the handler for the given name, when present.
    pub fn handler(&self, name: &str) -> Option<CapabilityHandler> {
        self.handlers.get(name).and_then(|slot| slot.clone())
    }

    /// Total number of registered capabilities (handler-backed and
    /// metadata-only combined).
    pub fn len(&self) -> usize {
        self.handlers.len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.handlers.is_empty()
    }
}

/// Outcome of a single capability dispatch. The dispatcher writes
/// one audit event per call to its `AuditSink`; the event is
/// always delivered, including on parse failure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DispatchAuditEvent {
    capability: String,
    outcome: String,
    handler_invoked: bool,
    correlation_id: String,
    message: String,
}

impl DispatchAuditEvent {
    /// Build a new audit event. `capability` and `correlation_id`
    /// are normalised by the dispatcher; `message` is redacted of
    /// any `Authorization` / `Bearer` / token-shaped material.
    pub(crate) fn new(
        capability: impl Into<String>,
        outcome: impl Into<String>,
        handler_invoked: bool,
        correlation_id: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            capability: capability.into(),
            outcome: outcome.into(),
            handler_invoked,
            correlation_id: correlation_id.into(),
            message: redact_secrets(message.into().as_str()),
        }
    }

    /// Borrow the capability name (or the literal `"<parse-error>"`
    /// when the envelope could not be parsed).
    pub fn capability(&self) -> &str {
        &self.capability
    }

    /// Whether the adapter invoked a handler. `false` for
    /// `MetadataOnly` / `HandlerMissing` / `ArgumentShape`.
    pub fn handler_invoked(&self) -> bool {
        self.handler_invoked
    }

    /// Stable outcome label (e.g. `"ok"`, `"metadata_only"`,
    /// `"handler_error"`).
    pub fn outcome(&self) -> &str {
        &self.outcome
    }

    /// Redacted message safe to forward to the audit log and the
    /// UI. The raw cause NEVER crosses this boundary.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Borrow the correlation id. Empty when the caller did not
    /// supply one in the envelope.
    pub fn correlation_id(&self) -> &str {
        &self.correlation_id
    }
}

/// Typed error returned by [`CapabilityAdapter::dispatch_typed`]
/// and by [`crate::runtime::DesktopHostDispatcher::dispatch_typed`].
/// The wire code is stable; consumers MUST switch on
/// [`AdapterErrorCode`] and not on the numeric value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityDispatchError {
    code: AdapterErrorCode,
    capability: String,
    message: String,
}

impl CapabilityDispatchError {
    /// Construct a typed dispatch error.
    pub fn new(
        code: AdapterErrorCode,
        capability: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            capability: capability.into(),
            message: redact_secrets(message.into().as_str()),
        }
    }

    /// Stable wire code emitted on the JSON envelope.
    pub fn wire_code(&self) -> u16 {
        self.code.wire()
    }

    /// Stable typed code. Consumers MUST switch on this enum
    /// instead of the numeric wire code.
    pub fn code(&self) -> AdapterErrorCode {
        self.code
    }

    /// Borrow the capability name (or the literal `"<parse-error>"`
    /// when the envelope could not be parsed).
    pub fn capability(&self) -> &str {
        &self.capability
    }

    /// Borrow the redacted human-readable message. The raw cause
    /// is never exposed; tokens / Authorization headers are
    /// replaced with `<redacted>`.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for CapabilityDispatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "capability dispatch error: code={:?} capability={} message={}",
            self.code, self.capability, self.message
        )
    }
}

impl std::error::Error for CapabilityDispatchError {}

/// Sink for capability-dispatch audit events. The dispatcher
/// writes exactly one event per call, including on parse
/// failure. Implementations MUST be `Send + Sync` because the
/// dispatcher is called from async contexts.
pub trait AuditSink: Send + Sync {
    /// Persist a single audit event.
    fn record_event(&self, event: DispatchAuditEvent);
}

/// Default audit sink: drops every event. Production code wires
/// a real sink into the activity log; tests use a
/// `CollectingSink` to inspect the events.
#[derive(Debug, Default)]
pub struct NullAuditSink;

impl AuditSink for NullAuditSink {
    fn record_event(&self, _event: DispatchAuditEvent) {}
}

/// The capability dispatch adapter. The adapter owns the
/// [`HandlerRegistry`] and the [`AuditSink`] and provides the
/// `dispatch` / `dispatch_typed` entry points used by the
/// desktop host dispatcher.
pub struct CapabilityAdapter {
    registry: HandlerRegistry,
    audit_sink: Arc<dyn AuditSink>,
}

impl std::fmt::Debug for CapabilityAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CapabilityAdapter")
            .field("registry", &self.registry)
            .finish()
    }
}

impl CapabilityAdapter {
    /// Build a new adapter from the given registry, using the
    /// [`NullAuditSink`] for audit delivery.
    pub fn new(registry: HandlerRegistry) -> Self {
        Self {
            registry,
            audit_sink: Arc::new(NullAuditSink),
        }
    }

    /// Build a new adapter from the given registry + audit sink.
    pub fn with_audit_sink(registry: HandlerRegistry, audit_sink: Arc<dyn AuditSink>) -> Self {
        Self {
            registry,
            audit_sink,
        }
    }

    /// Borrow the underlying registry.
    pub fn registry(&self) -> &HandlerRegistry {
        &self.registry
    }

    /// Run the dispatch pipeline against the adapter directly.
    /// The desktop host dispatcher delegates to this method.
    /// The order is fixed: parse → unknown → unauthorized →
    /// handler-existence → argument-shape → handler. Each step
    /// emits exactly one audit event and returns the
    /// corresponding [`CapabilityDispatchError`].
    pub async fn dispatch(
        &self,
        envelope: &str,
        allowed_capabilities: &[String],
    ) -> Result<DispatchOutcome, CapabilityDispatchError> {
        let parsed = match serde_json::from_str::<CallEnvelope>(envelope) {
            Ok(call) => call,
            Err(err) => {
                let error = CapabilityDispatchError::new(
                    AdapterErrorCode::InvalidEnvelope,
                    "<parse-error>",
                    format!("invalid host_call envelope: {err}"),
                );
                self.audit_sink.record_event(DispatchAuditEvent::new(
                    "<parse-error>",
                    "invalid_envelope",
                    false,
                    "",
                    format!("envelope parse error: {err}"),
                ));
                return Err(error);
            }
        };

        let capability_name = parsed.capability;
        let correlation_id = parsed.correlation_id.unwrap_or_default();
        let args = parsed.args;

        // Step 1: unknown
        if !self.registry.contains(&capability_name) {
            let error = CapabilityDispatchError::new(
                AdapterErrorCode::Unknown,
                capability_name.clone(),
                format!("unknown capability: {capability_name}"),
            );
            self.audit_sink.record_event(DispatchAuditEvent::new(
                capability_name.clone(),
                "unknown",
                false,
                correlation_id.clone(),
                format!("unknown capability: {capability_name}"),
            ));
            return Err(error);
        }

        // Step 2: unauthorized
        if !allowed_capabilities.contains(&capability_name) {
            let allowed = if allowed_capabilities.is_empty() {
                "<none>".to_string()
            } else {
                allowed_capabilities.join(", ")
            };
            let message = format!("未授权 {capability_name}；当前已选 Capability：{allowed}");
            let error = CapabilityDispatchError::new(
                AdapterErrorCode::Unauthorized,
                capability_name.clone(),
                message.clone(),
            );
            self.audit_sink.record_event(DispatchAuditEvent::new(
                capability_name.clone(),
                "unauthorized",
                false,
                correlation_id.clone(),
                message,
            ));
            return Err(error);
        }

        // Step 3: handler-existence (metadata-only rows surface
        // `MetadataOnly` — same wire code as the typed enum)
        let handler = match self.registry.handler(&capability_name) {
            Some(h) => h,
            None => {
                let error = CapabilityDispatchError::new(
                    AdapterErrorCode::MetadataOnly,
                    capability_name.clone(),
                    format!(
                        "capability {} is registered as metadata only; no local handler",
                        capability_name
                    ),
                );
                self.audit_sink.record_event(DispatchAuditEvent::new(
                    capability_name.clone(),
                    "metadata_only",
                    false,
                    correlation_id.clone(),
                    format!(
                        "capability {} has no local handler; metadata-only rejection",
                        capability_name
                    ),
                ));
                return Err(error);
            }
        };

        // Step 4: argument-shape (the registry refuses non-object
        // args so the handler always receives a JSON value it can
        // inspect safely).
        if !args.is_object() {
            let error = CapabilityDispatchError::new(
                AdapterErrorCode::ArgumentShape,
                capability_name.clone(),
                format!(
                    "arguments for {} must be a JSON object; got {}",
                    capability_name,
                    value_kind(&args)
                ),
            );
            self.audit_sink.record_event(DispatchAuditEvent::new(
                capability_name.clone(),
                "argument_shape",
                false,
                correlation_id.clone(),
                format!("argument shape for {} is invalid", capability_name),
            ));
            return Err(error);
        }

        // Step 5: handler invocation
        let result = handler(&args);
        match result {
            Ok(data) => {
                let args_repr = redact_secrets(&args.to_string());
                self.audit_sink.record_event(DispatchAuditEvent::new(
                    capability_name.clone(),
                    "ok",
                    true,
                    correlation_id.clone(),
                    format!("handler for {capability_name} succeeded; args={args_repr}"),
                ));
                Ok(DispatchOutcome {
                    data,
                    correlation_id,
                })
            }
            Err(message) => {
                let redacted = redact_secrets(&message);
                let args_repr = redact_secrets(&args.to_string());
                self.audit_sink.record_event(DispatchAuditEvent::new(
                    capability_name.clone(),
                    "handler_error",
                    true,
                    correlation_id.clone(),
                    format!(
                        "handler for {capability_name} returned error: {redacted}; args={args_repr}"
                    ),
                ));
                Err(CapabilityDispatchError::new(
                    AdapterErrorCode::HandlerError,
                    capability_name.clone(),
                    format!("handler for {capability_name} returned error: {redacted}"),
                ))
            }
        }
    }
}

/// Successful result of a [`CapabilityAdapter::dispatch`] call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchOutcome {
    data: Value,
    correlation_id: String,
}

impl DispatchOutcome {
    /// Borrow the JSON data returned by the handler.
    pub fn data(&self) -> &Value {
        &self.data
    }

    /// Borrow the correlation id forwarded to the audit event.
    pub fn correlation_id(&self) -> &str {
        &self.correlation_id
    }
}

#[derive(Debug, Deserialize)]
struct CallEnvelope {
    capability: String,
    #[serde(default)]
    args: Value,
    #[serde(default)]
    correlation_id: Option<String>,
}

fn value_kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Redact token-shaped material from an audit message. The
/// redaction is intentionally simple and conservative: anything
/// that looks like an `Authorization: Bearer …` header, a raw
/// Bearer token, or a `key=` / `token=` / `api_key=` value is
/// replaced with `<redacted>`. The function is best-effort —
/// it is the *boundary*'s responsibility, not the caller's.
pub(crate) fn redact_secrets(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(idx) = rest.find("Authorization") {
        out.push_str("<redacted-Authorization:");
        out.push_str(&rest[..idx + "Authorization".len()]);
        out.push('>');
        rest = &rest[idx + "Authorization".len()..];
    }
    if !out.is_empty() {
        // We found an Authorization; replace the rest of the
        // current line wholesale with `<redacted>` so any
        // surrounding "Bearer …" or token literal on the same
        // envelope does not leak.
        if let Some(newline) = rest.find('\n') {
            out.push_str("<redacted>");
            out.push_str(&rest[newline..]);
        } else {
            out.push_str("<redacted>");
        }
        return out;
    }

    // No Authorization reference: redact raw Bearer tokens and
    // key=value pairs.
    let mut buffer = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c == 'B' {
            let mut probe = String::from('B');
            for _ in 0..6 {
                if let Some(&nc) = chars.peek() {
                    probe.push(nc);
                    chars.next();
                } else {
                    break;
                }
            }
            if probe.eq_ignore_ascii_case("Bearer ") {
                buffer.push_str("<redacted>");
                for nc in chars.by_ref() {
                    if nc == '\n' {
                        buffer.push(nc);
                        break;
                    }
                }
                continue;
            }
            buffer.push_str(&probe);
        } else if c.is_ascii_alphabetic() {
            // Detect `key`, `token`, `api_key`, `apiKey`, etc.
            let mut probe = String::from(c);
            while let Some(&nc) = chars.peek() {
                if nc.is_ascii_alphabetic() || nc == '_' {
                    probe.push(nc);
                    chars.next();
                } else {
                    break;
                }
            }
            let lower = probe.to_ascii_lowercase();
            let is_secret_key = matches!(
                lower.as_str(),
                "token" | "key" | "api_key" | "apikey" | "password" | "secret"
            );
            if is_secret_key {
                // Skip optional whitespace + `=` + quote.
                let mut skipped_ws = String::new();
                while let Some(&nc) = chars.peek() {
                    if nc.is_whitespace() {
                        skipped_ws.push(nc);
                        chars.next();
                    } else {
                        break;
                    }
                }
                if let Some(&nc) = chars.peek() {
                    if nc == '=' || nc == ':' {
                        chars.next();
                        // Skip whitespace after the separator.
                        while let Some(&nc) = chars.peek() {
                            if nc.is_whitespace() {
                                chars.next();
                            } else {
                                break;
                            }
                        }
                        // Skip an opening quote if present.
                        if let Some(&nc) = chars.peek() {
                            if nc == '"' || nc == '\'' {
                                chars.next();
                            }
                        }
                        buffer.push_str(&probe);
                        buffer.push_str(&skipped_ws);
                        if let Some(&nc) = chars.peek() {
                            buffer.push(nc);
                        }
                        buffer.push_str("<redacted>");
                        // Consume the value up to the next separator.
                        for nc in chars.by_ref() {
                            if nc == ',' || nc == ';' || nc == '}' || nc == ']' || nc == '\n' {
                                buffer.push(nc);
                                break;
                            }
                        }
                        continue;
                    }
                }
            }
            buffer.push_str(&probe);
        } else {
            buffer.push(c);
        }
    }
    buffer
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct CollectingSink {
        events: Mutex<Vec<DispatchAuditEvent>>,
    }

    impl AuditSink for CollectingSink {
        fn record_event(&self, event: DispatchAuditEvent) {
            self.events.lock().expect("audit lock").push(event);
        }
    }

    #[test]
    fn adapter_error_code_wire_is_stable() {
        assert_eq!(AdapterErrorCode::InvalidEnvelope.wire(), 4001);
        assert_eq!(AdapterErrorCode::Unknown.wire(), 4045);
        assert_eq!(AdapterErrorCode::Unauthorized.wire(), 4030);
        assert_eq!(AdapterErrorCode::MetadataOnly.wire(), 4050);
        assert_eq!(AdapterErrorCode::ArgumentShape.wire(), 4010);
        assert_eq!(AdapterErrorCode::HandlerError.wire(), 4020);
    }

    #[test]
    fn adapter_error_code_from_wire_round_trips() {
        assert_eq!(
            AdapterErrorCode::from_wire(4001),
            AdapterErrorCode::InvalidEnvelope
        );
        assert_eq!(AdapterErrorCode::from_wire(4045), AdapterErrorCode::Unknown);
        assert_eq!(
            AdapterErrorCode::from_wire(4030),
            AdapterErrorCode::Unauthorized
        );
        assert_eq!(
            AdapterErrorCode::from_wire(4050),
            AdapterErrorCode::MetadataOnly
        );
        assert_eq!(
            AdapterErrorCode::from_wire(4010),
            AdapterErrorCode::ArgumentShape
        );
        assert_eq!(
            AdapterErrorCode::from_wire(4020),
            AdapterErrorCode::HandlerError
        );
    }

    #[test]
    fn redact_secrets_replaces_authorization_header() {
        let redacted = redact_secrets("Authorization: Bearer super-secret-token");
        assert!(!redacted.contains("super-secret-token"));
        assert!(redacted.contains("<redacted>") || redacted.contains("Authorization"));
    }

    #[test]
    fn redact_secrets_replaces_bearer_token() {
        let redacted = redact_secrets("error: Bearer abcdef1234567890");
        assert!(!redacted.contains("abcdef1234567890"));
    }

    #[test]
    fn registry_metadata_only_does_not_carry_handler() {
        let reg = HandlerRegistry::for_test_with_metadata_only(["db.execute"]);
        assert!(reg.contains("db.execute"));
        assert!(!reg.has_handler("db.execute"));
    }

    #[test]
    fn registry_handler_carries_callable() {
        let reg = HandlerRegistry::for_test_with_handler("time.now", |_| {
            Ok(serde_json::json!({"unix": 0}))
        });
        assert!(reg.has_handler("time.now"));
        let handler = reg.handler("time.now").expect("handler");
        let outcome = handler(&serde_json::json!({})).expect("handler returns ok");
        assert_eq!(outcome["unix"], 0);
    }

    #[tokio::test]
    async fn adapter_returns_metadata_only_without_invoking_handler() {
        let reg = HandlerRegistry::for_test_with_metadata_only(["db.execute"]);
        let sink = Arc::new(CollectingSink::default());
        let adapter = CapabilityAdapter::with_audit_sink(reg, sink.clone());
        let err = adapter
            .dispatch(
                r#"{"capability":"db.execute","args":{"query":"select 1"}}"#,
                &["db.execute".to_string()],
            )
            .await
            .expect_err("metadata-only must error");
        assert_eq!(err.code(), AdapterErrorCode::MetadataOnly);
        let events = sink.events.lock().expect("lock").clone();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].outcome(), "metadata_only");
        assert!(!events[0].handler_invoked());
    }

    #[tokio::test]
    async fn adapter_invokes_handler_and_records_ok() {
        let reg = HandlerRegistry::for_test_with_handler("time.now", |_| {
            Ok(serde_json::json!({"unix": 1_700_000_000_i64}))
        });
        let sink = Arc::new(CollectingSink::default());
        let adapter = CapabilityAdapter::with_audit_sink(reg, sink.clone());
        let outcome = adapter
            .dispatch(
                r#"{"capability":"time.now","args":{}}"#,
                &["time.now".to_string()],
            )
            .await
            .expect("ok");
        assert_eq!(outcome.data()["unix"], 1_700_000_000_i64);
        let events = sink.events.lock().expect("lock").clone();
        assert_eq!(events.len(), 1);
        assert!(events[0].handler_invoked());
        assert_eq!(events[0].outcome(), "ok");
    }
}
