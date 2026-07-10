use std::convert::Infallible;
use std::sync::Arc;
use std::time::Instant;

use axum::{
    Json,
    extract::Request,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response, Sse, sse::Event, sse::KeepAlive},
};
use futures::stream::Stream;
use tracing::info;

use crate::agent_backend::{self, AgentBackend};
use crate::openresponses::{self, AttachmentMeta, ErrorEnvelope, limits};

const STREAM_CHUNK_DELAY: std::time::Duration = std::time::Duration::from_millis(8);

pub fn make_handler(
    agent: Arc<AgentBackend>,
) -> impl Fn(Request) -> std::pin::Pin<Box<dyn std::future::Future<Output = Response> + Send>> + Clone
{
    move |req: Request| {
        let agent = agent.clone();
        Box::pin(async move { handle(req, agent).await })
    }
}

async fn handle(req: Request, agent: Arc<AgentBackend>) -> Response {
    let started = Instant::now();
    let request_id = extract_or_generate_request_id(req.headers());

    let ct_ok = req
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|h| h.to_str().ok())
        .map(|s| s.to_ascii_lowercase().starts_with("application/json"))
        .unwrap_or(false);
    if !ct_ok {
        return finish_error(
            request_id,
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            ErrorEnvelope::invalid_request("Content-Type must be application/json"),
            false,
            started,
        );
    }

    let bytes = match axum::body::to_bytes(req.into_body(), limits::MAX_REQUEST_BYTES).await {
        Ok(b) => b,
        Err(_) => {
            return finish_error(
                request_id,
                StatusCode::PAYLOAD_TOO_LARGE,
                ErrorEnvelope::invalid_request(
                    "request body exceeds 8 MiB transport limit".to_string(),
                ),
                false,
                started,
            );
        }
    };

    let parsed: openresponses::OpenResponsesRequest = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(_) => {
            return finish_error(
                request_id,
                StatusCode::BAD_REQUEST,
                ErrorEnvelope::invalid_request("request body is not valid JSON"),
                false,
                started,
            );
        }
    };

    let validated = match openresponses::validate(parsed) {
        Ok(v) => v,
        Err(e) => {
            let status = if e.is_payload_too_large() {
                StatusCode::PAYLOAD_TOO_LARGE
            } else {
                StatusCode::BAD_REQUEST
            };
            return finish_error(
                request_id,
                status,
                ErrorEnvelope::invalid_request(e.message().to_string()),
                false,
                started,
            );
        }
    };

    if validated.stream {
        streaming_response(request_id, validated, started, agent).await
    } else {
        sync_response(request_id, validated, started, agent).await
    }
}

async fn sync_response(
    request_id: String,
    req: openresponses::ValidatedRequest,
    started: Instant,
    agent: Arc<AgentBackend>,
) -> Response {
    let body = agent.run_sync(&req, &request_id, started).await;

    let mut response = (StatusCode::OK, Json(body)).into_response();
    response
        .headers_mut()
        .insert("x-request-id", header_value(&request_id));
    response
}

async fn streaming_response(
    request_id: String,
    req: openresponses::ValidatedRequest,
    started: Instant,
    agent: Arc<AgentBackend>,
) -> Response {
    let chunks = agent_backend::build_stream_frames(&agent, &req, &request_id, started).await;
    let stream_state = agent_backend::prepare_stream_state(&req, chunks);

    let request_id_for_log = request_id.clone();
    let attachment_count = req.attachments.len();
    let attachments_bytes = total_attachment_bytes(&req.attachments);

    let stream = agent_backend::build_event_stream(stream_state);

    let body = Sse::new(stream).keep_alive(KeepAlive::default());
    let mut response = body.into_response();
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert("x-request-id", header_value(&request_id));

    info!(
        request_id = %request_id_for_log,
        operation = "responses.create",
        outcome = "completed",
        duration_ms = started.elapsed().as_millis() as u64,
        stream = true,
        status_code = 200,
        attachment_count = attachment_count,
        attachments_bytes = attachments_bytes,
    );

    response
}

fn finish_error(
    request_id: String,
    status: StatusCode,
    body: ErrorEnvelope,
    stream: bool,
    started: Instant,
) -> Response {
    let outcome = if status.is_client_error() {
        "validation_error"
    } else {
        "server_error"
    };
    info!(
        request_id = %request_id,
        operation = "responses.create",
        outcome = outcome,
        duration_ms = started.elapsed().as_millis() as u64,
        stream = stream,
        status_code = status.as_u16(),
    );

    let mut response = (status, Json(body)).into_response();
    response
        .headers_mut()
        .insert("x-request-id", header_value(&request_id));
    response
}

fn extract_or_generate_request_id(headers: &HeaderMap) -> String {
    headers
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
}

fn header_value(s: &str) -> HeaderValue {
    HeaderValue::from_str(s).unwrap_or_else(|_| HeaderValue::from_static("invalid"))
}

fn total_attachment_bytes(attachments: &[AttachmentMeta]) -> u64 {
    attachments.iter().map(|a| a.size_bytes as u64).sum()
}
