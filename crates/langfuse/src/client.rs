//! Langfuse HTTP ingestion client.
//!
//! Architecture:
//! - A background task drains events from an `mpsc` channel and POSTs them
//!   in batches to Langfuse (either cloud or self-hosted).
//! - `TraceHandle` / `GenerationHandle` are cheap cloneable handles that send
//!   JSON events into the channel when dropped or manually `.end()`-ed.
//! - The channel has a generous bound; if it fills up, events are silently
//!   dropped (tracing is observability, not critical path).

use crate::lf_debug;
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
    /// Optional environment name for tracing (e.g., "production", "staging").
    pub environment: Option<String>,
    /// Optional release version/hash of your application.
    pub release: Option<String>,
    /// Optional sampling rate for traces (0.0 to 1.0). Default is 1.0 (100%).
    pub sample_rate: f64,
}

impl Default for LangfuseConfig {
    fn default() -> Self {
        Self {
            public_key: String::new(),
            secret_key: String::new(),
            host: "https://cloud.langfuse.com".to_string(),
            environment: None,
            release: None,
            sample_rate: 1.0,
        }
    }
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
// Observation types (matching Langfuse Python SDK observation types)
// ---------------------------------------------------------------------------

/// Types of observations supported by Langfuse (matching Python SDK).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObservationType {
    /// General-purpose span for any operation
    Span,
    /// LLM generation (supports model, usage, cost, prompt fields)
    Generation,
    /// Agent reasoning block
    Agent,
    /// External tool call
    Tool,
    /// Chain connecting LLM application steps
    Chain,
    /// Data retrieval step (vector store, database)
    Retriever,
    /// Evaluator for assessing LLM output quality
    Evaluator,
    /// Embedding call
    Embedding,
    /// Guardrail for content safety
    Guardrail,
    /// Instant event (cannot be updated after creation)
    Event,
}

impl ObservationType {
    /// Returns the string representation for serialization.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Span => "span",
            Self::Generation => "generation",
            Self::Agent => "agent",
            Self::Tool => "tool",
            Self::Chain => "chain",
            Self::Retriever => "retriever",
            Self::Evaluator => "evaluator",
            Self::Embedding => "embedding",
            Self::Guardrail => "guardrail",
            Self::Event => "event",
        }
    }

    /// Returns true if this is a generation-like type (supports model, usage, cost).
    pub fn is_generation_like(&self) -> bool {
        matches!(self, Self::Generation | Self::Embedding)
    }
}

/// Span level for observation severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum SpanLevel {
    Debug,
    Default,
    Warning,
    Error,
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
    #[serde(rename = "trace-update")]
    TraceUpdate {
        id: String,
        timestamp: String,
        body: TraceUpdateBody,
    },
    #[serde(rename = "span-create")]
    SpanCreate {
        id: String,
        trace_id: String,
        timestamp: String,
        body: SpanBody,
    },
    #[serde(rename = "span-update")]
    SpanUpdate {
        id: String,
        trace_id: String,
        timestamp: String,
        body: SpanUpdateBody,
    },
    #[serde(rename = "generation-create")]
    GenerationCreate {
        id: String,
        trace_id: String,
        timestamp: String,
        body: GenerationBody,
    },
    #[serde(rename = "generation-update")]
    GenerationUpdate {
        id: String,
        trace_id: String,
        timestamp: String,
        body: GenerationUpdateBody,
    },
    #[serde(rename = "event-create")]
    EventCreate {
        id: String,
        trace_id: String,
        timestamp: String,
        body: EventBody,
    },
    #[serde(rename = "score-create")]
    ScoreCreate {
        id: String,
        timestamp: String,
        body: ScoreBody,
    },
}

#[derive(Debug, Clone, Serialize)]
struct TraceBody {
    id: String,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "userId")]
    user_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tags: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    input: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    environment: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    release: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "public")]
    is_public: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
struct TraceUpdateBody {
    id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    output: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
struct SpanBody {
    id: String,
    #[serde(rename = "traceId")]
    trace_id: String,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "parentObservationId")]
    parent_observation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    input: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    level: Option<SpanLevel>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "statusMessage")]
    status_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "startTime")]
    start_time: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct SpanUpdateBody {
    id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    input: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    level: Option<SpanLevel>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "statusMessage")]
    status_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "endTime")]
    end_time: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct GenerationBody {
    id: String,
    #[serde(rename = "traceId")]
    trace_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "parentObservationId")]
    parent_observation_id: Option<String>,
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
    #[serde(rename = "completionStartTime")]
    completion_start_time: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    level: Option<SpanLevel>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "statusMessage")]
    status_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "promptName")]
    prompt_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "promptVersion")]
    prompt_version: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "costDetails")]
    cost_details: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
struct GenerationUpdateBody {
    id: String,
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
    level: Option<SpanLevel>,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "promptName")]
    prompt_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "promptVersion")]
    prompt_version: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "costDetails")]
    cost_details: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
struct EventBody {
    id: String,
    #[serde(rename = "traceId")]
    trace_id: String,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "parentObservationId")]
    parent_observation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    input: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    level: Option<SpanLevel>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "statusMessage")]
    status_message: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct ScoreBody {
    id: String,
    #[serde(rename = "traceId")]
    trace_id: String,
    name: String,
    value: ScoreValue,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "observationId")]
    observation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    comment: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "configId")]
    config_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "dataType")]
    data_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
enum ScoreValue {
    Numeric(f64),
    Boolean(bool),
    Categorical(String),
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
        lf_debug!("LangfuseClient::new called | public_key_len={} secret_key_len={} host={}",
            cfg.public_key.len(), cfg.secret_key.len(), cfg.host);

        if cfg.public_key.is_empty() || cfg.secret_key.is_empty() {
            lf_debug!("LangfuseClient::new -> BAIL: empty keys (pub={} sec={})",
                cfg.public_key.is_empty(), cfg.secret_key.is_empty());
            tracing::warn!("LangfuseClient: public_key or secret_key empty, tracing disabled");
            return None;
        }

        if cfg.sample_rate < 0.0 || cfg.sample_rate > 1.0 {
            tracing::warn!("LangfuseClient: sample_rate must be between 0.0 and 1.0, got {}", cfg.sample_rate);
            return None;
        }

        lf_debug!("LangfuseClient::new: keys valid, creating mpsc channel with buffer={}", EVENT_BUFFER);
        let (tx, rx) = mpsc::channel::<IngestEvent>(EVENT_BUFFER);
        lf_debug!("LangfuseClient::new: channel created, spawning BackgroundWorker");
        let host = cfg.host.clone();
        let bg = BackgroundWorker {
            rx,
            cfg: Arc::new(cfg),
            http: reqwest::Client::new(),
            buffer: Vec::with_capacity(MAX_BATCH_SIZE),
        };
        tokio::spawn(bg.run());
        lf_debug!("LangfuseClient::new: BackgroundWorker spawned, ingestion_url={}/api/public/ingestion", host.trim_end_matches('/'));

        tracing::info!("LangfuseClient: tracing enabled");
        lf_debug!("LangfuseClient::new -> SUCCESS, returning Some(client)");
        Some(Self { tx })
    }

    fn send(&self, event: IngestEvent) {
        let event_debug = format!("{:?}", &event);
        lf_debug!("LangfuseClient::send event={}", truncate_str(&event_debug, 300));
        match self.tx.try_send(event) {
            Ok(()) => {
                lf_debug!("LangfuseClient::send -> OK (enqueued)");
            }
            Err(mpsc::error::TrySendError::Full(ev)) => {
                lf_debug!("LangfuseClient::send -> FULL (dropping event type={:?})",
                    extract_event_type(&ev));
                tracing::warn!("Langfuse event buffer full, dropping event");
            }
            Err(mpsc::error::TrySendError::Closed(ev)) => {
                lf_debug!("LangfuseClient::send -> CLOSED (dropping event type={:?})",
                    extract_event_type(&ev));
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
    /// Call `.set_output()` on the handle after the LLM response arrives
    /// to record the output at trace level.
    pub fn trace(
        &self,
        name: impl Into<String>,
        input: Option<Value>,
    ) -> TraceHandle {
        self.trace_with_options(name, input, TraceOptions::default())
    }

    /// Start a new trace with additional options.
    pub fn trace_with_options(
        &self,
        name: impl Into<String>,
        input: Option<Value>,
        options: TraceOptions,
    ) -> TraceHandle {
        let trace_id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let name: String = name.into();

        lf_debug!("LangfuseClient::trace name={} trace_id={} timestamp={} input_len={}",
            name, trace_id, now,
            input.as_ref().map(|v| serde_json::to_string(v).unwrap_or_default().len()).unwrap_or(0));

        self.send(IngestEvent::TraceCreate {
            id: trace_id.clone(),
            timestamp: now,
            body: TraceBody {
                id: trace_id.clone(),
                name,
                user_id: options.user_id,
                session_id: options.session_id,
                metadata: options.metadata,
                tags: options.tags,
                input,
                output: None,
                version: options.version,
                environment: options.environment.or_else(|| {
                    // Will be set from config in BackgroundWorker if not provided
                    None
                }),
                release: options.release,
                is_public: options.is_public,
            },
        });

        lf_debug!("LangfuseClient::trace -> returning TraceHandle id={}", trace_id);
        TraceHandle {
            id: trace_id,
            tx: self.tx.clone(),
        }
    }

    /// Create a score for a trace.
    pub fn score(
        &self,
        trace_id: impl Into<String>,
        name: impl Into<String>,
        value: ScoreValueInput,
    ) {
        self.score_with_options(trace_id, name, value, ScoreOptions::default())
    }

    /// Create a score with additional options.
    pub fn score_with_options(
        &self,
        trace_id: impl Into<String>,
        name: impl Into<String>,
        value: ScoreValueInput,
        options: ScoreOptions,
    ) {
        let score_id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);

        let score_value = match value {
            ScoreValueInput::Numeric(v) => ScoreValue::Numeric(v),
            ScoreValueInput::Boolean(v) => ScoreValue::Boolean(v),
            ScoreValueInput::Categorical(v) => ScoreValue::Categorical(v),
        };

        let data_type = options.data_type.or_else(|| {
            match &score_value {
                ScoreValue::Numeric(_) => Some("NUMERIC".to_string()),
                ScoreValue::Boolean(_) => Some("BOOLEAN".to_string()),
                ScoreValue::Categorical(_) => Some("CATEGORICAL".to_string()),
            }
        });

        self.send(IngestEvent::ScoreCreate {
            id: score_id,
            timestamp: now,
            body: ScoreBody {
                id: uuid::Uuid::new_v4().to_string(),
                trace_id: trace_id.into(),
                name: name.into(),
                value: score_value,
                observation_id: options.observation_id,
                comment: options.comment,
                config_id: options.config_id,
                data_type,
                metadata: options.metadata,
            },
        });
    }
}

// ---------------------------------------------------------------------------
// Option structs for API methods
// ---------------------------------------------------------------------------

/// Options for creating a trace.
#[derive(Debug, Clone, Default)]
pub struct TraceOptions {
    pub user_id: Option<String>,
    pub session_id: Option<String>,
    pub metadata: Option<Value>,
    pub tags: Option<Vec<String>>,
    pub version: Option<String>,
    pub environment: Option<String>,
    pub release: Option<String>,
    pub is_public: Option<bool>,
}

/// Options for creating a score.
#[derive(Debug, Clone, Default)]
pub struct ScoreOptions {
    pub observation_id: Option<String>,
    pub comment: Option<String>,
    pub config_id: Option<String>,
    pub data_type: Option<String>,
    pub metadata: Option<Value>,
}

/// Score value input type.
#[derive(Debug, Clone)]
pub enum ScoreValueInput {
    Numeric(f64),
    Boolean(bool),
    Categorical(String),
}

/// Options for creating a span.
#[derive(Debug, Clone, Default)]
pub struct SpanOptions {
    pub parent_observation_id: Option<String>,
    pub input: Option<Value>,
    pub output: Option<Value>,
    pub metadata: Option<Value>,
    pub version: Option<String>,
    pub level: Option<SpanLevel>,
    pub status_message: Option<String>,
}

/// Options for creating a generation.
#[derive(Debug, Clone, Default)]
pub struct GenerationOptions {
    pub parent_observation_id: Option<String>,
    pub input: Option<Value>,
    pub model: Option<String>,
    pub model_parameters: Option<Value>,
    pub metadata: Option<Value>,
    pub version: Option<String>,
    pub level: Option<SpanLevel>,
    pub status_message: Option<String>,
    pub completion_start_time: Option<String>,
    pub prompt_name: Option<String>,
    pub prompt_version: Option<i32>,
    pub cost_details: Option<Value>,
}

/// Options for creating an event.
#[derive(Debug, Clone, Default)]
pub struct EventOptions {
    pub parent_observation_id: Option<String>,
    pub input: Option<Value>,
    pub output: Option<Value>,
    pub metadata: Option<Value>,
    pub version: Option<String>,
    pub level: Option<SpanLevel>,
    pub status_message: Option<String>,
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
        let event_debug = format!("{:?}", &event);
        lf_debug!("TraceHandle::send trace_id={} event={}",
            self.id, truncate_str(&event_debug, 300));
        match self.tx.try_send(event) {
            Ok(()) => {
                lf_debug!("TraceHandle::send -> OK (enqueued)");
            }
            Err(mpsc::error::TrySendError::Full(ev)) => {
                lf_debug!("TraceHandle::send -> FULL (dropping event type={:?})",
                    extract_event_type(&ev));
                tracing::warn!("Langfuse event buffer full, dropping event");
            }
            Err(mpsc::error::TrySendError::Closed(ev)) => {
                lf_debug!("TraceHandle::send -> CLOSED (dropping event type={:?})",
                    extract_event_type(&ev));
                tracing::error!("Langfuse channel closed, event dropped");
            }
        }
    }

    /// The trace unique id (UUID v4).
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Update the trace output after the LLM response is available.
    /// Sends a trace-update event with the output value.
    pub fn set_output(&self, output: Option<Value>) {
        let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        lf_debug!("TraceHandle::set_output trace_id={} output_len={}",
            self.id,
            output.as_ref().map(|v| serde_json::to_string(v).unwrap_or_default().len()).unwrap_or(0));
        self.send(IngestEvent::TraceUpdate {
            id: self.id.clone(),
            timestamp: now,
            body: TraceUpdateBody {
                id: self.id.clone(),
                output,
            },
        });
    }

    /// Make this trace publicly accessible via its URL.
    pub fn set_public(&self) {
        let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        self.send(IngestEvent::TraceUpdate {
            id: self.id.clone(),
            timestamp: now,
            body: TraceUpdateBody {
                id: self.id.clone(),
                output: None,
            },
        });
    }

    /// Start a span under this trace.
    pub fn span(&self, name: impl Into<String>) -> SpanHandle {
        self.span_with_options(name, SpanOptions::default())
    }

    /// Start a span with options.
    pub fn span_with_options(&self, name: impl Into<String>, options: SpanOptions) -> SpanHandle {
        let span_id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let name: String = name.into();

        let event = IngestEvent::SpanCreate {
            id: span_id.clone(),
            trace_id: self.id.clone(),
            timestamp: now,
            body: SpanBody {
                id: span_id.clone(),
                trace_id: self.id.clone(),
                name,
                parent_observation_id: options.parent_observation_id,
                input: options.input,
                output: options.output,
                metadata: options.metadata,
                version: options.version,
                level: options.level,
                status_message: options.status_message,
                start_time: Some(Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)),
            },
        };
        self.send(event);

        SpanHandle {
            id: span_id,
            trace_id: self.id.clone(),
            tx: self.tx.clone(),
        }
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
        let model_owned: Option<String> = model.map(|m| m.into());
        let mut options = GenerationOptions::default();
        options.model = model_owned;
        options.input = input;
        options.model_parameters = model_parameters;
        self.generation_with_options(name, options)
    }

    /// Start a generation with options.
    pub fn generation_with_options(&self, name: impl Into<String>, options: GenerationOptions) -> GenerationHandle {
        let gen_id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let name: String = name.into();

        lf_debug!("TraceHandle::generation name={} gen_id={} trace_id={}",
            name, gen_id, self.id);

        let event = IngestEvent::GenerationCreate {
            id: gen_id.clone(),
            trace_id: self.id.clone(),
            timestamp: now.clone(),
            body: GenerationBody {
                id: gen_id.clone(),
                trace_id: self.id.clone(),
                name: Some(name),
                parent_observation_id: options.parent_observation_id,
                model: options.model,
                input: options.input,
                output: None,
                model_parameters: options.model_parameters,
                usage: None,
                start_time: Some(now),
                completion_start_time: options.completion_start_time,
                metadata: options.metadata,
                version: options.version,
                level: options.level,
                status_message: options.status_message,
                prompt_name: options.prompt_name,
                prompt_version: options.prompt_version,
                cost_details: options.cost_details,
            },
        };
        self.send(event);

        GenerationHandle {
            id: gen_id,
            trace_id: self.id.clone(),
            tx: self.tx.clone(),
        }
    }

    /// Create an event under this trace.
    pub fn event(&self, name: impl Into<String>) {
        self.event_with_options(name, EventOptions::default());
    }

    /// Create an event with options.
    pub fn event_with_options(&self, name: impl Into<String>, options: EventOptions) {
        let event_id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let name: String = name.into();

        let ingest_event = IngestEvent::EventCreate {
            id: event_id,
            trace_id: self.id.clone(),
            timestamp: now,
            body: EventBody {
                id: uuid::Uuid::new_v4().to_string(),
                trace_id: self.id.clone(),
                name,
                parent_observation_id: options.parent_observation_id,
                input: options.input,
                output: options.output,
                metadata: options.metadata,
                version: options.version,
                level: options.level,
                status_message: options.status_message,
            },
        };
        self.send(ingest_event);
    }

    /// Create a score for this trace.
    pub fn score(&self, name: impl Into<String>, value: ScoreValueInput) {
        self.score_with_options(name, value, ScoreOptions::default());
    }

    /// Create a score with options.
    pub fn score_with_options(&self, name: impl Into<String>, value: ScoreValueInput, options: ScoreOptions) {
        let score_id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);

        let score_value = match value {
            ScoreValueInput::Numeric(v) => ScoreValue::Numeric(v),
            ScoreValueInput::Boolean(v) => ScoreValue::Boolean(v),
            ScoreValueInput::Categorical(v) => ScoreValue::Categorical(v),
        };

        let data_type = options.data_type.or_else(|| {
            match &score_value {
                ScoreValue::Numeric(_) => Some("NUMERIC".to_string()),
                ScoreValue::Boolean(_) => Some("BOOLEAN".to_string()),
                ScoreValue::Categorical(_) => Some("CATEGORICAL".to_string()),
            }
        });

        self.send(IngestEvent::ScoreCreate {
            id: score_id,
            timestamp: now,
            body: ScoreBody {
                id: uuid::Uuid::new_v4().to_string(),
                trace_id: self.id.clone(),
                name: name.into(),
                value: score_value,
                observation_id: options.observation_id,
                comment: options.comment,
                config_id: options.config_id,
                data_type,
                metadata: options.metadata,
            },
        });
    }
}

/// Handle representing an open Langfuse span.
#[derive(Debug, Clone)]
pub struct SpanHandle {
    id: String,
    trace_id: String,
    tx: mpsc::Sender<IngestEvent>,
}

impl SpanHandle {
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

    pub fn update(
        &self,
        name: Option<String>,
        input: Option<Value>,
        output: Option<Value>,
        metadata: Option<Value>,
        version: Option<String>,
        level: Option<SpanLevel>,
        status_message: Option<String>,
    ) {
        let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        self.send(IngestEvent::SpanUpdate {
            id: self.id.clone(),
            trace_id: self.trace_id.clone(),
            timestamp: now,
            body: SpanUpdateBody {
                id: self.id.clone(),
                name,
                input,
                output,
                metadata,
                version,
                level,
                status_message,
                end_time: None,
            },
        });
    }

    pub fn end(self) {
        let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let span_id = self.id.clone();
        let trace_id = self.trace_id.clone();
        self.send(IngestEvent::SpanUpdate {
            id: span_id.clone(),
            trace_id: trace_id.clone(),
            timestamp: now.clone(),
            body: SpanUpdateBody {
                id: span_id,
                name: None,
                input: None,
                output: None,
                metadata: None,
                version: None,
                level: None,
                status_message: None,
                end_time: Some(now),
            },
        });
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
        self.end_with_options(output, usage, finish_reason, is_error, GenerationEndOptions::default())
    }

    /// Record the final response with additional options.
    pub fn end_with_options(
        self,
        output: Option<Value>,
        usage: Option<(i64, i64, i64)>,
        finish_reason: Option<String>,
        is_error: bool,
        options: GenerationEndOptions,
    ) {
        let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);

        let usage_body = usage.map(|(p, c, t)| UsageBody {
            prompt_tokens: Some(p),
            completion_tokens: Some(c),
            total_tokens: Some(t),
        });

        let level = if is_error {
            Some(SpanLevel::Error)
        } else {
            options.level
        };

        let output_len = output.as_ref()
            .map(|v| serde_json::to_string(v).unwrap_or_default().len())
            .unwrap_or(0);

        lf_debug!("GenerationHandle::end gen_id={} trace_id={} output_len={} usage={:?} finish_reason={:?} is_error={}",
            self.id, self.trace_id, output_len,
            usage, finish_reason, is_error);

        let gen_id_copy = self.id.clone();

        let event = IngestEvent::GenerationUpdate {
            id: self.id.clone(),
            trace_id: self.trace_id.clone(),
            timestamp: now.clone(),
            body: GenerationUpdateBody {
                id: gen_id_copy,
                trace_id: Some(self.trace_id.clone()),
                output,
                usage: usage_body,
                end_time: Some(now),
                status_message: finish_reason.or(options.status_message),
                level,
                version: options.version,
                prompt_name: options.prompt_name,
                prompt_version: options.prompt_version,
                cost_details: options.cost_details,
            },
        };
        match self.tx.try_send(event) {
            Ok(()) => {
                lf_debug!("GenerationHandle::end -> OK (enqueued)");
            }
            Err(mpsc::error::TrySendError::Full(_ev)) => {
                lf_debug!("GenerationHandle::end -> FULL (dropping GenerationUpdate)");
                tracing::warn!("Langfuse event buffer full, dropping event");
            }
            Err(mpsc::error::TrySendError::Closed(_ev)) => {
                lf_debug!("GenerationHandle::end -> CLOSED (dropping GenerationUpdate)");
                tracing::error!("Langfuse channel closed, event dropped");
            }
        }
    }
}

/// Options for ending a generation.
#[derive(Debug, Clone, Default)]
pub struct GenerationEndOptions {
    pub level: Option<SpanLevel>,
    pub status_message: Option<String>,
    pub version: Option<String>,
    pub prompt_name: Option<String>,
    pub prompt_version: Option<i32>,
    pub cost_details: Option<Value>,
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
        lf_debug!("BackgroundWorker::run STARTED, flush_interval_ms={}, max_batch_size={}",
            FLUSH_INTERVAL_MS, MAX_BATCH_SIZE);
        tracing::info!("Langfuse BackgroundWorker started");
        let mut interval = tokio::time::interval(std::time::Duration::from_millis(FLUSH_INTERVAL_MS));
        let mut event_count: u64 = 0;

        loop {
            tokio::select! {
                // Drain incoming events into the buffer.
                event = self.rx.recv() => {
                    match event {
                        Some(ev) => {
                            event_count += 1;
                            lf_debug!("BackgroundWorker recv event #{}, type={:?}, buffer_len={}",
                                event_count, extract_event_type(&ev), self.buffer.len() + 1);
                            self.buffer.push(ev);
                            if self.buffer.len() >= MAX_BATCH_SIZE {
                                lf_debug!("BackgroundWorker buffer reached MAX_BATCH_SIZE ({}), flushing",
                                    self.buffer.len());
                                self.flush().await;
                            }
                        }
                        None => {
                            lf_debug!("BackgroundWorker channel closed, total_events={}, flushing remaining {} events",
                                event_count, self.buffer.len());
                            // Channel closed — flush remaining and exit.
                            self.flush().await;
                            lf_debug!("BackgroundWorker::run EXITED (channel closed)");
                            return;
                        }
                    }
                }
                // Periodic flush.
                _ = interval.tick() => {
                    if !self.buffer.is_empty() {
                        lf_debug!("BackgroundWorker periodic flush tick, buffer_len={}", self.buffer.len());
                    }
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
        let payload_str = serde_json::to_string(&payload).unwrap_or_default();
        let payload_len = payload_str.len();
        let url = self.cfg.ingestion_url();

        lf_debug!("BackgroundWorker::flush batch_size={} payload_bytes={} url={}",
            count, payload_len, url);

        let result = self
            .http
            .post(&url)
            .header("Authorization", self.cfg.basic_auth())
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .await;

        match result {
            Ok(resp) => {
                let status = resp.status();
                let status_code = status.as_u16();
                let body_text = resp.text().await.unwrap_or_default();
                let body_truncated: String = body_text.chars().take(500).collect();

                if status.is_success() {
                    lf_debug!("BackgroundWorker::flush SUCCESS status={} response_body_len={} body={}",
                        status_code, body_text.len(), body_truncated);
                    tracing::info!("Langfuse flush succeeded: {} events sent", count);
                } else {
                    lf_debug!("BackgroundWorker::flush FAILED status={} body={}",
                        status_code, body_truncated);
                    tracing::warn!(
                        status = %status,
                        body = %body_truncated,
                        "Langfuse ingestion failed"
                    );
                }
            }
            Err(e) => {
                lf_debug!("BackgroundWorker::flush ERROR error={}", e);
                tracing::warn!(error = %e, "Langfuse ingestion request failed");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Debug helpers
// ---------------------------------------------------------------------------

/// Truncate a string to `max_len` chars for logging.
fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...<truncated>", &s[..max_len])
    }
}

/// Extract a short event type name from an IngestEvent for logging.
fn extract_event_type(event: &IngestEvent) -> &'static str {
    match event {
        IngestEvent::TraceCreate { .. } => "TraceCreate",
        IngestEvent::TraceUpdate { .. } => "TraceUpdate",
        IngestEvent::SpanCreate { .. } => "SpanCreate",
        IngestEvent::SpanUpdate { .. } => "SpanUpdate",
        IngestEvent::GenerationCreate { .. } => "GenerationCreate",
        IngestEvent::GenerationUpdate { .. } => "GenerationUpdate",
        IngestEvent::EventCreate { .. } => "EventCreate",
        IngestEvent::ScoreCreate { .. } => "ScoreCreate",
    }
}
