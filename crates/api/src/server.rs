//! axum-based HTTP server implementing the OpenAI-compatible endpoints.

use std::collections::HashMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{FromRequest, Multipart, Request, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::stream::{self, Stream, StreamExt};
use log::{error, info, warn};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tower_http::limit::RequestBodyLimitLayer;
use uuid::Uuid;

use crate::agent::{ApiAgent, ApiAnswer, ApiRequest};
use crate::media::{FileSizeExceeded, MAX_FILE_SIZE, safe_filename, save_base64_data_url};
use crate::types::{ChatCompletionRequest, ModelInfo, ModelsList, assistant_completion};

const API_CHAT_ID: &str = "default";
const API_CHANNEL: &str = "api";
const EMPTY_FINAL_RESPONSE_MESSAGE: &str = "The assistant produced no response. Please try again.";

/// Configuration accepted by [`serve`].
#[derive(Debug, Clone)]
pub struct ApiServerConfig {
    pub bind_addr: SocketAddr,
    pub model_name: String,
    pub request_timeout: Duration,
    /// Where saved base64 / multipart uploads land. Created on demand.
    pub media_dir: PathBuf,
    /// Per-request body cap (bytes). Defaults to 20 MiB to match Python.
    pub max_body_size: usize,
}

impl ApiServerConfig {
    pub fn new(bind_addr: SocketAddr, media_dir: PathBuf) -> Self {
        Self {
            bind_addr,
            model_name: "nanobot".into(),
            request_timeout: Duration::from_secs(120),
            media_dir,
            max_body_size: 20 * 1024 * 1024,
        }
    }
    pub fn with_model_name(mut self, model_name: String) -> Self {
        self.model_name = model_name;
        self
    }
    pub fn with_request_timeout(mut self, request_timeout: Duration) -> Self {
        self.request_timeout = request_timeout;
        self
    }
    pub fn with_max_body_size(mut self, max_body_size: usize) -> Self {
        self.max_body_size = max_body_size;
        self
    }
    pub fn with_media_dir(mut self, media_dir: PathBuf) -> Self {
        self.media_dir = media_dir;
        self
    }
}

/// Shared mutable state held by axum handlers.
///
/// `session_locks` matches the Python server's per-session mutex map —
/// an inbound request that reuses an already-busy `session_id` will
/// serialise behind the earlier one.
pub struct ServerState {
    pub agent: Arc<dyn ApiAgent>,
    pub config: ApiServerConfig,
    session_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

impl ServerState {
    pub fn new(agent: Arc<dyn ApiAgent>, config: ApiServerConfig) -> Arc<Self> {
        Arc::new(Self {
            agent,
            config,
            session_locks: Mutex::new(HashMap::new()),
        })
    }

    async fn session_lock(&self, key: &str) -> Arc<Mutex<()>> {
        let mut guard = self.session_locks.lock().await;
        guard
            .entry(key.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }
}

/// Build the axum router. Exposed so tests can mount it on an in-process
/// server without calling [`serve`].
pub fn build_router(state: Arc<ServerState>) -> Router {
    let limit = state.config.max_body_size;
    Router::new()
        .route("/v1/chat/completions", post(handle_chat_completions))
        .route("/v1/models", get(handle_models))
        .route("/health", get(handle_health))
        .layer(RequestBodyLimitLayer::new(limit))
        .with_state(state)
}

/// Start the server and block until it shuts down.
pub async fn serve(
    agent: Arc<dyn ApiAgent>,
    config: ApiServerConfig,
) -> Result<(), std::io::Error> {
    let addr = config.bind_addr;
    let state = ServerState::new(agent, config);
    let router = build_router(state);
    let listener = TcpListener::bind(addr).await?;
    info!("api server listening on {addr}");
    axum::serve(listener, router).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn handle_health() -> Json<Value> {
    Json(json!({"status": "ok"}))
}

async fn handle_models(State(state): State<Arc<ServerState>>) -> Json<ModelsList> {
    Json(ModelsList {
        object: "list",
        data: vec![ModelInfo {
            id: state.config.model_name.clone(),
            object: "model",
            created: 0,
            owned_by: "nanobot",
        }],
    })
}

async fn handle_chat_completions(
    State(state): State<Arc<ServerState>>,
    request: Request,
) -> Response {
    let content_type = request
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    let parsed = if content_type.starts_with("multipart/") {
        parse_multipart(request).await
    } else {
        parse_json(request.into_body()).await
    };

    let parsed = match parsed {
        Ok(p) => p,
        Err(ParseError::BadRequest(msg)) => {
            return error_json(StatusCode::BAD_REQUEST, &msg, "invalid_request_error");
        }
        Err(ParseError::PayloadTooLarge(msg)) => {
            return error_json(StatusCode::PAYLOAD_TOO_LARGE, &msg, "invalid_request_error");
        }
        Err(ParseError::Internal(msg)) => {
            error!("api: unexpected parse error: {msg}");
            return error_json(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Internal server error",
                "server_error",
            );
        }
    };

    // Model whitelist — Python rejects requests that name another model.
    if let Some(requested) = parsed.model.as_deref() {
        if requested != state.config.model_name {
            return error_json(
                StatusCode::BAD_REQUEST,
                &format!(
                    "Only configured model '{}' is available",
                    state.config.model_name
                ),
                "invalid_request_error",
            );
        }
    }

    let req = ApiRequest {
        content: parsed.text,
        media: parsed.media,
        session_id: parsed.session_id,
        model: parsed.model,
        channel: API_CHANNEL.into(),
        chat_id: API_CHAT_ID.into(),
    };
    let session_key = req.session_key();
    info!(
        "api request session={} media={} stream={}",
        session_key,
        req.media.len(),
        parsed.stream
    );

    let lock = state.session_lock(&session_key).await;

    if parsed.stream {
        stream_response(state.clone(), req, lock).await
    } else {
        non_stream_response(state, req, lock).await
    }
}

// ---------------------------------------------------------------------------
// Body parsing
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct ParsedRequest {
    text: String,
    media: Vec<PathBuf>,
    session_id: Option<String>,
    model: Option<String>,
    stream: bool,
}

#[derive(Debug)]
enum ParseError {
    BadRequest(String),
    PayloadTooLarge(String),
    Internal(String),
}

impl From<FileSizeExceeded> for ParseError {
    fn from(e: FileSizeExceeded) -> Self {
        Self::PayloadTooLarge(e.0)
    }
}

async fn parse_json(body: axum::body::Body) -> Result<ParsedRequest, ParseError> {
    let bytes = axum::body::to_bytes(body, usize::MAX)
        .await
        .map_err(|e| ParseError::BadRequest(format!("failed to read body: {e}")))?;
    let json: ChatCompletionRequest = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(_) => return Err(ParseError::BadRequest("Invalid JSON body".into())),
    };
    let (text, media) = extract_json_content(&json)?;
    Ok(ParsedRequest {
        text,
        media,
        session_id: json.session_id,
        model: json.model,
        stream: json.stream,
    })
}

fn extract_json_content(req: &ChatCompletionRequest) -> Result<(String, Vec<PathBuf>), ParseError> {
    if req.messages.len() != 1 {
        return Err(ParseError::BadRequest(
            "Only a single user message is supported".into(),
        ));
    }
    let msg = &req.messages[0];
    if msg.role != "user" {
        return Err(ParseError::BadRequest(
            "Only a single user message is supported".into(),
        ));
    }

    let media_dir = std::env::current_dir()
        .map(|d| d.join("media").join("api"))
        .unwrap_or_else(|_| PathBuf::from("/tmp/rustbot_api_media"));

    let mut media_paths: Vec<PathBuf> = Vec::new();

    let text = match &msg.content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => {
            let mut chunks: Vec<String> = Vec::new();
            for part in parts {
                let Some(obj) = part.as_object() else {
                    continue;
                };
                match obj.get("type").and_then(|v| v.as_str()) {
                    Some("text") => {
                        if let Some(t) = obj.get("text").and_then(|v| v.as_str()) {
                            chunks.push(t.into());
                        }
                    }
                    Some("image_url") => {
                        let url = obj
                            .get("image_url")
                            .and_then(|v| v.get("url"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        if url.starts_with("data:") {
                            if let Some(path) = save_base64_data_url(url, &media_dir) {
                                media_paths.push(path);
                            }
                        } else if !url.is_empty() {
                            return Err(ParseError::BadRequest(
                                "Remote image URLs are not supported. \
                                 Use base64 data URLs or upload files via multipart/form-data."
                                    .into(),
                            ));
                        }
                    }
                    _ => {}
                }
            }
            chunks.join(" ")
        }
        _ => return Err(ParseError::BadRequest("Invalid content format".into())),
    };
    Ok((text, media_paths))
}

async fn parse_multipart(request: Request) -> Result<ParsedRequest, ParseError> {
    let mut multipart = <Multipart as FromRequest<()>>::from_request(request, &())
        .await
        .map_err(|e| ParseError::BadRequest(format!("invalid multipart: {e}")))?;

    let media_dir = std::env::current_dir()
        .map(|d| d.join("media").join("api"))
        .unwrap_or_else(|_| PathBuf::from("/tmp/rustbot_api_media"));

    let mut text = String::new();
    let mut session_id: Option<String> = None;
    let mut model: Option<String> = None;
    let mut media_paths: Vec<PathBuf> = Vec::new();

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ParseError::Internal(format!("multipart next: {e}")))?
    {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "message" => {
                let bytes = field
                    .bytes()
                    .await
                    .map_err(|e| ParseError::Internal(format!("multipart message: {e}")))?;
                text = String::from_utf8_lossy(&bytes).to_string();
            }
            "session_id" => {
                let bytes = field
                    .bytes()
                    .await
                    .map_err(|e| ParseError::Internal(format!("multipart session_id: {e}")))?;
                session_id = Some(String::from_utf8_lossy(&bytes).trim().to_string());
            }
            "model" => {
                let bytes = field
                    .bytes()
                    .await
                    .map_err(|e| ParseError::Internal(format!("multipart model: {e}")))?;
                model = Some(String::from_utf8_lossy(&bytes).trim().to_string());
            }
            "files" => {
                let filename = field.file_name().unwrap_or("upload.bin").to_string();
                let bytes = field
                    .bytes()
                    .await
                    .map_err(|e| ParseError::Internal(format!("multipart files: {e}")))?;
                if bytes.len() > MAX_FILE_SIZE {
                    return Err(FileSizeExceeded(format!(
                        "File '{filename}' exceeds {}MB limit",
                        MAX_FILE_SIZE / (1024 * 1024)
                    ))
                    .into());
                }
                std::fs::create_dir_all(&media_dir)
                    .map_err(|e| ParseError::Internal(format!("mkdir media: {e}")))?;
                let base = safe_filename(&filename);
                let prefix = Uuid::new_v4().simple().to_string();
                let out_path = media_dir.join(format!("{}_{base}", &prefix[..12]));
                std::fs::write(&out_path, &bytes)
                    .map_err(|e| ParseError::Internal(format!("write media: {e}")))?;
                media_paths.push(out_path);
            }
            _ => {}
        }
    }

    let text = if text.is_empty() {
        "请分析上传的文件".to_string()
    } else {
        text
    };

    // Multipart path never sets `stream`.
    Ok(ParsedRequest {
        text,
        media: media_paths,
        session_id: session_id.filter(|s| !s.is_empty()),
        model: model.filter(|s| !s.is_empty()),
        stream: false,
    })
}

// ---------------------------------------------------------------------------
// Response paths
// ---------------------------------------------------------------------------

async fn non_stream_response(
    state: Arc<ServerState>,
    req: ApiRequest,
    lock: Arc<Mutex<()>>,
) -> Response {
    let model_name = state.config.model_name.clone();
    let timeout = state.config.request_timeout;
    let session_key = req.session_key();

    let _guard = lock.lock().await;
    let first = tokio::time::timeout(timeout, state.agent.generate(req.clone())).await;
    let first = match first {
        Ok(Ok(a)) => a,
        Ok(Err(e)) => {
            error!("api: agent error for {session_key}: {e}");
            return error_json(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Internal server error",
                "server_error",
            );
        }
        Err(_) => {
            return error_json(
                StatusCode::GATEWAY_TIMEOUT,
                &format!("Request timed out after {}s", timeout.as_secs()),
                "server_error",
            );
        }
    };

    let text = if is_blank(&first.content) {
        warn!("api: empty response for {session_key}, retrying once");
        match tokio::time::timeout(timeout, state.agent.generate(req)).await {
            Ok(Ok(ApiAnswer { content })) if !is_blank(&content) => content,
            _ => EMPTY_FINAL_RESPONSE_MESSAGE.to_string(),
        }
    } else {
        first.content
    };

    let id = format!("chatcmpl-{}", &Uuid::new_v4().simple().to_string()[..12]);
    Json(assistant_completion(id, model_name, text)).into_response()
}

async fn stream_response(
    state: Arc<ServerState>,
    req: ApiRequest,
    lock: Arc<Mutex<()>>,
) -> Response {
    let model_name = state.config.model_name.clone();
    let chunk_id = format!("chatcmpl-{}", &Uuid::new_v4().simple().to_string()[..12]);
    let timeout = state.config.request_timeout;

    // Collect tokens under the session lock, then replay as SSE.
    // (Real per-token streaming will arrive once the runner gains a
    // delta callback; this mirrors Python's user-visible contract.)
    struct Collector(Vec<String>);
    impl crate::agent::StreamSink for Collector {
        fn on_delta(&mut self, d: &str) {
            self.0.push(d.to_string());
        }
    }
    let mut sink = Collector(Vec::new());
    let content: String = {
        let _guard = lock.lock().await;
        let result =
            tokio::time::timeout(timeout, state.agent.generate_stream(req, &mut sink)).await;
        match result {
            Ok(Ok(ans)) => ans.content,
            Ok(Err(e)) => {
                error!("api: agent stream error: {e}");
                format!("[error] {e}")
            }
            Err(_) => format!("[error] request timed out after {}s", timeout.as_secs()),
        }
    };
    let mut chunks = sink.0;

    // If the agent emitted no deltas (default impl didn't supply any),
    // fall back to the final content as a single delta.
    if chunks.is_empty() && !content.is_empty() {
        chunks.push(content);
    }
    let model_clone = model_name.clone();
    let id_clone = chunk_id.clone();
    let event_stream = make_sse_stream(chunks, model_clone, id_clone);

    Sse::new(event_stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

fn make_sse_stream(
    chunks: Vec<String>,
    model: String,
    chunk_id: String,
) -> impl Stream<Item = Result<Event, Infallible>> {
    let finish = sse_chunk_event(None, &model, &chunk_id, Some("stop"));
    let done = Event::default().data("[DONE]");

    let body_events: Vec<Result<Event, Infallible>> = chunks
        .into_iter()
        .map(move |c| Ok(sse_chunk_event(Some(c), &model, &chunk_id, None)))
        .collect();

    let tail: Vec<Result<Event, Infallible>> = vec![Ok(finish), Ok(done)];
    stream::iter(body_events).chain(stream::iter(tail)).boxed()
}

fn sse_chunk_event(
    delta: Option<String>,
    model: &str,
    chunk_id: &str,
    finish_reason: Option<&str>,
) -> Event {
    let delta_field = match delta {
        Some(ref s) if !s.is_empty() => json!({"content": s}),
        _ => json!({}),
    };
    let payload = json!({
        "id": chunk_id,
        "object": "chat.completion.chunk",
        "created": chrono::Utc::now().timestamp(),
        "model": model,
        "choices": [{
            "index": 0,
            "delta": delta_field,
            "finish_reason": finish_reason,
        }],
    });
    Event::default().data(payload.to_string())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn error_json(status: StatusCode, message: &str, err_type: &str) -> Response {
    let body = json!({
        "error": {
            "message": message,
            "type": err_type,
            "code": status.as_u16(),
        }
    });
    (status, Json(body)).into_response()
}

fn is_blank(s: &str) -> bool {
    s.trim().is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Echo;

    #[async_trait::async_trait]
    impl ApiAgent for Echo {
        async fn generate(&self, req: ApiRequest) -> Result<ApiAnswer, String> {
            Ok(ApiAnswer {
                content: format!("echo: {}", req.content),
            })
        }
    }

    fn make_state() -> Arc<ServerState> {
        let cfg = ApiServerConfig::new(
            "127.0.0.1:0".parse().unwrap(),
            std::env::temp_dir().join("rustbot_api_test"),
        );
        ServerState::new(Arc::new(Echo), cfg)
    }

    #[tokio::test]
    async fn health_returns_ok() {
        let res = handle_health().await;
        assert_eq!(res.0["status"], "ok");
    }

    #[tokio::test]
    async fn models_advertises_configured_model() {
        let state = make_state();
        let list = handle_models(State(state.clone())).await;
        assert_eq!(list.object, "list");
        assert_eq!(list.data[0].id, "nanobot");
    }

    #[tokio::test]
    async fn extract_json_content_rejects_multi_message() {
        let req = ChatCompletionRequest {
            model: None,
            messages: vec![
                crate::types::ChatMessage {
                    role: "user".into(),
                    content: Value::String("a".into()),
                },
                crate::types::ChatMessage {
                    role: "user".into(),
                    content: Value::String("b".into()),
                },
            ],
            stream: false,
            session_id: None,
        };
        let err = extract_json_content(&req);
        assert!(matches!(err, Err(ParseError::BadRequest(_))));
    }

    #[tokio::test]
    async fn extract_json_content_rejects_remote_image_url() {
        let req = ChatCompletionRequest {
            model: None,
            messages: vec![crate::types::ChatMessage {
                role: "user".into(),
                content: json!([
                    {"type":"text","text":"describe"},
                    {"type":"image_url","image_url":{"url":"https://example.com/a.png"}}
                ]),
            }],
            stream: false,
            session_id: None,
        };
        assert!(matches!(
            extract_json_content(&req),
            Err(ParseError::BadRequest(_))
        ));
    }

    #[tokio::test]
    async fn extract_json_content_flattens_text_parts() {
        let req = ChatCompletionRequest {
            model: None,
            messages: vec![crate::types::ChatMessage {
                role: "user".into(),
                content: json!([
                    {"type":"text","text":"hello"},
                    {"type":"text","text":"world"},
                ]),
            }],
            stream: false,
            session_id: None,
        };
        let (text, media) = extract_json_content(&req).unwrap();
        assert_eq!(text, "hello world");
        assert!(media.is_empty());
    }

    #[tokio::test]
    async fn full_round_trip_over_tcp() {
        let state = make_state();
        // Bind to an ephemeral port.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let router = build_router(state.clone());
        let server = tokio::spawn(async move { axum::serve(listener, router).await });

        // /health
        let client = reqwest::Client::new();
        let health: Value = client
            .get(format!("http://{addr}/health"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(health["status"], "ok");

        // /v1/models
        let models: Value = client
            .get(format!("http://{addr}/v1/models"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(models["data"][0]["id"], "nanobot");

        // /v1/chat/completions (non-streaming)
        let body = json!({
            "model": "nanobot",
            "messages": [{"role":"user","content":"ping"}]
        });
        let chat: Value = client
            .post(format!("http://{addr}/v1/chat/completions"))
            .json(&body)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(chat["choices"][0]["message"]["content"], "echo: ping");
        assert_eq!(chat["object"], "chat.completion");

        // /v1/chat/completions (wrong model)
        let bad_body = json!({
            "model": "other",
            "messages": [{"role":"user","content":"ping"}]
        });
        let bad = client
            .post(format!("http://{addr}/v1/chat/completions"))
            .json(&bad_body)
            .send()
            .await
            .unwrap();
        assert_eq!(bad.status(), 400);

        server.abort();
    }

    #[test]
    fn sse_chunk_event_has_payload() {
        let ev = sse_chunk_event(Some("hi".into()), "m", "id", None);
        // Event is opaque; hit its Debug to ensure it doesn't panic.
        let _ = format!("{ev:?}");
    }
}
