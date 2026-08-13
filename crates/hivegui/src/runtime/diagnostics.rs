//! Runtime diagnostics public boundary (T027).
//!
//! This module defines the typed boundary between runtime adapters
//! (provider, agent runner, tool, workflow, plugin, capability) and the
//! diagnostic sink. The invariants are:
//!
//! 1. The internal `RuntimeFailure` carries a `cause` chain that may
//!    contain sensitive material (provider frames, raw cause strings).
//! 2. Adapters may *enrich* the chain (via `with_context`) but must
//!    never record the failure on their own — the failure is only
//!    recorded when the *handling boundary* calls
//!    [`RuntimeErrorBoundary::handle`].
//! 3. The public [`RuntimeErrorBoundary::handle`] returns a
//!    [`PublicError`] that strips the raw `cause` from the message
//!    visible to the UI or higher layers.
//! 4. The sink records exactly one [`DiagnosticRecord`] per handled
//!    failure, even when multiple adapters observed the same chain.

#![warn(missing_docs)]

use super::capability_adapter::redact_secrets;
use std::sync::{Arc, Mutex};

/// A raw failure observed by an internal adapter. May contain
/// sensitive material in `cause`; only the handling boundary is
/// allowed to derive a redacted record from it.
#[derive(Debug, Clone)]
pub struct RuntimeFailure {
    /// Stable identifier of the execution that produced the failure.
    execution_id: String,
    /// Stable kind of the failure (`"internal"`, `"io"`, `"cancelled"`,
    /// `"function_not_executable"`, …).
    kind: String,
    /// Raw cause text. May contain sensitive data — never expose.
    cause: String,
    /// Context labels contributed by adapters, in observation order.
    context: Vec<String>,
}

impl RuntimeFailure {
    /// Construct an `internal` failure carrying the raw `cause` text.
    pub fn internal(execution_id: impl Into<String>, cause: impl Into<String>) -> Self {
        Self {
            execution_id: execution_id.into(),
            kind: "internal".to_string(),
            cause: cause.into(),
            context: Vec::new(),
        }
    }

    /// Construct a `function_not_executable` failure.
    pub fn function_not_executable(
        execution_id: impl Into<String>,
        cause: impl Into<String>,
    ) -> Self {
        Self {
            execution_id: execution_id.into(),
            kind: "function_not_executable".to_string(),
            cause: cause.into(),
            context: Vec::new(),
        }
    }

    /// Construct an `io` failure.
    pub fn io(execution_id: impl Into<String>, cause: impl Into<String>) -> Self {
        Self {
            execution_id: execution_id.into(),
            kind: "io".to_string(),
            cause: cause.into(),
            context: Vec::new(),
        }
    }

    /// Construct a `cancelled` failure.
    pub fn cancelled(execution_id: impl Into<String>, cause: impl Into<String>) -> Self {
        Self {
            execution_id: execution_id.into(),
            kind: "cancelled".to_string(),
            cause: cause.into(),
            context: Vec::new(),
        }
    }

    /// Borrow the execution id.
    pub fn execution_id(&self) -> &str {
        &self.execution_id
    }

    /// Borrow the stable kind label.
    pub fn kind(&self) -> &str {
        &self.kind
    }

    /// Borrow the raw cause text. Only the handling boundary may
    /// surface this; adapters and UI must use the redacted
    /// [`PublicError::message`].
    pub fn cause(&self) -> &str {
        &self.cause
    }

    /// Borrow the chain of adapter context labels in observation order.
    pub fn context(&self) -> &[String] {
        &self.context
    }

    /// Append an adapter context label. Adapters may call this freely;
    /// it never produces a diagnostic record.
    pub fn with_context(mut self, label: impl Into<String>) -> Self {
        self.context.push(label.into());
        self
    }
}

/// A diagnostic record produced by the handling boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticRecord {
    execution_id: String,
    operation: String,
    error_kind: String,
    /// Redacted cause. The handling boundary strips the raw `cause`
    /// before passing it to the sink; only context labels and a
    /// sanitised summary of the cause are kept.
    redacted_cause: String,
    context: Vec<String>,
}

impl DiagnosticRecord {
    /// Construct a new record. The handling boundary is the only
    /// caller allowed to construct a record.
    pub fn new(
        execution_id: impl Into<String>,
        operation: impl Into<String>,
        error_kind: impl Into<String>,
        redacted_cause: impl Into<String>,
        context: Vec<String>,
    ) -> Self {
        Self {
            execution_id: execution_id.into(),
            operation: operation.into(),
            error_kind: error_kind.into(),
            redacted_cause: redacted_cause.into(),
            context,
        }
    }

    /// Borrow the execution id.
    pub fn execution_id(&self) -> &str {
        &self.execution_id
    }

    /// Borrow the operation label.
    pub fn operation(&self) -> &str {
        &self.operation
    }

    /// Borrow the stable error kind.
    pub fn error_kind(&self) -> &str {
        &self.error_kind
    }

    /// Borrow the redacted cause.
    pub fn cause(&self) -> &str {
        &self.redacted_cause
    }

    /// Borrow the chain of adapter context labels.
    pub fn context(&self) -> &[String] {
        &self.context
    }
}

/// Sink consumed by the handling boundary. Implementations persist the
/// record (e.g. into the activity log).
pub trait DiagnosticSink: Send + Sync {
    /// Persist a single diagnostic record. Called exactly once per
    /// handled failure.
    fn record(&self, diagnostic: DiagnosticRecord);
}

/// A no-op sink used by tests that do not need to observe records.
#[derive(Debug, Default, Clone)]
pub struct NullDiagnosticSink;

impl DiagnosticSink for NullDiagnosticSink {
    fn record(&self, _diagnostic: DiagnosticRecord) {}
}

/// The handling boundary. Adapters add context but never call
/// `record`; only this type does.
pub struct RuntimeErrorBoundary {
    sink: Arc<dyn DiagnosticSink>,
    handled: Mutex<std::collections::HashSet<String>>,
}

impl RuntimeErrorBoundary {
    /// Build a new boundary writing into `sink`.
    pub fn new(sink: Arc<dyn DiagnosticSink>) -> Self {
        Self {
            sink,
            handled: Mutex::new(std::collections::HashSet::new()),
        }
    }

    /// Handle a failure. Produces a redacted [`PublicError`] for the
    /// UI and records exactly one diagnostic record per unique
    /// failure chain. Repeated calls with the same cause/context pair
    /// do not produce additional records.
    pub fn handle(&self, operation: &str, failure: RuntimeFailure) -> PublicError {
        let dedup_key = format!(
            "{}|{}|{}|{:?}",
            failure.execution_id(),
            failure.kind(),
            failure.cause(),
            failure.context()
        );
        let already = {
            let mut handled = self.handled.lock().expect("diagnostic handled lock");
            handled.insert(dedup_key)
        };
        if !already {
            self.sink.record(DiagnosticRecord::new(
                failure.execution_id(),
                operation,
                failure.kind(),
                redact_cause(failure.cause()),
                failure.context().to_vec(),
            ));
        }
        PublicError {
            execution_id: failure.execution_id().to_string(),
            kind: failure.kind().to_string(),
            message: public_message(failure.kind()),
        }
    }
}

/// Public, redacted error returned by the handling boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicError {
    execution_id: String,
    kind: String,
    message: String,
}

impl PublicError {
    /// Borrow the execution id.
    pub fn execution_id(&self) -> &str {
        &self.execution_id
    }

    /// Borrow the stable error kind.
    pub fn kind(&self) -> &str {
        &self.kind
    }

    /// Borrow the redacted public message. Never contains the raw
    /// cause.
    pub fn message(&self) -> &str {
        &self.message
    }
}

/// Maximum allowed length of a redacted cause. Any cause longer
/// than this is truncated at a UTF-8 character boundary before
/// being persisted, so the diagnostic record's `cause` field is
/// guaranteed to fit in the 512-byte v1 contract.
pub const CAUSE_SUMMARY_MAX_BYTES: usize = 512;

fn redact_cause(cause: &str) -> String {
    // First pass: redact `Bearer <token>` patterns wherever they
    // appear (including inside `Authorization=Bearer <token>`).
    let mut out = redact_bearer_tokens(cause);
    // Second pass: redact `key=value` pairs.
    out = redact_key_value_pairs(&out);
    // Third pass: redact `<…>` angle-bracketed tokens.
    out = redact_angle_tokens(&out);
    // Finally, cap to CAUSE_SUMMARY_MAX_BYTES at a UTF-8 boundary.
    if out.len() > CAUSE_SUMMARY_MAX_BYTES {
        let mut cut = CAUSE_SUMMARY_MAX_BYTES;
        while cut > 0 && !out.is_char_boundary(cut) {
            cut -= 1;
        }
        out.truncate(cut);
    }
    out
}

fn redact_bearer_tokens(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < bytes.len() {
        if matches_bearer_at(bytes, i) {
            out.push_str("Bearer");
            i += 6;
            // Copy leading whitespace through.
            while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
                out.push(bytes[i] as char);
                i += 1;
            }
            // Redact the token value.
            out.push_str("<redacted>");
            while i < bytes.len() && !is_value_terminator(bytes[i]) {
                i += 1;
            }
            continue;
        }
        // Copy the current char (preserving UTF-8) from input.
        let ch_end = next_char_boundary(bytes, i);
        out.push_str(&input[i..ch_end]);
        i = ch_end;
    }
    out
}

fn redact_key_value_pairs(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'=' {
            out.push('=');
            i += 1;
            // If the value already starts with `Bearer`, preserve
            // the literal and redact the token only.
            if matches_bearer_at(bytes, i) {
                out.push_str("Bearer");
                i += 6;
                while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
                    out.push(bytes[i] as char);
                    i += 1;
                }
            }
            out.push_str("<redacted>");
            while i < bytes.len() && !is_value_terminator(bytes[i]) {
                i += 1;
            }
            continue;
        }
        let ch_end = next_char_boundary(bytes, i);
        out.push_str(&input[i..ch_end]);
        i = ch_end;
    }
    out
}

fn redact_angle_tokens(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'<' {
            if let Some(end) = find_angle_close(&bytes[i + 1..]) {
                out.push_str("<redacted>");
                i = i + 1 + end + 1;
                continue;
            }
        }
        let ch_end = next_char_boundary(bytes, i);
        out.push_str(&input[i..ch_end]);
        i = ch_end;
    }
    out
}

fn next_char_boundary(bytes: &[u8], i: usize) -> usize {
    let mut j = i + 1;
    while j < bytes.len() && (bytes[j] & 0xC0) == 0x80 {
        j += 1;
    }
    j
}

fn matches_bearer_at(bytes: &[u8], i: usize) -> bool {
    if i + 6 > bytes.len() {
        return false;
    }
    if !bytes[i..i + 6].eq_ignore_ascii_case(b"bearer") {
        return false;
    }
    let after = i + 6;
    after == bytes.len() || bytes[after] == b' ' || bytes[after] == b'\t'
}

fn is_value_terminator(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | b',' | b';')
}

fn find_angle_close(rest: &[u8]) -> Option<usize> {
    for (offset, byte) in rest.iter().enumerate() {
        if *byte == b'>' {
            return Some(offset);
        }
        if *byte == b'<' {
            return None;
        }
    }
    None
}

fn public_message(kind: &str) -> String {
    match kind {
        "internal" => "internal runtime error".to_string(),
        "function_not_executable" => "function is not executable".to_string(),
        "io" => "i/o error".to_string(),
        "cancelled" => "operation cancelled".to_string(),
        other => format!("{other} error"),
    }
}

// =========================================================================
// T131 [US13] Diagnostic bundle
// =========================================================================

/// One collected event for a specific execution id. The
/// `category` is the agent / llm / tool / workflow / plugin /
/// capability taxonomy, `event` is the typed event name, and
/// `summary` is a short user-facing description. Sensitive
/// fields (prompt, token, conversation, tool payload, device
/// key, backup passphrase) MUST NEVER be stored in `summary`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionEvent {
    category: String,
    event: String,
    summary: String,
    timestamp_unix_ms: u64,
}

impl ExecutionEvent {
    /// Build a new event. Callers pass a stable wire `category`
    /// and `event` so the bundle and UI can group by them.
    pub fn new(
        category: impl Into<String>,
        event: impl Into<String>,
        summary: impl Into<String>,
    ) -> Self {
        Self {
            category: category.into(),
            event: event.into(),
            summary: summary.into(),
            timestamp_unix_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0),
        }
    }

    /// Category wire string.
    pub fn category(&self) -> &str {
        &self.category
    }

    /// Event wire string.
    pub fn event(&self) -> &str {
        &self.event
    }

    /// Short summary.
    pub fn summary(&self) -> &str {
        &self.summary
    }

    /// Unix millisecond timestamp.
    pub fn timestamp_unix_ms(&self) -> u64 {
        self.timestamp_unix_ms
    }
}

/// Thread-safe per-execution-id event collector. Used by the
/// Agent / LLM / Tool / Workflow / Plugin / Capability layers to
/// record typed events; the [`DiagnosticBundle`] reads the
/// collector and produces a redacted JSON bundle.
#[derive(Debug, Default)]
pub struct ExecutionEventCollector {
    events: std::sync::Mutex<Vec<(String, ExecutionEvent)>>,
}

impl ExecutionEventCollector {
    /// Build a new empty collector.
    pub fn new() -> Self {
        Self::default()
    }

    fn push(&self, execution_id: impl Into<String>, event: ExecutionEvent) {
        let mut events = self.events.lock().expect("events lock poisoned");
        events.push((execution_id.into(), event));
    }

    /// Record an Agent-layer event.
    pub fn record_agent_event(
        &self,
        execution_id: impl Into<String>,
        event: impl Into<String>,
        summary: impl Into<String>,
    ) {
        self.push(execution_id, ExecutionEvent::new("agent", event, summary));
    }

    /// Record an LLM-layer event.
    pub fn record_llm_event(
        &self,
        execution_id: impl Into<String>,
        event: impl Into<String>,
        summary: impl Into<String>,
    ) {
        self.push(execution_id, ExecutionEvent::new("llm", event, summary));
    }

    /// Record a Tool-layer event.
    pub fn record_tool_event(
        &self,
        execution_id: impl Into<String>,
        event: impl Into<String>,
        summary: impl Into<String>,
    ) {
        self.push(execution_id, ExecutionEvent::new("tool", event, summary));
    }

    /// Record a Workflow-layer event.
    pub fn record_workflow_event(
        &self,
        execution_id: impl Into<String>,
        event: impl Into<String>,
        summary: impl Into<String>,
    ) {
        self.push(
            execution_id,
            ExecutionEvent::new("workflow", event, summary),
        );
    }

    /// Record a Plugin-layer event.
    pub fn record_plugin_event(
        &self,
        execution_id: impl Into<String>,
        event: impl Into<String>,
        summary: impl Into<String>,
    ) {
        self.push(execution_id, ExecutionEvent::new("plugin", event, summary));
    }

    /// Record a Capability-layer event.
    pub fn record_capability_event(
        &self,
        execution_id: impl Into<String>,
        event: impl Into<String>,
        summary: impl Into<String>,
    ) {
        self.push(
            execution_id,
            ExecutionEvent::new("capability", event, summary),
        );
    }

    /// All events for one execution id, ordered by insertion.
    pub fn for_execution(&self, execution_id: &str) -> Vec<ExecutionEvent> {
        let events = self.events.lock().expect("events lock poisoned");
        events
            .iter()
            .filter_map(|(id, event)| {
                if id == execution_id {
                    Some(event.clone())
                } else {
                    None
                }
            })
            .collect()
    }

    /// All known execution ids.
    pub fn execution_ids(&self) -> Vec<String> {
        let events = self.events.lock().expect("events lock poisoned");
        let mut ids: Vec<String> = events.iter().map(|(id, _)| id.clone()).collect();
        ids.sort();
        ids.dedup();
        ids
    }
}

/// Redaction policy. Every field defaults to `true` (redact).
#[derive(Debug, Clone)]
pub struct RedactionConfig {
    /// Redact prompt content.
    pub redact_prompt: bool,
    /// Redact provider tokens.
    pub redact_token: bool,
    /// Redact tool payloads.
    pub redact_tool_payload: bool,
    /// Redact conversation messages.
    pub redact_conversation: bool,
    /// Redact device key material.
    pub redact_device_key: bool,
    /// Redact backup passphrases.
    pub redact_backup_passphrase: bool,
}

impl Default for RedactionConfig {
    fn default() -> Self {
        Self {
            redact_prompt: true,
            redact_token: true,
            redact_tool_payload: true,
            redact_conversation: true,
            redact_device_key: true,
            redact_backup_passphrase: true,
        }
    }
}

/// Result of a successful redacted bundle export. Holds the
/// final on-disk path.
#[derive(Debug, Clone)]
pub struct RedactedBundle {
    path: std::path::PathBuf,
    events: Vec<(String, ExecutionEvent)>,
}

impl RedactedBundle {
    /// Path the bundle was written to.
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// Number of events included in the bundle.
    pub fn event_count(&self) -> usize {
        self.events.len()
    }
}

/// Diagnostic bundle exporter. Reads from an
/// [`ExecutionEventCollector`] and writes a redacted JSON file
/// to the requested target path. The exporter never writes
/// prompt, token, conversation, tool payload, device key, or
/// backup passphrase material — the [`RedactionConfig`] is
/// the only source of those decisions.
pub struct DiagnosticBundle {
    collector: Arc<ExecutionEventCollector>,
}

impl DiagnosticBundle {
    /// Build a new bundle for the supplied collector.
    pub fn new(collector: Arc<ExecutionEventCollector>) -> Self {
        Self { collector }
    }

    /// Underlying collector.
    pub fn collector(&self) -> &Arc<ExecutionEventCollector> {
        &self.collector
    }

    /// Export the bundle to `target`. The target must not exist;
    /// the exporter creates it fresh so an existing sensitive
    /// file cannot be aliased.
    pub fn export_redacted(
        &self,
        target: &std::path::Path,
        config: &RedactionConfig,
    ) -> Result<RedactedBundle, std::io::Error> {
        use std::io::Write;
        if target.exists() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("target already exists: {}", target.display()),
            ));
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut all_events: Vec<(String, ExecutionEvent)> = Vec::new();
        for execution_id in self.collector.execution_ids() {
            for event in self.collector.for_execution(&execution_id) {
                all_events.push((execution_id.clone(), event));
            }
        }
        let json = serde_json::json!({
            "schema_version": 1,
            "format": "hivegui-diagnostic-bundle/v1",
            "event_count": all_events.len(),
            "events": all_events.iter().map(|(id, event)| {
                let summary = redact_event_summary(event.summary(), config);
                serde_json::json!({
                    "execution_id": id,
                    "category": event.category(),
                    "event": event.event(),
                    "summary": summary,
                    "timestamp_unix_ms": event.timestamp_unix_ms(),
                })
            }).collect::<Vec<_>>(),
        });
        let body = serde_json::to_vec_pretty(&json)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(target)?;
        file.write_all(&body)?;
        file.sync_all()?;
        Ok(RedactedBundle {
            path: target.to_path_buf(),
            events: all_events,
        })
    }
}

fn redact_event_summary(summary: &str, config: &RedactionConfig) -> String {
    let should_redact = config.redact_prompt
        || config.redact_token
        || config.redact_tool_payload
        || config.redact_conversation
        || config.redact_device_key
        || config.redact_backup_passphrase;

    if should_redact {
        redact_secrets(summary)
    } else {
        summary.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Recording {
        records: Mutex<Vec<DiagnosticRecord>>,
    }

    impl DiagnosticSink for Recording {
        fn record(&self, diagnostic: DiagnosticRecord) {
            self.records.lock().expect("lock").push(diagnostic);
        }
    }

    #[test]
    fn one_failure_one_record() {
        let sink = Arc::new(Recording::default());
        let boundary = RuntimeErrorBoundary::new(sink.clone());
        let f = RuntimeFailure::internal("exec", "boom");
        let _ = boundary.handle("op", f);
        let _ = boundary.handle("op", RuntimeFailure::internal("exec", "boom"));
        assert_eq!(sink.records.lock().unwrap().len(), 1);
    }

    #[test]
    fn public_error_strips_cause() {
        let sink = Arc::new(Recording::default());
        let boundary = RuntimeErrorBoundary::new(sink.clone());
        let f = RuntimeFailure::internal("exec", "Authorization=Bearer abc");
        let p = boundary.handle("op", f);
        assert!(!p.message().contains("Authorization"));
        assert!(!p.message().contains("abc"));
    }

    #[test]
    fn export_redacted_applies_redaction_config() {
        let path = std::env::temp_dir().join(format!(
            "hivegui-diagnostic-redaction-test-{}.json",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let mut collector = ExecutionEventCollector::new();
        collector.record_llm_event("exec-1", "step", "conversation=hello token=Bearer secret123");

        let bundle = DiagnosticBundle::new(Arc::new(collector));
        let config = RedactionConfig {
            redact_prompt: true,
            redact_token: true,
            redact_tool_payload: true,
            redact_conversation: true,
            redact_device_key: true,
            redact_backup_passphrase: true,
        };

        bundle
            .export_redacted(&path, &config)
            .expect("export should succeed");

        let bytes = std::fs::read_to_string(&path).expect("bundle file should exist");
        let value: serde_json::Value =
            serde_json::from_str(&bytes).expect("bundle payload should be valid json");
        let summary = value["events"][0]["summary"].as_str().unwrap_or("");
        assert!(!summary.contains("secret123"));
        assert!(summary.contains("redacted") || summary.contains("conversation"));
    }
}
