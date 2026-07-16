//! Agent backend for HiveClaw responses.
//!
//! Bridges `agent::AgentLoop` (via `cli::LoopBundle`) into the
//! `POST /v1/responses` handler, replacing the previous stub
//! implementation with a real nanobot-backed dialogue.

use std::sync::Arc;
use std::time::Instant;

use agent::AgentLoop;
use bus::InboundMessage;
use chrono::Local;
use tracing::info;
use uuid::Uuid;

use crate::config::Config;
use crate::openresponses::{
    self, AttachmentMeta, ContentItem, CreatedPayload, DeltaPayload, OpenResponse, OutputItem,
    ResponseStatus, Usage, ValidatedRequest,
};

/// The fully wired nanobot agent loop, or a stub for test/placeholder mode.
pub struct AgentBackend {
    pub agent: Option<Arc<AgentLoop>>,
}

impl AgentBackend {
    pub fn new(agent: Arc<AgentLoop>) -> Self {
        Self { agent: Some(agent) }
    }

    /// Create a stub backend that returns placeholder text.
    /// Useful for tests and for environments without LLM config.
    pub fn stub() -> Self {
        Self { agent: None }
    }

    pub async fn build(cfg: &Config) -> anyhow::Result<AgentBackend> {
        let resolved_config = cfg.nanobot_config.clone();
        let workspace = cfg.nanobot_workspace.clone();

        // Set the global config path for path helpers if a custom config is provided.
        if let Some(ref p) = resolved_config {
            config::loader::set_config_path(p);
        }

        // Load nanobot config.
        let nanobot_cfg = config::schema::Config::from_config(resolved_config.as_deref());
        let mut nanobot_cfg = nanobot_cfg
            .resolve_env_vars()
            .map_err(|e| anyhow::anyhow!("config resolve error: {e}"))?;

        // Apply workspace override if provided.
        if let Some(ref ws) = workspace {
            // Expand tilde if present.
            let ws_str = ws.to_string_lossy();
            let expanded = if ws_str.starts_with('~') {
                dirs::home_dir()
                    .map(|h| {
                        let rest = ws_str.trim_start_matches('~').trim_start_matches('/');
                        h.join(rest).to_string_lossy().into_owned()
                    })
                    .unwrap_or_else(|| ws_str.into_owned())
            } else {
                ws_str.into_owned()
            };
            nanobot_cfg.agents.defaults.workspace = expanded;
        }

        let bundle = cli::LoopBundle::build_agent_loop(&nanobot_cfg, None)
            .await
            .map_err(|e| anyhow::anyhow!("failed to initialise agent loop: {e}"))?;

        Ok(AgentBackend::new(Arc::new(bundle.agent)))
    }

    async fn run_inner(&self, req: &ValidatedRequest) -> String {
        match &self.agent {
            Some(agent) => {
                let inbound = build_inbound(&req.input_text, "inner");
                match agent.process_inbound(inbound).await {
                    Ok(result) => result.final_content.unwrap_or_default(),
                    Err(e) => {
                        tracing::error!("agent error: {e}");
                        String::new()
                    }
                }
            }
            None => openresponses::stub::build_text(&req.attachments),
        }
    }

    /// Run a synchronous (non-streaming) request through the agent.
    pub async fn run_sync(
        &self,
        req: &ValidatedRequest,
        request_id: &str,
        started: Instant,
    ) -> OpenResponse {
        let response_id = format!("resp_{}", Uuid::new_v4().simple());
        let created = chrono::Utc::now().timestamp();

        let content = self.run_inner(req).await;

        let duration_ms = started.elapsed().as_millis() as u64;
        log_sync(request_id, duration_ms, req);

        build_open_response(
            &response_id,
            &req.model,
            created,
            &content,
            &req.attachments,
        )
    }
}

/// Build streaming SSE frames from an agent response.
///
/// Since the agent runner doesn't expose per-token callbacks, we run
/// the agent synchronously and split the result into chunks matching
/// the OpenResponses streaming contract.
pub async fn build_stream_frames(
    backend: &AgentBackend,
    req: &ValidatedRequest,
    request_id: &str,
    started: Instant,
) -> Vec<String> {
    let content = backend.run_inner(req).await;

    let duration_ms = started.elapsed().as_millis() as u64;
    log_stream(request_id, duration_ms, req);

    split_into_chunks(&content, &req.attachments)
}

fn build_inbound(text: &str, response_id: &str) -> InboundMessage {
    InboundMessage {
        channel: "api".into(),
        sender_id: "hiveclaw".into(),
        chat_id: "default".into(),
        content: text.to_string(),
        timestamp: Local::now(),
        media: Vec::new(),
        metadata: Default::default(),
        session_key_override: Some(format!("hiveclaw:{response_id}")),
    }
}

fn build_open_response(
    response_id: &str,
    model: &str,
    created: i64,
    content: &str,
    attachments: &[AttachmentMeta],
) -> OpenResponse {
    let text = if content.is_empty() {
        openresponses::stub::build_text(attachments)
    } else {
        content.to_string()
    };
    let output_tokens = approx_tokens(&text);
    let input_chars = text.chars().count();

    OpenResponse {
        id: response_id.to_string(),
        object: "response",
        created,
        model: model.to_string(),
        status: ResponseStatus::Completed,
        output: vec![OutputItem {
            kind: "message",
            role: "assistant",
            content: vec![ContentItem {
                kind: "output_text",
                text,
            }],
        }],
        usage: Usage {
            input_tokens: input_chars as u32,
            output_tokens,
            total_tokens: input_chars as u32 + output_tokens,
        },
    }
}

/// Split the final text into streaming chunks.
/// If text is empty, fall back to stub chunks.
fn split_into_chunks(text: &str, attachments: &[AttachmentMeta]) -> Vec<String> {
    if text.is_empty() {
        return openresponses::stub::stream_chunks(attachments);
    }

    // Split into ~50 character chunks for natural streaming feel.
    const CHUNK_SIZE: usize = 50;
    let mut chunks = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let end = (i + CHUNK_SIZE).min(chars.len());
        chunks.push(chars[i..end].iter().collect());
        i = end;
    }
    // Ensure at least 2 chunks for streaming semantics.
    if chunks.len() < 2 && !text.is_empty() {
        let mid = text.len() / 2;
        chunks = vec![text[..mid].to_string(), text[mid..].to_string()];
    }
    chunks
}

pub struct StreamState {
    pub response_id: String,
    pub model: String,
    pub created: i64,
    pub chunks: Vec<String>,
    pub final_response: OpenResponse,
}

pub fn prepare_stream_state(req: &ValidatedRequest, chunks: Vec<String>) -> StreamState {
    let response_id = format!("resp_{}", Uuid::new_v4().simple());
    let created = chrono::Utc::now().timestamp();
    let final_response = build_open_response(
        &response_id,
        &req.model,
        created,
        &chunks.join(""),
        &req.attachments,
    );
    StreamState {
        response_id,
        model: req.model.clone(),
        created,
        chunks,
        final_response,
    }
}

/// Build the SSE event stream frames.
pub fn build_event_stream(
    state: StreamState,
) -> impl futures::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>> + Send
{
    use axum::response::sse::Event;
    use futures::stream;

    enum Frame {
        Created,
        Delta(String),
        Completed,
        Done,
    }

    let response_id = state.response_id.clone();
    let model = state.model.clone();
    let created = state.created;
    let chunks: Vec<String> = state.chunks.clone();
    let final_response = state.final_response.clone();

    let mut frames: Vec<Frame> = Vec::with_capacity(chunks.len() + 3);
    frames.push(Frame::Created);
    for c in chunks {
        frames.push(Frame::Delta(c));
    }
    frames.push(Frame::Completed);
    frames.push(Frame::Done);

    stream::unfold(
        (
            frames.into_iter(),
            response_id,
            model,
            created,
            final_response,
            false,
        ),
        move |(mut iter, response_id, model, created, final_response, sent_first)| async move {
            let next = iter.next()?;
            if sent_first {
                tokio::time::sleep(std::time::Duration::from_millis(8)).await;
            }
            let event = match next {
                Frame::Created => {
                    let payload = CreatedPayload {
                        id: &response_id,
                        object: "response",
                        created,
                        model: &model,
                        status: ResponseStatus::InProgress,
                    };
                    Event::default()
                        .event("response.created")
                        .json_data(&payload)
                        .expect("created payload serializable")
                }
                Frame::Delta(text) => {
                    let payload = DeltaPayload {
                        id: &response_id,
                        delta: &text,
                    };
                    Event::default()
                        .event("response.output_text.delta")
                        .json_data(&payload)
                        .expect("delta payload serializable")
                }
                Frame::Completed => Event::default()
                    .event("response.completed")
                    .json_data(&final_response)
                    .expect("completed payload serializable"),
                Frame::Done => Event::default().data("[DONE]"),
            };
            Some((
                Ok(event),
                (iter, response_id, model, created, final_response, true),
            ))
        },
    )
}

fn log_sync(request_id: &str, duration_ms: u64, req: &ValidatedRequest) {
    info!(
        request_id = %request_id,
        operation = "responses.create",
        outcome = "completed",
        duration_ms = duration_ms,
        stream = false,
        status_code = 200,
        attachment_count = req.attachments.len(),
        attachments_bytes = total_attachment_bytes(&req.attachments),
    );
}

fn log_stream(request_id: &str, duration_ms: u64, req: &ValidatedRequest) {
    info!(
        request_id = %request_id,
        operation = "responses.create",
        outcome = "completed",
        duration_ms = duration_ms,
        stream = true,
        status_code = 200,
        attachment_count = req.attachments.len(),
        attachments_bytes = total_attachment_bytes(&req.attachments),
    );
}

fn total_attachment_bytes(attachments: &[AttachmentMeta]) -> u64 {
    attachments.iter().map(|a| a.size_bytes as u64).sum()
}

fn approx_tokens(text: &str) -> u32 {
    text.chars().count() as u32
}
