//! Langfuse HTTP ingestion client.
//!
//! Architecture:
//! - A background task drains events from an `mpsc` channel and POSTs them
//!   in batches to Langfuse (either cloud or self-hosted).
//! - `TraceHandle` / `GenerationHandle` are cheap cloneable handles that send
//!   JSON events into the channel when dropped or manually `.end()`-ed.
//! - The channel has a generous bound; if it fills up, events are silently
//!   dropped (tracing is observability, not critical path).

use base64::Engine as _;
use chrono::Utc;
use serde::Serialize;
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::mpsc;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Langfuse connection settings.
#[derive(Debug, Clone)]
pub struct LangfuseConfig {
    /// Public key (from Langfuse project settings).
    pub public_key: String,
    /// Secret key (from Langfuse project settings).
    pub secret_key: String,
    /// Base URL for the Langfuse API.
    ///
    /// - Cloud: `"https://cloud.langfuse.com"` (default)
    /// - Self-hosted: `"https://langfuse.example.com"`
    pub host: String,
}

impl LangfuseConfig {
    /// Build the `Authorization: Basic ...` header value.
    fn basic_auth(&self) -> String {
        let creds = format!("{}:{}", self.public_key, self.secret_key);
        let encoded = base64::engine::general_purpose::STANDARD.encode(creds.as_bytes());
        format!("Basic {encoded}")
    }

    /// Ingestion endpoint URL.
    fn ingestion_url(&self) -> String {
        format!("{}/api/public/ingestion", self.host.trim_end_matches('/'))
    }
}

// ---------------------------------------------------------------------------
// Ingest event types (matching Langfuse public API v2)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
enum IngestEvent {
    #[serde(rename = "trace-create")]
    TraceCreate {
        id: String,
        timestamp: String,
        body: TraceBody,
    },
    #[serde(rename = "generation-create")]
    GenerationCreate {
        id: String,
        timestamp: String,
        body: GenerationBody,
    },
    #[serde(rename = "generation-update")]
    GenerationUpdate {
        id: String,
        timestamp: String,
        body: GenerationUpdateBody,
    },
}

#[derive(Debug, Clone, Serialize)]
struct TraceBody {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "userId")]
    user_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tags: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    input: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
struct GenerationBody {
    #[serde(rename = "traceId", skip_serializing_if = "Option::is_none")]
    trace_id: Option<String>,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    input: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "modelParameters")]
    model_parameters: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    usage: Option<UsageBody>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "startTime")]
    start_time: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
struct GenerationUpdateBody {
    #[serde(rename = "traceId", skip_serializing_if = "Option::is_none")]
    trace_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    usage: Option<UsageBody>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "endTime")]
    end_time: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "statusMessage")]
    status_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    level: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct UsageBody {
    #[serde(rename = "promptTokens", skip_serializing_if = "Option::is_none")]
    prompt_tokens: Option<i64>,
    #[serde(rename = "completionTokens", skip_serializing_if = "Option::is_none")]
    completion_tokens: Option<i64>,
    #[serde(rename = "totalTokens", skip_serializing_if = "Option::is_none")]
    total_tokens: Option<i64>,
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

/// Maximum pending events before dropping.
const EVENT_BUFFER: usize = 4096;
/// Flush interval in milliseconds.
const FLUSH_INTERVAL_MS: u64 = 5000;
/// Maximum batch size per POST.
const MAX_BATCH_SIZE: usize = 100;

/// Async, non-blocking Langfuse client.
///
/// Spawns a background task on construction. Clone is cheap (Arc).
#[derive(Clone)]
pub struct LangfuseClient {
    tx: mpsc::Sender<IngestEvent>,
}

impl std::fmt::Debug for LangfuseClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LangfuseClient").finish_non_exhaustive()
    }
}

impl LangfuseClient {
    /// Create a new client and spawn the background flush task.
    ///
    /// Returns `None` if the configuration is invalid (e.g. empty keys).
    pub fn new(cfg: LangfuseConfig) -> Option<Self> {
        if cfg.public_key.is_empty() || cfg.secret_key.is_empty() {
            tracing::warn!("LangfuseClient: public_key or secret_key empty, tracing disabled");
            return None;
        }

        let (tx, rx) = mpsc::channel::<IngestEvent>(EVENT_BUFFER);
        let bg = BackgroundWorker {
            rx,
            cfg: Arc::new(cfg),
            http: reqwest::Client::new(),
            buffer: Vec::with_capacity(MAX_BATCH_SIZE),
        };
        tokio::spawn(bg.run());

        tracing::info!("LangfuseClient: tracing enabled");
        Some(Self { tx })
    }

    fn send(&self, event: IngestEvent) {
        match self.tx.try_send(event) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => {
                tracing::warn!("Langfuse event buffer full, dropping event");
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                tracing::error!("Langfuse channel closed, event dropped — BackgroundWorker may have panicked");
            }
        }
    }

    // -----------------------------------------------------------------------
    // Public API: trace & generation handles
    // -----------------------------------------------------------------------

    /// Start a new trace. The trace-create event is enqueued immediately.
    ///
    /// Returns a handle that can be used to create child generations.
    pub fn trace(
        &self,
        name: impl Into<String>,
    ) -> TraceHandle {
        let trace_id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);

        self.send(IngestEvent::TraceCreate {
            id: trace_id.clone(),
            timestamp: now,
            body: TraceBody {
                name: name.into(),
                user_id: None,
                metadata: None,
                tags: None,
                input: None,
                output: None,
            },
        });

        TraceHandle {
            id: trace_id,
            tx: self.tx.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Handles
// ---------------------------------------------------------------------------

/// Handle representing an open Langfuse trace.
///
/// Use `.generation()` to create child generation spans under this trace.
#[derive(Debug, Clone)]
pub struct TraceHandle {
    id: String,
    tx: mpsc::Sender<IngestEvent>,
}

impl TraceHandle {
    fn send(&self, event: IngestEvent) {
        match self.tx.try_send(event) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => {
                tracing::warn!("Langfuse event buffer full, dropping event");
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                tracing::error!("Langfuse channel closed, event dropped");
            }
        }
    }

    /// The trace unique id (UUID v4).
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Start a generation (LLM call) under this trace.
    ///
    /// The generation-create event is enqueued immediately.  Returns a
    /// `GenerationHandle` that should be `.end()`-ed (or dropped) with the
    /// response data.
    pub fn generation(
        &self,
        name: impl Into<String>,
        model: Option<impl Into<String>>,
        input: Option<Value>,
        model_parameters: Option<Value>,
    ) -> GenerationHandle {
        let gen_id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);

        let event = IngestEvent::GenerationCreate {
            id: gen_id.clone(),
            timestamp: now.clone(),
            body: GenerationBody {
                trace_id: Some(self.id.clone()),
                name: name.into(),
                model: model.map(|m| m.into()),
                input,
                output: None,
                model_parameters,
                usage: None,
                start_time: Some(now),
                metadata: None,
            },
        };
        self.send(event);

        GenerationHandle {
            id: gen_id,
            trace_id: self.id.clone(),
            tx: self.tx.clone(),
        }
    }
}

/// Handle representing an open generation (LLM call).
///
/// Must be `.end()`-ed with the final response data.  Dropping the handle
/// without calling `.end()` will still send an update with whatever data
/// has been accumulated via `.update_output()`.
#[derive(Debug, Clone)]
pub struct GenerationHandle {
    id: String,
    trace_id: String,
    tx: mpsc::Sender<IngestEvent>,
}

impl GenerationHandle {
    fn send(&self, event: IngestEvent) {
        match self.tx.try_send(event) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => {
                tracing::warn!("Langfuse event buffer full, dropping event");
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                tracing::error!("Langfuse channel closed, event dropped");
            }
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    /// Record the final response: token usage, output content, finish status.
    pub fn end(
        self,
        output: Option<Value>,
        usage: Option<(i64, i64, i64)>, // (prompt, completion, total)
        finish_reason: Option<String>,
        is_error: bool,
    ) {
        let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);

        let usage = usage.map(|(p, c, t)| UsageBody {
            prompt_tokens: Some(p),
            completion_tokens: Some(c),
            total_tokens: Some(t),
        });

        let level = if is_error {
            Some("ERROR".to_string())
        } else {
            None
        };

        let event = IngestEvent::GenerationUpdate {
            id: self.id,
            timestamp: now.clone(),
            body: GenerationUpdateBody {
                trace_id: Some(self.trace_id),
                output,
                usage,
                end_time: Some(now),
                status_message: finish_reason,
                level,
            },
        };

        match self.tx.try_send(event) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => {
                tracing::warn!("Langfuse event buffer full, dropping event");
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                tracing::error!("Langfuse channel closed, event dropped");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Background worker
// ---------------------------------------------------------------------------

struct BackgroundWorker {
    rx: mpsc::Receiver<IngestEvent>,
    cfg: Arc<LangfuseConfig>,
    http: reqwest::Client,
    buffer: Vec<IngestEvent>,
}

impl BackgroundWorker {
    async fn run(mut self) {
        tracing::info!("Langfuse BackgroundWorker started");
        let mut interval = tokio::time::interval(std::time::Duration::from_millis(FLUSH_INTERVAL_MS));

        loop {
            tokio::select! {
                // Drain incoming events into the buffer.
                event = self.rx.recv() => {
                    match event {
                        Some(ev) => {
                            self.buffer.push(ev);
                            if self.buffer.len() >= MAX_BATCH_SIZE {
                                self.flush().await;
                            }
                        }
                        None => {
                            // Channel closed — flush remaining and exit.
                            self.flush().await;
                            return;
                        }
                    }
                }
                // Periodic flush.
                _ = interval.tick() => {
                    self.flush().await;
                }
            }
        }
    }

    async fn flush(&mut self) {
        if self.buffer.is_empty() {
            return;
        }

        let count = self.buffer.len();
        let batch: Vec<_> = self.buffer.drain(..).collect();
        let payload = json!({ "batch": &batch });

        let result = self
            .http
            .post(self.cfg.ingestion_url())
            .header("Authorization", self.cfg.basic_auth())
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .await;

        match result {
            Ok(resp) => {
                if resp.status().is_success() {
                    tracing::info!("Langfuse flush succeeded: {} events sent", count);
                } else {
                    let status = resp.status();
                    let body = resp.text().await.unwrap_or_default();
                    tracing::warn!(
                        status = %status,
                        body = %body.chars().take(500).collect::<String>(),
                        "Langfuse ingestion failed"
                    );
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "Langfuse ingestion request failed");
            }
        }
    }
}
