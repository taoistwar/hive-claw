//! Runtime audit log — structured tracing plus best-effort DB persistence.
//!
//! All host_call / plugin_invoke / workflow_node / agent_route / llm_invoke /
//! llm_fallback / llm_local_fallback audit events are always emitted via tracing. A bounded,
//! process-global queue additionally persists sanitized records to
//! `runtime_audit_logs` when the writer is installed at service startup.
//! Hook execution uses the same structured tracing path but is explicitly
//! prevented from enqueueing a DB persistence copy.

use async_trait::async_trait;
use chrono::{NaiveDateTime, Utc};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use sqlx::{MySql, MySqlPool, QueryBuilder};
use std::sync::{
    Arc, LazyLock, OnceLock, RwLock,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use tokio::runtime::Handle;
use tokio::sync::mpsc;

use crate::models::RuntimeAuditLog;
use crate::runtime::execution_context::RuntimeExecutionContext;

pub const MAX_PAYLOAD_SUMMARY_BYTES: usize = 1024;
pub const DEFAULT_AUDIT_QUEUE_CAPACITY: usize = 1024;

/// 单条 audit 记录的参数集
#[derive(Debug, Clone)]
pub struct AuditRecord<'a> {
    pub agent_id: Option<i64>,
    pub plugin_id: Option<i64>,
    pub function_id: Option<i64>,
    pub capability: Option<&'a str>,
    /// e.g. capability_call / capability_denied / plugin_invoke / workflow_node /
    /// agent_route / llm_invoke / llm_fallback / llm_local_fallback
    pub event_type: &'a str,
    /// success / error / denied / timeout
    pub outcome: &'a str,
    pub elapsed_ms: Option<i32>,
    pub error_message: Option<&'a str>,
    pub payload_summary: Option<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct AuditMetricsSnapshot {
    pub enqueued: u64,
    pub persisted: u64,
    pub tracing_only: u64,
    pub dropped_queue_full: u64,
    pub dropped_writer_closed: u64,
    pub dropped_no_writer: u64,
    pub persist_failures: u64,
}

struct AuditMetrics {
    enqueued: AtomicU64,
    persisted: AtomicU64,
    tracing_only: AtomicU64,
    dropped_queue_full: AtomicU64,
    dropped_writer_closed: AtomicU64,
    dropped_no_writer: AtomicU64,
    persist_failures: AtomicU64,
    warned_queue_full: AtomicBool,
    warned_writer_closed: AtomicBool,
    warned_no_writer: AtomicBool,
    warned_persist_failure: AtomicBool,
}

impl AuditMetrics {
    fn new() -> Self {
        Self {
            enqueued: AtomicU64::new(0),
            persisted: AtomicU64::new(0),
            tracing_only: AtomicU64::new(0),
            dropped_queue_full: AtomicU64::new(0),
            dropped_writer_closed: AtomicU64::new(0),
            dropped_no_writer: AtomicU64::new(0),
            persist_failures: AtomicU64::new(0),
            warned_queue_full: AtomicBool::new(false),
            warned_writer_closed: AtomicBool::new(false),
            warned_no_writer: AtomicBool::new(false),
            warned_persist_failure: AtomicBool::new(false),
        }
    }

    fn snapshot(&self) -> AuditMetricsSnapshot {
        AuditMetricsSnapshot {
            enqueued: self.enqueued.load(Ordering::Relaxed),
            persisted: self.persisted.load(Ordering::Relaxed),
            tracing_only: self.tracing_only.load(Ordering::Relaxed),
            dropped_queue_full: self.dropped_queue_full.load(Ordering::Relaxed),
            dropped_writer_closed: self.dropped_writer_closed.load(Ordering::Relaxed),
            dropped_no_writer: self.dropped_no_writer.load(Ordering::Relaxed),
            persist_failures: self.persist_failures.load(Ordering::Relaxed),
        }
    }

    fn warn_once(flag: &AtomicBool, error_kind: &'static str, message: &'static str) {
        if !flag.swap(true, Ordering::Relaxed) {
            tracing::warn!(error_kind, "{message}");
        }
    }
}

static AUDIT_METRICS: LazyLock<Arc<AuditMetrics>> = LazyLock::new(|| Arc::new(AuditMetrics::new()));

pub fn metrics_snapshot() -> AuditMetricsSnapshot {
    AUDIT_METRICS.snapshot()
}

#[derive(Debug, Clone)]
struct OwnedAuditRecord {
    request_id: Option<String>,
    session_id: Option<i64>,
    agent_id: Option<i64>,
    plugin_id: Option<i64>,
    function_id: Option<i64>,
    capability: Option<String>,
    event_type: String,
    outcome: String,
    elapsed_ms: Option<i32>,
    error_message: Option<String>,
    payload_summary: Option<Value>,
    occurred_at: NaiveDateTime,
}

#[async_trait]
trait AuditSink: Send + Sync {
    async fn persist(&self, record: OwnedAuditRecord) -> Result<(), ()>;
}

struct MySqlAuditSink {
    pool: MySqlPool,
}

#[async_trait]
impl AuditSink for MySqlAuditSink {
    async fn persist(&self, record: OwnedAuditRecord) -> Result<(), ()> {
        sqlx::query(
            r#"INSERT INTO runtime_audit_logs
               (request_id, session_id, agent_id, plugin_id, function_id,
                capability, event_type, outcome, elapsed_ms, error_message,
                payload_summary, occurred_at)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
        )
        .bind(record.request_id)
        .bind(record.session_id)
        .bind(record.agent_id)
        .bind(record.plugin_id)
        .bind(record.function_id)
        .bind(record.capability)
        .bind(record.event_type)
        .bind(record.outcome)
        .bind(record.elapsed_ms)
        .bind(record.error_message)
        .bind(record.payload_summary)
        .bind(record.occurred_at)
        .execute(&self.pool)
        .await
        .map(|_| ())
        .map_err(|_| ())
    }
}

#[derive(Clone)]
struct AuditWriter {
    sender: mpsc::Sender<OwnedAuditRecord>,
    metrics: Arc<AuditMetrics>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AuditEnqueueError {
    Full,
    Closed,
}

impl AuditWriter {
    fn spawn(handle: &Handle, sink: Arc<dyn AuditSink>, capacity: usize) -> Self {
        Self::spawn_with_metrics(handle, sink, capacity, Arc::clone(&*AUDIT_METRICS))
    }

    fn spawn_with_metrics(
        handle: &Handle,
        sink: Arc<dyn AuditSink>,
        capacity: usize,
        metrics: Arc<AuditMetrics>,
    ) -> Self {
        let (sender, mut receiver) = mpsc::channel(capacity.max(1));
        let worker_metrics = Arc::clone(&metrics);
        handle.spawn(async move {
            while let Some(record) = receiver.recv().await {
                if sink.persist(record).await.is_err() {
                    worker_metrics
                        .persist_failures
                        .fetch_add(1, Ordering::Relaxed);
                    AuditMetrics::warn_once(
                        &worker_metrics.warned_persist_failure,
                        "runtime_audit_persist_failed",
                        "runtime audit DB insert failed",
                    );
                } else {
                    worker_metrics.persisted.fetch_add(1, Ordering::Relaxed);
                }
            }
        });
        Self { sender, metrics }
    }

    #[cfg(test)]
    fn from_sender(sender: mpsc::Sender<OwnedAuditRecord>, metrics: Arc<AuditMetrics>) -> Self {
        Self { sender, metrics }
    }

    fn try_enqueue(&self, record: OwnedAuditRecord) -> Result<(), AuditEnqueueError> {
        match self.sender.try_send(record) {
            Ok(()) => {
                self.metrics.enqueued.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.metrics
                    .dropped_queue_full
                    .fetch_add(1, Ordering::Relaxed);
                AuditMetrics::warn_once(
                    &self.metrics.warned_queue_full,
                    "runtime_audit_queue_full",
                    "runtime audit DB queue full; dropping persistence copy",
                );
                Err(AuditEnqueueError::Full)
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                self.metrics
                    .dropped_writer_closed
                    .fetch_add(1, Ordering::Relaxed);
                AuditMetrics::warn_once(
                    &self.metrics.warned_writer_closed,
                    "runtime_audit_writer_closed",
                    "runtime audit DB writer closed; dropping persistence copy",
                );
                Err(AuditEnqueueError::Closed)
            }
        }
    }
}

static AUDIT_WRITER: OnceLock<RwLock<Option<AuditWriter>>> = OnceLock::new();

fn writer_slot() -> &'static RwLock<Option<AuditWriter>> {
    AUDIT_WRITER.get_or_init(|| RwLock::new(None))
}

/// Install or replace the bounded, best-effort MySQL audit writer.
///
/// This must be called from a Tokio runtime. Replacing the sender lets router
/// tests inject their own pool without retaining an unbounded set of workers;
/// the old worker exits after its bounded queue drains.
pub fn install_mysql_writer(pool: MySqlPool) -> Result<(), &'static str> {
    let handle = Handle::try_current().map_err(|_| "Tokio runtime is unavailable")?;
    let sink = Arc::new(MySqlAuditSink { pool });
    let writer = AuditWriter::spawn(&handle, sink, DEFAULT_AUDIT_QUEUE_CAPACITY);
    let mut slot = writer_slot()
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *slot = Some(writer);
    Ok(())
}

/// Emit a structured tracing event and, when allowed by the explicit execution
/// context, enqueue one sanitized DB row.
///
/// Neither an unavailable writer, a full queue nor a DB failure can block or
/// fail the runtime operation being audited.
pub fn record(execution_context: &RuntimeExecutionContext, rec: AuditRecord<'_>) {
    let writer = writer_slot()
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    record_with_writer(
        execution_context,
        rec,
        writer.as_ref(),
        AUDIT_METRICS.as_ref(),
    );
}

fn record_with_writer(
    execution_context: &RuntimeExecutionContext,
    rec: AuditRecord<'_>,
    writer: Option<&AuditWriter>,
    metrics: &AuditMetrics,
) {
    let safe_error = safe_record_error(&rec);
    let event_type = safe_event_type(rec.event_type);
    let outcome = safe_outcome(rec.outcome);
    let capability = rec.capability.and_then(safe_capability);
    let owned = OwnedAuditRecord {
        request_id: execution_context.request_id().map(safe_request_id),
        session_id: execution_context.session_id(),
        agent_id: rec.agent_id,
        plugin_id: rec.plugin_id,
        function_id: rec.function_id,
        capability: capability.map(str::to_string),
        event_type: event_type.to_string(),
        outcome: outcome.to_string(),
        elapsed_ms: rec.elapsed_ms,
        error_message: safe_error.map(|error| error.message.to_string()),
        payload_summary: sanitize_payload_summary(
            event_type,
            capability,
            rec.payload_summary.as_ref(),
        ),
        occurred_at: Utc::now().naive_utc(),
    };

    tracing::info!(
        request_id = owned.request_id.as_deref(),
        session_id = owned.session_id,
        agent_id = owned.agent_id,
        plugin_id = owned.plugin_id,
        function_id = owned.function_id,
        capability = owned.capability.as_deref(),
        event_type = owned.event_type.as_str(),
        outcome = owned.outcome.as_str(),
        elapsed_ms = owned.elapsed_ms,
        error_kind = safe_error.map(|error| error.kind),
        error_message = owned.error_message.as_deref(),
        payload_summary = ?owned.payload_summary,
        "runtime_audit",
    );

    if !execution_context.persists_best_effort() {
        metrics.tracing_only.fetch_add(1, Ordering::Relaxed);
        return;
    }

    let Some(writer) = writer else {
        metrics.dropped_no_writer.fetch_add(1, Ordering::Relaxed);
        AuditMetrics::warn_once(
            &metrics.warned_no_writer,
            "runtime_audit_writer_unavailable",
            "runtime audit DB writer is not installed; tracing event retained",
        );
        return;
    };
    let _ = writer.try_enqueue(owned);
}

#[derive(Debug, Clone, Default)]
pub struct RuntimeAuditFilter {
    pub event_type: Option<String>,
    pub outcome: Option<String>,
    pub capability: Option<String>,
    pub request_id: Option<String>,
    pub session_id: Option<i64>,
    pub agent_id: Option<i64>,
    pub occurred_at_start: Option<NaiveDateTime>,
    pub occurred_at_end: Option<NaiveDateTime>,
}

impl RuntimeAuditFilter {
    fn is_empty(&self) -> bool {
        self.event_type.is_none()
            && self.outcome.is_none()
            && self.capability.is_none()
            && self.request_id.is_none()
            && self.session_id.is_none()
            && self.agent_id.is_none()
            && self.occurred_at_start.is_none()
            && self.occurred_at_end.is_none()
    }
}

fn push_filter_bindings(
    query: &mut QueryBuilder<MySql>,
    filter: &RuntimeAuditFilter,
) {
    let mut predicates = query.separated(" AND ");
    if let Some(value) = &filter.event_type {
        predicates.push("event_type = ").push_bind(value);
    }
    if let Some(value) = &filter.outcome {
        predicates.push("outcome = ").push_bind(value);
    }
    if let Some(value) = &filter.capability {
        predicates.push("capability = ").push_bind(value);
    }
    if let Some(value) = &filter.request_id {
        predicates.push("request_id = ").push_bind(value);
    }
    if let Some(value) = filter.session_id {
        predicates.push("session_id = ").push_bind(value);
    }
    if let Some(value) = filter.agent_id {
        predicates.push("agent_id = ").push_bind(value);
    }
    if let Some(value) = filter.occurred_at_start {
        predicates.push("occurred_at >= ").push_bind(value);
    }
    if let Some(value) = filter.occurred_at_end {
        predicates.push("occurred_at <= ").push_bind(value);
    }
}

fn append_where(query: &mut QueryBuilder<MySql>, filter: &RuntimeAuditFilter) {
    if !filter.is_empty() {
        query.push(" WHERE ");
        push_filter_bindings(query, filter);
    }
}

/// Query a bounded page of runtime audit logs using typed, in-condition-order
/// bindings. API callers must still apply their authorization policy.
pub async fn list_runtime_audit_logs(
    pool: &MySqlPool,
    offset: u32,
    limit: u32,
    filter: &RuntimeAuditFilter,
) -> Result<(Vec<RuntimeAuditLog>, u64), sqlx::Error> {
    let limit = limit.clamp(1, 100);

    let mut count_query = QueryBuilder::<MySql>::new("SELECT COUNT(*) FROM runtime_audit_logs");
    append_where(&mut count_query, filter);
    let (count,): (i64,) = count_query.build_query_as().fetch_one(pool).await?;

    let mut list_query = QueryBuilder::<MySql>::new(
        r#"SELECT id, request_id, session_id, agent_id, plugin_id, function_id,
                  capability, event_type, outcome, elapsed_ms, error_message,
                  payload_summary, occurred_at
           FROM runtime_audit_logs"#,
    );
    append_where(&mut list_query, filter);
    list_query
        .push(" ORDER BY occurred_at DESC, id DESC LIMIT ")
        .push_bind(i64::from(limit))
        .push(" OFFSET ")
        .push_bind(i64::from(offset));
    let items = list_query
        .build_query_as::<RuntimeAuditLog>()
        .fetch_all(pool)
        .await?;

    Ok((items, u64::try_from(count).unwrap_or_default()))
}

pub async fn get_runtime_audit_log_by_id(
    pool: &MySqlPool,
    id: i64,
) -> Result<Option<RuntimeAuditLog>, sqlx::Error> {
    sqlx::query_as::<_, RuntimeAuditLog>(
        r#"SELECT id, request_id, session_id, agent_id, plugin_id, function_id,
                  capability, event_type, outcome, elapsed_ms, error_message,
                  payload_summary, occurred_at
           FROM runtime_audit_logs
           WHERE id = ?"#,
    )
    .bind(id)
    .fetch_optional(pool)
    .await
}

fn safe_event_type(event_type: &str) -> &'static str {
    match event_type {
        "capability_call" => "capability_call",
        "capability_denied" => "capability_denied",
        "plugin_invoke" => "plugin_invoke",
        "workflow_node" => "workflow_node",
        "agent_route" => "agent_route",
        "llm_invoke" => "llm_invoke",
        "llm_fallback" => "llm_fallback",
        "llm_local_fallback" => "llm_local_fallback",
        _ => "runtime_operation",
    }
}

fn safe_outcome(outcome: &str) -> &'static str {
    match outcome {
        "success" => "success",
        "error" => "error",
        "denied" => "denied",
        "timeout" => "timeout",
        "skipped" => "skipped",
        _ => "error",
    }
}

fn safe_capability(capability: &str) -> Option<&'static str> {
    match capability {
        "network.http" => Some("network.http"),
        "fs.read" => Some("fs.read"),
        "fs.write" => Some("fs.write"),
        "s3.read" => Some("s3.read"),
        "s3.write" => Some("s3.write"),
        "db.query" => Some("db.query"),
        "db.execute" => Some("db.execute"),
        "llm.invoke" => Some("llm.invoke"),
        "secret.get" => Some("secret.get"),
        "log.emit" => Some("log.emit"),
        "time.now" => Some("time.now"),
        _ => None,
    }
}

fn safe_request_id(request_id: &str) -> String {
    if !request_id.is_empty()
        && request_id.len() <= 64
        && request_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        request_id.to_string()
    } else {
        format!("sha256-{}", fingerprint(request_id))
    }
}

fn sanitize_payload_summary(
    event_type: &str,
    capability: Option<&str>,
    payload: Option<&Value>,
) -> Option<Value> {
    let payload = payload?.as_object()?;
    let sanitized = match event_type {
        "capability_call" | "capability_denied" => {
            sanitize_capability_summary(safe_capability(capability?)?, payload)
        }
        "agent_route" => {
            let mut result = Map::new();
            copy_i64(payload, &mut result, "to_agent_id");
            Value::Object(result)
        }
        "workflow_node" => {
            let mut result = Map::new();
            copy_i64(payload, &mut result, "workflow_id");
            if let Some(node_type) = payload.get("node_type").and_then(Value::as_str)
                && matches!(node_type, "function_node" | "generate_answer_node")
            {
                result.insert("node_type".into(), Value::String(node_type.to_string()));
            }
            if let Some(node_key) = payload.get("node_key").and_then(Value::as_str) {
                result.insert(
                    "node_key_fingerprint".into(),
                    Value::String(fingerprint(node_key)),
                );
            }
            Value::Object(result)
        }
        "llm_invoke" => sanitize_llm_invocation_summary(payload)?,
        "llm_fallback" => sanitize_llm_fallback_summary(payload)?,
        "llm_local_fallback" => sanitize_llm_local_fallback_summary(payload)?,
        _ => return None,
    };
    bound_payload_summary(Some(sanitized))
}

fn sanitize_capability_summary(capability: &str, payload: &Map<String, Value>) -> Value {
    let mut result = Map::new();
    copy_bool(payload, &mut result, "ok");
    if let Some(error_kind) = payload.get("error_kind").and_then(Value::as_str)
        && is_safe_error_kind(error_kind)
    {
        result.insert("error_kind".into(), Value::String(error_kind.to_string()));
    }

    match capability {
        "network.http" => {
            if let Some(method) = payload.get("method").and_then(Value::as_str)
                && matches!(
                    method,
                    "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "INVALID"
                )
            {
                result.insert("method".into(), Value::String(method.to_string()));
            }
            copy_u64(payload, &mut result, "body_bytes");
            copy_u64(payload, &mut result, "header_count");
            if let Some(host) = payload.get("host").and_then(Value::as_str)
                && is_safe_host(host)
            {
                result.insert("host".into(), Value::String(host.to_ascii_lowercase()));
            }
        }
        "fs.read" => copy_fingerprint(payload, &mut result, "path_fingerprint"),
        "fs.write" => {
            copy_fingerprint(payload, &mut result, "path_fingerprint");
            copy_u64(payload, &mut result, "content_bytes");
            copy_bool(payload, &mut result, "append");
        }
        "s3.read" => copy_fingerprint(payload, &mut result, "key_fingerprint"),
        "s3.write" => {
            copy_fingerprint(payload, &mut result, "key_fingerprint");
            copy_u64(payload, &mut result, "content_bytes");
        }
        "db.query" | "db.execute" => {
            copy_fingerprint(payload, &mut result, "query_fingerprint");
            copy_u64(payload, &mut result, "param_count");
        }
        "llm.invoke" => {
            copy_u64(payload, &mut result, "message_count");
            copy_u64(payload, &mut result, "prompt_bytes");
            copy_u64(payload, &mut result, "max_tokens");
            if let Some(temperature) = payload.get("temperature").and_then(Value::as_f64)
                && temperature.is_finite()
            {
                result.insert("temperature".into(), json!(temperature));
            }
        }
        "secret.get" => copy_fingerprint(payload, &mut result, "key_fingerprint"),
        "log.emit" => {
            if let Some(level) = payload.get("level").and_then(Value::as_str)
                && matches!(level, "error" | "warn" | "info" | "debug")
            {
                result.insert("level".into(), Value::String(level.to_string()));
            }
            copy_u64(payload, &mut result, "message_bytes");
            copy_u64(payload, &mut result, "field_count");
        }
        "time.now" => {}
        _ => {}
    }
    Value::Object(result)
}

fn sanitize_llm_invocation_summary(payload: &Map<String, Value>) -> Option<Value> {
    let mut result = Map::new();
    copy_optional_model_preset(payload, &mut result)?;
    copy_actual_model(payload, &mut result)?;
    let fallback_used = payload.get("fallback_used")?.as_bool()?;
    result.insert("fallback_used".into(), Value::Bool(fallback_used));
    let has_reason = copy_optional_llm_reason(payload, &mut result, is_safe_llm_invoke_reason)?;
    if fallback_used && !has_reason {
        return None;
    }
    copy_llm_source(payload, &mut result)?;

    Some(Value::Object(result))
}

fn sanitize_llm_fallback_summary(payload: &Map<String, Value>) -> Option<Value> {
    let mut result = Map::new();
    copy_optional_model_preset(payload, &mut result)?;
    let from_ordinal = payload.get("from_provider_ordinal")?.as_u64()?;
    let to_ordinal = payload.get("to_provider_ordinal")?.as_u64()?;
    if to_ordinal <= from_ordinal {
        return None;
    }
    result.insert("from_provider_ordinal".into(), json!(from_ordinal));
    result.insert("to_provider_ordinal".into(), json!(to_ordinal));
    copy_required_llm_model(payload, &mut result, "from_model")?;
    copy_required_llm_model(payload, &mut result, "to_model")?;
    copy_required_llm_reason(payload, &mut result, is_provider_fallback_reason)?;

    Some(Value::Object(result))
}

fn sanitize_llm_local_fallback_summary(payload: &Map<String, Value>) -> Option<Value> {
    let mut result = Map::new();
    copy_optional_model_preset(payload, &mut result)?;
    result.insert("actual_model".into(), Value::Null);
    result.insert("fallback_used".into(), Value::Bool(false));
    copy_required_llm_reason(payload, &mut result, is_local_fallback_reason)?;
    copy_llm_source(payload, &mut result)?;
    Some(Value::Object(result))
}

fn copy_optional_model_preset(
    source: &Map<String, Value>,
    target: &mut Map<String, Value>,
) -> Option<()> {
    let Some(value) = source.get("model_preset") else {
        return Some(());
    };
    if let Some(value) = value.as_str()
        && !value.is_empty()
        && value.len() <= 64
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'/' | b':')
        })
    {
        target.insert("model_preset".into(), Value::String(value.to_string()));
    }
    Some(())
}

fn copy_actual_model(source: &Map<String, Value>, target: &mut Map<String, Value>) -> Option<()> {
    match source.get("actual_model")? {
        Value::Null => {
            target.insert("actual_model".into(), Value::Null);
            Some(())
        }
        Value::String(model) if is_safe_llm_model(model) => {
            target.insert("actual_model".into(), Value::String(model.clone()));
            Some(())
        }
        _ => None,
    }
}

fn copy_required_llm_model(
    source: &Map<String, Value>,
    target: &mut Map<String, Value>,
    key: &str,
) -> Option<()> {
    let value = source.get(key)?.as_str()?;
    if !is_safe_llm_model(value) {
        return None;
    }
    target.insert(key.to_string(), Value::String(value.to_string()));
    Some(())
}

fn is_safe_llm_model(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'/' | b':')
        })
}

fn copy_optional_llm_reason(
    source: &Map<String, Value>,
    target: &mut Map<String, Value>,
    allowed: fn(&str) -> bool,
) -> Option<bool> {
    let Some(value) = source.get("reason") else {
        return Some(false);
    };
    if value.is_null() {
        return Some(false);
    }
    let value = value.as_str()?;
    if !allowed(value) {
        return None;
    }
    target.insert("reason".into(), Value::String(value.to_string()));
    Some(true)
}

fn copy_required_llm_reason(
    source: &Map<String, Value>,
    target: &mut Map<String, Value>,
    allowed: fn(&str) -> bool,
) -> Option<()> {
    let value = source.get("reason")?.as_str()?;
    if !allowed(value) {
        return None;
    }
    target.insert("reason".into(), Value::String(value.to_string()));
    Some(())
}

fn is_provider_fallback_reason(reason: &str) -> bool {
    matches!(
        reason,
        "rate_limited"
            | "server_error"
            | "network_error"
            | "tls_error"
            | "provider_timeout"
            | "node_timeout"
            | "circuit_open"
            | "retryable_error"
    )
}

fn is_safe_llm_invoke_reason(reason: &str) -> bool {
    is_provider_fallback_reason(reason)
        || matches!(
            reason,
            "provider_error" | "chain_timeout" | "cancelled" | "model_preset_unknown"
        )
}

fn is_local_fallback_reason(reason: &str) -> bool {
    matches!(
        reason,
        "provider_unavailable" | "invocation_failed" | "empty_response"
    )
}

fn copy_llm_source(source: &Map<String, Value>, target: &mut Map<String, Value>) -> Option<()> {
    let source_name = source.get("source")?.as_str()?;
    let id = match source_name {
        "tool_test" => Some(("tool_id", source.get("tool_id")?.as_i64()?)),
        "skill_test" => Some(("skill_id", source.get("skill_id")?.as_i64()?)),
        "orchestrator" | "llm_invoke" | "generate_answer" | "builtin_game_info" => None,
        _ => return None,
    };
    target.insert("source".into(), Value::String(source_name.to_string()));
    if let Some((id_key, id)) = id {
        if id <= 0 {
            return None;
        }
        target.insert(id_key.into(), json!(id));
    }
    Some(())
}

fn copy_bool(source: &Map<String, Value>, target: &mut Map<String, Value>, key: &str) {
    if let Some(value) = source.get(key).and_then(Value::as_bool) {
        target.insert(key.to_string(), Value::Bool(value));
    }
}

fn copy_u64(source: &Map<String, Value>, target: &mut Map<String, Value>, key: &str) {
    if let Some(value) = source.get(key).and_then(Value::as_u64) {
        target.insert(key.to_string(), json!(value));
    }
}

fn copy_i64(source: &Map<String, Value>, target: &mut Map<String, Value>, key: &str) {
    if let Some(value) = source.get(key).and_then(Value::as_i64) {
        target.insert(key.to_string(), json!(value));
    }
}

fn copy_fingerprint(source: &Map<String, Value>, target: &mut Map<String, Value>, key: &str) {
    if let Some(value) = source.get(key).and_then(Value::as_str)
        && value.len() == 16
        && value.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        target.insert(key.to_string(), Value::String(value.to_ascii_lowercase()));
    }
}

fn is_safe_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && host
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b':'))
}

fn is_safe_error_kind(kind: &str) -> bool {
    matches!(
        kind,
        "invalid_capability_args"
            | "capability_timeout"
            | "network_request_failed"
            | "filesystem_operation_failed"
            | "object_storage_operation_failed"
            | "database_operation_failed"
            | "llm_invocation_failed"
            | "model_preset_unknown"
            | "secret_access_failed"
            | "log_emit_failed"
            | "capability_handler_failed"
            | "capability_response_too_large"
            | "capability_denied"
            | "capability_rate_limited"
            | "runtime_operation_timeout"
            | "runtime_operation_failed"
    )
}

fn bound_payload_summary(payload: Option<Value>) -> Option<Value> {
    let payload = payload?;
    match serde_json::to_vec(&payload) {
        Ok(encoded) if encoded.len() <= MAX_PAYLOAD_SUMMARY_BYTES => Some(payload),
        _ => Some(json!({ "summary_omitted": "size_limit" })),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SafeAuditError {
    pub kind: &'static str,
    pub message: &'static str,
}

fn safe_record_error(rec: &AuditRecord<'_>) -> Option<SafeAuditError> {
    rec.error_message?;

    if rec.outcome == "timeout" {
        return Some(SafeAuditError {
            kind: "runtime_operation_timeout",
            message: "Runtime operation timed out",
        });
    }

    Some(match rec.event_type {
        "capability_denied" => {
            if rec.error_message == Some("Capability rate limit exceeded") {
                SafeAuditError {
                    kind: "capability_rate_limited",
                    message: "Capability rate limit exceeded",
                }
            } else {
                SafeAuditError {
                    kind: "capability_denied",
                    message: "Capability invocation was denied",
                }
            }
        }
        "capability_call" => rec.capability.map_or(
            SafeAuditError {
                kind: "capability_handler_failed",
                message: "Capability handler failed",
            },
            |capability| {
                let kind = rec
                    .payload_summary
                    .as_ref()
                    .and_then(|summary| summary.get("error_kind"))
                    .and_then(Value::as_str);
                safe_capability_error_for_kind(capability, kind)
            },
        ),
        "plugin_invoke" => SafeAuditError {
            kind: "plugin_invocation_failed",
            message: "Plugin invocation failed",
        },
        "llm_invoke" | "llm_fallback" | "llm_local_fallback" => SafeAuditError {
            kind: "llm_invocation_failed",
            message: "LLM invocation failed",
        },
        "workflow_node" => SafeAuditError {
            kind: "workflow_node_failed",
            message: "Workflow node execution failed",
        },
        "agent_route" => SafeAuditError {
            kind: "agent_route_failed",
            message: "Agent routing failed",
        },
        _ => SafeAuditError {
            kind: "runtime_operation_failed",
            message: "Runtime operation failed",
        },
    })
}

pub fn safe_capability_error_for_kind(
    capability: &str,
    error_kind: Option<&str>,
) -> SafeAuditError {
    match error_kind {
        Some("invalid_capability_args") => SafeAuditError {
            kind: "invalid_capability_args",
            message: "Capability arguments were invalid",
        },
        Some("capability_timeout") => SafeAuditError {
            kind: "capability_timeout",
            message: "Capability invocation timed out",
        },
        Some("capability_response_too_large") => SafeAuditError {
            kind: "capability_response_too_large",
            message: "Capability response exceeded the host_call limit",
        },
        Some("model_preset_unknown") => SafeAuditError {
            kind: "model_preset_unknown",
            message: "LLM model preset is unavailable",
        },
        _ => safe_capability_error(capability),
    }
}

pub fn safe_capability_error(capability: &str) -> SafeAuditError {
    match capability {
        "network.http" => SafeAuditError {
            kind: "network_request_failed",
            message: "Network capability request failed",
        },
        "fs.read" | "fs.write" => SafeAuditError {
            kind: "filesystem_operation_failed",
            message: "Filesystem capability operation failed",
        },
        "s3.read" | "s3.write" => SafeAuditError {
            kind: "object_storage_operation_failed",
            message: "Object storage capability operation failed",
        },
        "db.query" | "db.execute" => SafeAuditError {
            kind: "database_operation_failed",
            message: "Database capability operation failed",
        },
        "llm.invoke" => SafeAuditError {
            kind: "llm_invocation_failed",
            message: "LLM capability invocation failed",
        },
        "secret.get" => SafeAuditError {
            kind: "secret_access_failed",
            message: "Secret capability access failed",
        },
        "log.emit" => SafeAuditError {
            kind: "log_emit_failed",
            message: "Log capability emission failed",
        },
        _ => SafeAuditError {
            kind: "capability_handler_failed",
            message: "Capability handler failed",
        },
    }
}

pub fn summarize_capability_call(
    capability: &str,
    args: &Value,
    outcome: &str,
    error_kind: Option<&'static str>,
) -> Option<Value> {
    let mut summary = Map::new();
    summary.insert("ok".into(), Value::Bool(outcome == "success"));
    if let Some(error_kind) = error_kind {
        summary.insert("error_kind".into(), Value::String(error_kind.into()));
    }

    match capability {
        "network.http" => summarize_network(args, outcome, &mut summary),
        "fs.read" => summarize_fs(args, false, &mut summary),
        "fs.write" => summarize_fs(args, true, &mut summary),
        "s3.read" => summarize_s3(args, false, &mut summary),
        "s3.write" => summarize_s3(args, true, &mut summary),
        "db.query" | "db.execute" => summarize_db(args, &mut summary),
        "llm.invoke" => summarize_llm(args, &mut summary),
        "secret.get" => summarize_secret(args, &mut summary),
        "log.emit" => summarize_log(args, &mut summary),
        "time.now" => {}
        _ => return None,
    }

    let value = Value::Object(summary);
    let encoded = serde_json::to_vec(&value).ok()?;
    if encoded.len() <= MAX_PAYLOAD_SUMMARY_BYTES {
        Some(value)
    } else {
        Some(json!({
            "ok": outcome == "success",
            "error_kind": error_kind,
            "summary_omitted": "size_limit"
        }))
    }
}

fn summarize_network(args: &Value, outcome: &str, summary: &mut Map<String, Value>) {
    let method = args
        .get("method")
        .and_then(Value::as_str)
        .map(str::to_ascii_uppercase)
        .filter(|method| matches!(method.as_str(), "GET" | "POST" | "PUT" | "PATCH" | "DELETE"))
        .unwrap_or_else(|| "INVALID".into());
    summary.insert("method".into(), Value::String(method));
    summary.insert(
        "body_bytes".into(),
        json!(args.get("body").and_then(Value::as_str).map_or(0, str::len)),
    );
    summary.insert(
        "header_count".into(),
        json!(
            args.get("headers")
                .and_then(Value::as_object)
                .map_or(0, Map::len)
        ),
    );

    if outcome == "success"
        && let Some(host) = args
            .get("url")
            .and_then(Value::as_str)
            .and_then(|url| reqwest::Url::parse(url).ok())
            .and_then(|url| url.host_str().map(str::to_ascii_lowercase))
    {
        summary.insert("host".into(), Value::String(host));
    }
}

fn summarize_fs(args: &Value, is_write: bool, summary: &mut Map<String, Value>) {
    if let Some(path) = args.get("path").and_then(Value::as_str) {
        summary.insert("path_fingerprint".into(), Value::String(fingerprint(path)));
    }
    if is_write {
        summary.insert(
            "content_bytes".into(),
            json!(
                args.get("content")
                    .and_then(Value::as_str)
                    .map_or(0, str::len)
            ),
        );
        summary.insert(
            "append".into(),
            Value::Bool(args.get("append").and_then(Value::as_bool).unwrap_or(false)),
        );
    }
}

fn summarize_s3(args: &Value, is_write: bool, summary: &mut Map<String, Value>) {
    if let Some(key) = args.get("key").and_then(Value::as_str) {
        summary.insert("key_fingerprint".into(), Value::String(fingerprint(key)));
    }
    if is_write {
        summary.insert(
            "content_bytes".into(),
            json!(
                args.get("content")
                    .and_then(Value::as_str)
                    .map_or(0, str::len)
            ),
        );
    }
}

fn summarize_db(args: &Value, summary: &mut Map<String, Value>) {
    if let Some(query) = args.get("query").and_then(Value::as_str) {
        summary.insert(
            "query_fingerprint".into(),
            Value::String(fingerprint(query)),
        );
    }
    summary.insert(
        "param_count".into(),
        json!(
            args.get("params")
                .and_then(Value::as_object)
                .map_or(0, Map::len)
        ),
    );
}

fn summarize_llm(args: &Value, summary: &mut Map<String, Value>) {
    summary.insert(
        "message_count".into(),
        json!(
            args.get("messages")
                .and_then(Value::as_array)
                .map_or(0, Vec::len)
        ),
    );
    summary.insert(
        "prompt_bytes".into(),
        json!(
            args.get("prompt")
                .and_then(Value::as_str)
                .map_or(0, str::len)
        ),
    );
    if let Some(max_tokens) = args.get("max_tokens").and_then(Value::as_u64) {
        summary.insert("max_tokens".into(), json!(max_tokens));
    }
    if let Some(temperature) = args.get("temperature").and_then(Value::as_f64) {
        summary.insert("temperature".into(), json!(temperature));
    }
}

fn summarize_secret(args: &Value, summary: &mut Map<String, Value>) {
    if let Some(key) = args.get("key").and_then(Value::as_str) {
        summary.insert("key_fingerprint".into(), Value::String(fingerprint(key)));
    }
}

fn summarize_log(args: &Value, summary: &mut Map<String, Value>) {
    summary.insert(
        "level".into(),
        Value::String(safe_log_level(args.get("level").and_then(Value::as_str)).into()),
    );
    summary.insert(
        "message_bytes".into(),
        json!(
            args.get("message")
                .and_then(Value::as_str)
                .map_or(0, str::len)
        ),
    );
    let fields = args.get("fields");
    summary.insert("field_count".into(), json!(log_field_count(fields)));
}

pub fn safe_log_level(level: Option<&str>) -> &'static str {
    match level.map(str::to_ascii_lowercase).as_deref() {
        Some("error") => "error",
        Some("warn") => "warn",
        Some("debug") => "debug",
        _ => "info",
    }
}

pub fn log_field_count(fields: Option<&Value>) -> usize {
    fields.and_then(Value::as_object).map_or(0, Map::len)
}

fn fingerprint(value: &str) -> String {
    let digest = format!("{:x}", Sha256::digest(value.as_bytes()));
    digest[..16].to_string()
}

pub(crate) fn identifier_fingerprint(value: &str) -> String {
    fingerprint(value)
}

#[cfg(test)]
mod tests {
    use super::{
        AuditEnqueueError, AuditMetrics, AuditRecord, AuditSink, AuditWriter,
        MAX_PAYLOAD_SUMMARY_BYTES, OwnedAuditRecord, record, record_with_writer,
        safe_capability_error, safe_record_error, safe_request_id, sanitize_payload_summary,
        summarize_capability_call,
    };
    use crate::runtime::execution_context::RuntimeExecutionContext;
    use chrono::Utc;
    use serde_json::{Value, json};
    use std::sync::{Arc, Mutex};
    use tokio::sync::{Semaphore, mpsc};

    #[derive(Clone, Default)]
    struct TraceWriter(Arc<Mutex<Vec<u8>>>);

    struct TraceWriterGuard(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for TraceWriterGuard {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().expect("trace buffer poisoned").extend(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for TraceWriter {
        type Writer = TraceWriterGuard;

        fn make_writer(&'a self) -> Self::Writer {
            TraceWriterGuard(Arc::clone(&self.0))
        }
    }

    fn rendered_summary(
        capability: &str,
        args: Value,
        outcome: &str,
        error_kind: Option<&'static str>,
    ) -> String {
        let summary = summarize_capability_call(capability, &args, outcome, error_kind)
            .expect("known capabilities must produce a bounded metadata summary");
        let encoded = serde_json::to_vec(&summary).expect("summary must serialize");
        assert!(
            encoded.len() <= MAX_PAYLOAD_SUMMARY_BYTES,
            "summary exceeded 1 KiB: {} bytes",
            encoded.len()
        );
        String::from_utf8(encoded).expect("JSON is UTF-8")
    }

    fn sample_record() -> AuditRecord<'static> {
        AuditRecord {
            agent_id: Some(1),
            plugin_id: Some(2),
            function_id: Some(3),
            capability: None,
            event_type: "plugin_invoke",
            outcome: "success",
            elapsed_ms: Some(4),
            error_message: None,
            payload_summary: None,
        }
    }

    #[test]
    fn hook_execution_context_preserves_correlation_but_is_permanently_tracing_only() {
        let normal = RuntimeExecutionContext::best_effort(Some("request-42".into()), Some(42));
        assert!(normal.persists_best_effort());

        let hook = normal.for_hook();
        assert_eq!(hook.request_id(), Some("request-42"));
        assert_eq!(hook.session_id(), Some(42));
        assert!(!hook.persists_best_effort());
        assert!(!hook.for_hook().persists_best_effort());
    }

    #[test]
    fn record_enqueues_normal_context_but_never_hook_context() {
        let metrics = Arc::new(AuditMetrics::new());
        let (sender, mut receiver) = mpsc::channel(2);
        let writer = AuditWriter::from_sender(sender, Arc::clone(&metrics));
        let normal = RuntimeExecutionContext::best_effort(Some("request-7".into()), Some(7));

        record_with_writer(&normal, sample_record(), Some(&writer), &metrics);
        let persisted = receiver.try_recv().expect("normal audit should be queued");
        assert_eq!(persisted.request_id.as_deref(), Some("request-7"));
        assert_eq!(persisted.session_id, Some(7));

        record_with_writer(&normal.for_hook(), sample_record(), Some(&writer), &metrics);
        assert!(
            receiver.try_recv().is_err(),
            "Hook audit must not reach DB queue"
        );

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.enqueued, 1);
        assert_eq!(snapshot.tracing_only, 1);
    }

    #[test]
    fn writer_metrics_and_static_drop_warnings_are_observable() {
        let trace = TraceWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_target(false)
            .with_writer(trace.clone())
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);

        let metrics = Arc::new(AuditMetrics::new());
        let normal = RuntimeExecutionContext::best_effort(None, None);
        record_with_writer(&normal, sample_record(), None, &metrics);

        let (sender, receiver) = mpsc::channel(1);
        let writer = AuditWriter::from_sender(sender, Arc::clone(&metrics));
        record_with_writer(&normal, sample_record(), Some(&writer), &metrics);
        record_with_writer(&normal, sample_record(), Some(&writer), &metrics);
        drop(receiver);
        record_with_writer(&normal, sample_record(), Some(&writer), &metrics);

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.enqueued, 1);
        assert_eq!(snapshot.dropped_no_writer, 1);
        assert_eq!(snapshot.dropped_queue_full, 1);
        assert_eq!(snapshot.dropped_writer_closed, 1);

        let output =
            String::from_utf8(trace.0.lock().expect("trace buffer poisoned").clone()).unwrap();
        for error_kind in [
            "runtime_audit_writer_unavailable",
            "runtime_audit_queue_full",
            "runtime_audit_writer_closed",
        ] {
            assert!(
                output.contains(error_kind),
                "missing static warning {error_kind}: {output}"
            );
        }
    }

    #[test]
    fn runtime_chains_thread_one_explicit_context_without_task_local_state() {
        let runtime_mod = include_str!("../runtime/mod.rs");
        let assistant = include_str!("../api/chat_assistant.rs");
        let orchestrator = include_str!("../runtime/orchestrator.rs");
        let hook = include_str!("../runtime/hook.rs");
        let workflow = include_str!("../runtime/workflow/workflow_executor.rs");
        let node_executor = include_str!("../runtime/workflow/node_executor.rs");
        let function_node = include_str!("../runtime/workflow/function_node.rs");
        let invoker = include_str!("../runtime/invoker.rs");
        let capability = include_str!("../runtime/capability.rs");

        assert!(runtime_mod.contains("pub mod execution_context;"));
        assert!(assistant.contains("RuntimeExecutionContext::best_effort"));
        assert!(orchestrator.contains("pub execution_context: RuntimeExecutionContext"));
        assert!(hook.contains(".for_hook()"));
        assert!(workflow.contains("pub execution_context: RuntimeExecutionContext"));
        assert!(node_executor.contains("\"workflow_node\""));
        assert!(node_executor.contains("&deps.execution_context,"));
        assert!(node_executor.contains("WorkflowNodeTimeoutAuditGuard"));
        assert!(function_node.contains("execution_context: deps.execution_context.clone()"));
        assert!(invoker.contains("dispatch_ctx.execution_context.clone()"));
        assert!(capability.contains("&ctx.execution_context,"));

        for source in [
            runtime_mod,
            assistant,
            orchestrator,
            hook,
            workflow,
            node_executor,
            function_node,
            invoker,
            capability,
        ] {
            assert!(
                !source.contains("task_local!"),
                "runtime context must be passed explicitly"
            );
        }
    }

    #[test]
    fn summaries_keep_only_capability_specific_metadata() {
        let network = rendered_summary(
            "network.http",
            json!({
                "method": "POST",
                "url": "https://api.example.com/private/path?query=QUERY_SENTINEL",
                "headers": {
                    "authorization": "AUTHORIZATION_SENTINEL",
                    "cookie": "COOKIE_SENTINEL"
                },
                "body": "BODY_SENTINEL"
            }),
            "success",
            None,
        );
        assert!(network.contains("\"method\":\"POST\""));
        assert!(network.contains("\"host\":\"api.example.com\""));
        assert!(network.contains("\"body_bytes\":13"));
        assert!(network.contains("\"header_count\":2"));

        let fs = rendered_summary(
            "fs.write",
            json!({
                "path": "/tmp/plugin/PRIVATE_PATH_SENTINEL",
                "content": "FS_CONTENT_SENTINEL",
                "append": true
            }),
            "success",
            None,
        );
        let s3 = rendered_summary(
            "s3.write",
            json!({
                "key": "plugin-data/PRIVATE_KEY_SENTINEL",
                "content": "S3_CONTENT_SENTINEL"
            }),
            "success",
            None,
        );
        let db = rendered_summary(
            "db.query",
            json!({
                "query": "lookup_user",
                "params": {
                    "authorization": "DB_PARAM_SENTINEL",
                    "cookie": "DB_COOKIE_SENTINEL"
                }
            }),
            "error",
            Some("database_operation_failed"),
        );
        let llm = rendered_summary(
            "llm.invoke",
            json!({
                "messages": [{"role": "user", "content": "MESSAGE_SENTINEL"}],
                "prompt": "PROMPT_SENTINEL",
                "max_tokens": 42,
                "temperature": 0.2
            }),
            "success",
            None,
        );
        let secret = rendered_summary(
            "secret.get",
            json!({"key": "SECRET_NAME_SENTINEL", "value": "SECRET_VALUE_SENTINEL"}),
            "error",
            Some("secret_access_failed"),
        );
        let log = rendered_summary(
            "log.emit",
            json!({
                "level": "warn",
                "message": "LOG_MESSAGE_SENTINEL",
                "fields": {
                    "safe_field": "FIELD_VALUE_SENTINEL",
                    "authorization": "FIELD_AUTH_SENTINEL"
                }
            }),
            "success",
            None,
        );

        let all = [
            network.as_str(),
            fs.as_str(),
            s3.as_str(),
            db.as_str(),
            llm.as_str(),
            secret.as_str(),
            log.as_str(),
        ]
        .join("\n");
        for sentinel in [
            "QUERY_SENTINEL",
            "AUTHORIZATION_SENTINEL",
            "COOKIE_SENTINEL",
            "BODY_SENTINEL",
            "PRIVATE_PATH_SENTINEL",
            "FS_CONTENT_SENTINEL",
            "PRIVATE_KEY_SENTINEL",
            "S3_CONTENT_SENTINEL",
            "DB_PARAM_SENTINEL",
            "DB_COOKIE_SENTINEL",
            "MESSAGE_SENTINEL",
            "PROMPT_SENTINEL",
            "SECRET_NAME_SENTINEL",
            "SECRET_VALUE_SENTINEL",
            "LOG_MESSAGE_SENTINEL",
            "FIELD_VALUE_SENTINEL",
            "FIELD_AUTH_SENTINEL",
        ] {
            assert!(!all.contains(sentinel), "summary leaked {sentinel}: {all}");
        }
        assert!(db.contains("\"param_count\":2"));
        assert!(llm.contains("\"message_count\":1"));
        assert!(secret.contains("\"key_fingerprint\""));
        assert!(log.contains("\"field_count\":2"));
        assert!(!log.contains("\"safe_field\""));
        assert!(!log.contains("\"authorization\""));
    }

    #[test]
    fn unknown_capability_has_no_summary_and_errors_are_static() {
        assert!(
            summarize_capability_call(
                "unknown.SECRET_SENTINEL",
                &json!({"body": "UNKNOWN_BODY_SENTINEL"}),
                "denied",
                Some("unknown_capability"),
            )
            .is_none()
        );

        let raw = "provider failed: ORIGINAL_DOWNSTREAM_ERROR_SENTINEL";
        let safe = safe_capability_error("llm.invoke");
        assert_eq!(safe.kind, "llm_invocation_failed");
        assert_eq!(safe.message, "LLM capability invocation failed");
        assert!(!safe.message.contains(raw));
    }

    #[test]
    fn explicit_static_capability_kind_is_consistent_with_payload_summary() {
        let writer = TraceWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_target(false)
            .with_writer(writer.clone())
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);
        let payload_summary = summarize_capability_call(
            "network.http",
            &json!({"url": "https://example.com"}),
            "error",
            Some("capability_timeout"),
        );
        let audit_record = AuditRecord {
            agent_id: Some(1),
            plugin_id: Some(2),
            function_id: Some(3),
            capability: Some("network.http"),
            event_type: "capability_call",
            outcome: "error",
            elapsed_ms: Some(4),
            error_message: Some("Capability invocation timed out"),
            payload_summary,
        };

        let safe = safe_record_error(&audit_record).expect("typed failure must remain classified");
        assert_eq!(safe.kind, "capability_timeout");
        assert_eq!(safe.message, "Capability invocation timed out");
        assert_eq!(
            audit_record.payload_summary.as_ref().unwrap()["error_kind"],
            "capability_timeout"
        );

        let context =
            RuntimeExecutionContext::best_effort(Some("typed-timeout-audit".into()), None)
                .for_hook();
        record(&context, audit_record);
        let output =
            String::from_utf8(writer.0.lock().expect("trace buffer poisoned").clone()).unwrap();
        assert_eq!(
            output.matches("capability_timeout").count(),
            2,
            "top-level tracing and payload must share one safe kind: {output}"
        );
    }

    #[test]
    fn record_never_traces_caller_supplied_error_text() {
        let writer = TraceWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_target(false)
            .with_writer(writer.clone())
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);

        let execution_context =
            RuntimeExecutionContext::best_effort(Some("safe-audit-test".into()), None);
        record(
            &execution_context,
            AuditRecord {
                agent_id: Some(1),
                plugin_id: Some(2),
                function_id: Some(3),
                capability: None,
                event_type: "plugin_invoke",
                outcome: "error",
                elapsed_ms: Some(4),
                error_message: Some(
                    "EXTISM_JOIN_ERROR_SENTINEL authorization=AUTHORIZATION_SENTINEL",
                ),
                payload_summary: None,
            },
        );

        let output =
            String::from_utf8(writer.0.lock().expect("trace buffer poisoned").clone()).unwrap();
        assert!(output.contains("error_kind=\"plugin_invocation_failed\""));
        assert!(output.contains("error_message=\"Plugin invocation failed\""));
        for sentinel in [
            "EXTISM_JOIN_ERROR_SENTINEL",
            "AUTHORIZATION_SENTINEL",
            "authorization=",
        ] {
            assert!(
                !output.contains(sentinel),
                "audit record leaked {sentinel}: {output}"
            );
        }
    }

    fn owned_record(id: i64) -> OwnedAuditRecord {
        OwnedAuditRecord {
            request_id: Some(format!("request-{id}")),
            session_id: Some(id),
            agent_id: None,
            plugin_id: None,
            function_id: None,
            capability: None,
            event_type: "plugin_invoke".into(),
            outcome: "success".into(),
            elapsed_ms: None,
            error_message: None,
            payload_summary: None,
            occurred_at: Utc::now().naive_utc(),
        }
    }

    struct BlockingSink {
        started: mpsc::UnboundedSender<i64>,
        release: Arc<Semaphore>,
    }

    #[async_trait::async_trait]
    impl AuditSink for BlockingSink {
        async fn persist(&self, record: OwnedAuditRecord) -> Result<(), ()> {
            self.started
                .send(record.session_id.expect("test session id"))
                .map_err(|_| ())?;
            self.release.acquire().await.map_err(|_| ())?.forget();
            Ok(())
        }
    }

    #[tokio::test]
    async fn writer_is_bounded_and_never_waits_for_the_database() {
        let (started_tx, mut started_rx) = mpsc::unbounded_channel();
        let release = Arc::new(Semaphore::new(0));
        let sink = Arc::new(BlockingSink {
            started: started_tx,
            release: Arc::clone(&release),
        });
        let writer = AuditWriter::spawn(&tokio::runtime::Handle::current(), sink, 1);

        assert_eq!(writer.try_enqueue(owned_record(1)), Ok(()));
        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_secs(1), started_rx.recv())
                .await
                .expect("worker must receive promptly"),
            Some(1)
        );
        assert_eq!(writer.try_enqueue(owned_record(2)), Ok(()));
        assert_eq!(
            writer.try_enqueue(owned_record(3)),
            Err(AuditEnqueueError::Full)
        );

        release.add_permits(2);
        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_secs(1), started_rx.recv())
                .await
                .expect("worker must drain queued item"),
            Some(2)
        );
    }

    struct FailingSink {
        attempts: mpsc::UnboundedSender<i64>,
    }

    #[async_trait::async_trait]
    impl AuditSink for FailingSink {
        async fn persist(&self, record: OwnedAuditRecord) -> Result<(), ()> {
            self.attempts
                .send(record.session_id.expect("test session id"))
                .map_err(|_| ())?;
            Err(())
        }
    }

    #[tokio::test]
    async fn writer_continues_after_a_database_failure() {
        let trace = TraceWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_target(false)
            .with_writer(trace.clone())
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);

        let (attempts_tx, mut attempts_rx) = mpsc::unbounded_channel();
        let metrics = Arc::new(AuditMetrics::new());
        let writer = AuditWriter::spawn_with_metrics(
            &tokio::runtime::Handle::current(),
            Arc::new(FailingSink {
                attempts: attempts_tx,
            }),
            2,
            Arc::clone(&metrics),
        );
        assert_eq!(writer.try_enqueue(owned_record(1)), Ok(()));
        assert_eq!(writer.try_enqueue(owned_record(2)), Ok(()));

        let first = tokio::time::timeout(std::time::Duration::from_secs(1), attempts_rx.recv())
            .await
            .expect("first persistence attempt");
        let second = tokio::time::timeout(std::time::Duration::from_secs(1), attempts_rx.recv())
            .await
            .expect("worker must continue after failure");
        assert_eq!((first, second), (Some(1), Some(2)));
        assert_eq!(metrics.snapshot().persist_failures, 2);
        let output =
            String::from_utf8(trace.0.lock().expect("trace buffer poisoned").clone()).unwrap();
        assert!(output.contains("runtime_audit_persist_failed"));
    }

    #[test]
    fn persistence_boundary_rebuilds_only_allowlisted_summaries() {
        let payload = json!({
            "ok": true,
            "method": "PATCH",
            "host": "Api.Example.com",
            "body_bytes": 10,
            "header_count": 2,
            "authorization": "SECRET_SENTINEL",
            "body": "BODY_SENTINEL"
        });
        let sanitized =
            sanitize_payload_summary("capability_call", Some("network.http"), Some(&payload))
                .expect("known summary");
        let rendered = sanitized.to_string();
        assert!(rendered.contains("\"method\":\"PATCH\""));
        assert!(rendered.contains("\"host\":\"api.example.com\""));
        assert!(!rendered.contains("SECRET_SENTINEL"));
        assert!(!rendered.contains("BODY_SENTINEL"));
        assert!(!rendered.contains("authorization"));

        assert!(
            sanitize_payload_summary("unknown_event", Some("network.http"), Some(&payload))
                .is_none()
        );
        assert!(
            sanitize_payload_summary(
                "capability_call",
                Some("unknown.capability"),
                Some(&payload)
            )
            .is_none()
        );
    }

    #[test]
    fn persistence_boundary_rejects_arbitrary_test_and_route_payloads() {
        let route = sanitize_payload_summary(
            "agent_route",
            None,
            Some(&json!({
                "to_agent_id": 42,
                "prompt": "ROUTE_SECRET_SENTINEL"
            })),
        )
        .expect("route summary");
        assert_eq!(route, json!({"to_agent_id": 42}));

        let test = sanitize_payload_summary(
            "llm_invoke",
            None,
            Some(&json!({
                "actual_model": null,
                "fallback_used": false,
                "source": "tool_test",
                "tool_id": 7,
                "prompt": "TEST_SECRET_SENTINEL"
            })),
        )
        .expect("test summary");
        assert_eq!(
            test,
            json!({
                "actual_model": null,
                "fallback_used": false,
                "source": "tool_test",
                "tool_id": 7
            })
        );
        assert!(
            sanitize_payload_summary(
                "llm_invoke",
                None,
                Some(&json!({
                    "actual_model": null,
                    "fallback_used": false,
                    "source": "arbitrary",
                    "tool_id": 7
                }))
            )
            .is_none()
        );
    }

    #[test]
    fn llm_audit_summaries_keep_only_structured_model_and_fallback_metadata() {
        let invocation = sanitize_payload_summary(
            "llm_invoke",
            None,
            Some(&json!({
                "model_preset": "resilient",
                "actual_model": "fallback/model-v2",
                "fallback_used": true,
                "reason": "server_error",
                "source": "tool_test",
                "tool_id": 7,
                "prompt": "LLM_PROMPT_SECRET_SENTINEL",
                "provider_error": "LLM_PROVIDER_ERROR_SENTINEL"
            })),
        )
        .expect("structured LLM invocation summary");
        assert_eq!(
            invocation,
            json!({
                "model_preset": "resilient",
                "actual_model": "fallback/model-v2",
                "fallback_used": true,
                "reason": "server_error",
                "source": "tool_test",
                "tool_id": 7
            })
        );

        let transition = sanitize_payload_summary(
            "llm_fallback",
            None,
            Some(&json!({
                "model_preset": "resilient",
                "from_provider_ordinal": 0,
                "to_provider_ordinal": 1,
                "from_model": "primary-model",
                "to_model": "fallback/model-v2",
                "reason": "server_error",
                "raw_error": "LLM_RAW_ERROR_SENTINEL"
            })),
        )
        .expect("structured LLM fallback summary");
        assert_eq!(
            transition,
            json!({
                "model_preset": "resilient",
                "from_provider_ordinal": 0,
                "to_provider_ordinal": 1,
                "from_model": "primary-model",
                "to_model": "fallback/model-v2",
                "reason": "server_error"
            })
        );

        let rendered = format!("{invocation}{transition}");
        for secret in [
            "LLM_PROMPT_SECRET_SENTINEL",
            "LLM_PROVIDER_ERROR_SENTINEL",
            "LLM_RAW_ERROR_SENTINEL",
        ] {
            assert!(!rendered.contains(secret));
        }
    }

    #[test]
    fn llm_local_fallback_sanitizer_forces_non_provider_semantics() {
        let local = sanitize_payload_summary(
            "llm_local_fallback",
            None,
            Some(&json!({
                "model_preset": "resilient",
                "actual_model": "must-not-survive",
                "fallback_used": true,
                "reason": "invocation_failed",
                "source": "generate_answer",
                "raw_error": "LOCAL_FALLBACK_SECRET_SENTINEL"
            })),
        )
        .expect("typed local fallback summary");
        assert_eq!(
            local,
            json!({
                "model_preset": "resilient",
                "actual_model": null,
                "fallback_used": false,
                "reason": "invocation_failed",
                "source": "generate_answer"
            })
        );
        assert!(!local.to_string().contains("LOCAL_FALLBACK_SECRET_SENTINEL"));
    }

    #[test]
    fn llm_audit_sanitizer_rejects_unapproved_identifiers_sources_and_reasons() {
        let current_registry_name_shape = sanitize_payload_summary(
            "llm_invoke",
            None,
            Some(&json!({
                "model_preset": "A.v1",
                "actual_model": null,
                "fallback_used": false,
                "source": "orchestrator"
            })),
        )
        .expect("current registry-compatible preset name");
        assert_eq!(current_registry_name_shape["model_preset"], "A.v1");

        let too_long = "m".repeat(129);
        let invalid = sanitize_payload_summary(
            "llm_invoke",
            None,
            Some(&json!({
                "model_preset": "BAD PRESET\nSECRET",
                "actual_model": too_long,
                "fallback_used": true,
                "reason": "raw provider error: secret",
                "source": "arbitrary",
                "tool_id": 7,
                "provider_error": "PROVIDER_SECRET_SENTINEL"
            })),
        );
        assert!(
            invalid.is_none(),
            "an invocation without an allowlisted source must be discarded"
        );

        assert!(
            sanitize_payload_summary(
                "llm_fallback",
                None,
                Some(&json!({
                    "from_provider_ordinal": 0,
                    "to_provider_ordinal": 1,
                    "from_model": "primary",
                    "to_model": "fallback",
                    "reason": "raw provider error: secret"
                }))
            )
            .is_none()
        );
    }

    #[test]
    fn workflow_node_summary_contains_only_stable_identifiers() {
        let summary = sanitize_payload_summary(
            "workflow_node",
            None,
            Some(&json!({
                "workflow_id": 11,
                "node_type": "function_node",
                "node_key": "PRIVATE_NODE_KEY_SENTINEL",
                "input": "PRIVATE_INPUT_SENTINEL",
                "output": "PRIVATE_OUTPUT_SENTINEL"
            })),
        )
        .expect("workflow node summary");
        assert_eq!(summary.get("workflow_id"), Some(&json!(11)));
        assert_eq!(summary.get("node_type"), Some(&json!("function_node")));
        let fingerprint = summary
            .get("node_key_fingerprint")
            .and_then(Value::as_str)
            .expect("node key fingerprint");
        assert_eq!(fingerprint.len(), 16);

        let rendered = summary.to_string();
        for sentinel in [
            "PRIVATE_NODE_KEY_SENTINEL",
            "PRIVATE_INPUT_SENTINEL",
            "PRIVATE_OUTPUT_SENTINEL",
        ] {
            assert!(
                !rendered.contains(sentinel),
                "workflow audit leaked {sentinel}"
            );
        }
    }

    #[test]
    fn unsafe_request_ids_are_fingerprinted() {
        assert_eq!(safe_request_id("request_123-abc"), "request_123-abc");
        let safe = safe_request_id("authorization=REQUEST_SECRET_SENTINEL");
        assert!(safe.starts_with("sha256-"));
        assert!(!safe.contains("REQUEST_SECRET_SENTINEL"));
    }

    #[test]
    fn hook_source_has_no_direct_runtime_audit_calls() {
        let hook_source = include_str!("../runtime/hook.rs");
        assert!(hook_source.contains("trace_hook_exec"));
        assert!(!hook_source.contains("runtime_audit::record"));
        assert!(!hook_source.contains("hook_executions"));
    }

    #[test]
    fn plugin_pool_tracing_uses_static_sha_mismatch_classification() {
        let pool_source = include_str!("../runtime/pool.rs");
        assert!(pool_source.contains("error_kind = \"plugin_sha256_mismatch\""));
        for forbidden in ["expected = %row.sha256", "actual = %actual", "error = %"] {
            assert!(
                !pool_source.contains(forbidden),
                "Plugin pool tracing must not expose raw errors or digest values: {forbidden}"
            );
        }
    }
}
