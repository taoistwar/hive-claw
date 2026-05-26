//! Request ID middleware (T092 / spec FR-021).
//!
//! - Reads incoming `X-Request-Id` header if present (so an upstream proxy
//!   can propagate a correlation ID across services).
//! - Otherwise mints a fresh UUID v4.
//! - Stores the value in request extensions so handlers can include it in
//!   structured tracing events.
//! - Echoes the value back as the `X-Request-Id` response header for
//!   client-side correlation.
//!
//! The matching `RequestId` type is the canonical key for retrieving the
//! value inside handlers via `axum::Extension`.

use axum::{
    body::Body,
    extract::Request,
    http::{header::HeaderName, HeaderValue},
    middleware::Next,
    response::Response,
};
use tracing::Instrument;
use uuid::Uuid;

pub const REQUEST_ID_HEADER: HeaderName = HeaderName::from_static("x-request-id");

#[derive(Debug, Clone)]
pub struct RequestId(pub String);

impl RequestId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

pub async fn request_id_middleware(mut request: Request<Body>, next: Next) -> Response {
    let incoming = request
        .headers()
        .get(&REQUEST_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let id = incoming.unwrap_or_else(|| Uuid::new_v4().to_string());
    let request_id = RequestId(id.clone());
    request.extensions_mut().insert(request_id);

    // Open a span carrying request_id so every log emitted while handling
    // this request inherits the correlation field. `.instrument()` propagates
    // the span across .await points (a plain `let _enter` would drop it).
    let span = tracing::info_span!("request", request_id = %id);
    let mut response = next.run(request).instrument(span).await;
    if let Ok(value) = HeaderValue::from_str(&id) {
        response.headers_mut().insert(REQUEST_ID_HEADER, value);
    }
    response
}
