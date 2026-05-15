use std::convert::Infallible;
use std::time::{Duration, Instant};

use axum::{
    extract::Request,
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{sse::Event, sse::KeepAlive, IntoResponse, Response, Sse},
    Json,
};
use chrono::Utc;
use futures::stream::{self, Stream};
use tracing::info;
use uuid::Uuid;

use crate::openresponses::{self, limits, AttachmentMeta, ErrorEnvelope};

const STREAM_CHUNK_DELAY: Duration = Duration::from_millis(8);

pub async fn handle(req: Request) -> Response {
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
        streaming_response(request_id, validated, started)
    } else {
        sync_response(request_id, validated, started)
    }
}

fn sync_response(
    request_id: String,
    req: openresponses::ValidatedRequest,
    started: Instant,
) -> Response {
    let response_id = format!("resp_{}", Uuid::new_v4().simple());
    let body = openresponses::stub::build_response(
        &response_id,
        &req.model,
        Utc::now().timestamp(),
        req.input_text.chars().count(),
        &req.attachments,
    );

    info!(
        request_id = %request_id,
        operation = "responses.create",
        outcome = "completed",
        duration_ms = started.elapsed().as_millis() as u64,
        stream = false,
        status_code = 200,
        attachment_count = req.attachments.len(),
        attachments_bytes = total_attachment_bytes(&req.attachments),
    );

    let mut response = (StatusCode::OK, Json(body)).into_response();
    response
        .headers_mut()
        .insert("x-request-id", header_value(&request_id));
    response
}

fn streaming_response(
    request_id: String,
    req: openresponses::ValidatedRequest,
    started: Instant,
) -> Response {
    let response_id = format!("resp_{}", Uuid::new_v4().simple());
    let created = Utc::now().timestamp();
    let model = req.model.clone();
    let input_chars = req.input_text.chars().count();
    let attachments = req.attachments.clone();

    let chunks: Vec<String> = openresponses::stub::stream_chunks(&attachments);
    let final_response = openresponses::stub::build_response(
        &response_id,
        &model,
        created,
        input_chars,
        &attachments,
    );

    let request_id_for_log = request_id.clone();
    let attachment_count = attachments.len();
    let attachments_bytes = total_attachment_bytes(&attachments);

    let stream = build_event_stream(response_id, model, created, chunks, final_response);

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

fn build_event_stream(
    response_id: String,
    model: String,
    created: i64,
    chunks: Vec<String>,
    final_response: openresponses::OpenResponse,
) -> impl Stream<Item = Result<Event, Infallible>> {
    enum Frame {
        Created,
        Delta(String),
        Completed,
        Done,
    }

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
                tokio::time::sleep(STREAM_CHUNK_DELAY).await;
            }
            let event = match next {
                Frame::Created => {
                    let payload = openresponses::CreatedPayload {
                        id: &response_id,
                        object: "response",
                        created,
                        model: &model,
                        status: openresponses::ResponseStatus::InProgress,
                    };
                    Event::default()
                        .event("response.created")
                        .json_data(&payload)
                        .expect("created payload is JSON-serialisable")
                }
                Frame::Delta(text) => {
                    let payload = openresponses::DeltaPayload {
                        id: &response_id,
                        delta: &text,
                    };
                    Event::default()
                        .event("response.output_text.delta")
                        .json_data(&payload)
                        .expect("delta payload is JSON-serialisable")
                }
                Frame::Completed => Event::default()
                    .event("response.completed")
                    .json_data(&final_response)
                    .expect("response payload is JSON-serialisable"),
                Frame::Done => Event::default().data("[DONE]"),
            };
            Some((
                Ok(event),
                (iter, response_id, model, created, final_response, true),
            ))
        },
    )
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
        .unwrap_or_else(|| Uuid::new_v4().to_string())
}

fn header_value(s: &str) -> HeaderValue {
    HeaderValue::from_str(s).unwrap_or_else(|_| HeaderValue::from_static("invalid"))
}

fn total_attachment_bytes(attachments: &[AttachmentMeta]) -> u64 {
    attachments.iter().map(|a| a.size_bytes as u64).sum()
}
