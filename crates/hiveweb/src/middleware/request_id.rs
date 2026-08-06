//! Request ID middleware (T092 / spec FR-021).
//!
//! - Accepts an incoming `X-Request-Id` only when it is a canonical non-nil
//!   UUID; canonicalizes it to lowercase.
//! - Otherwise mints a fresh UUID v4 without logging the rejected header.
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
    http::{HeaderValue, header::HeaderName},
    middleware::Next,
    response::Response,
};
use tracing::Instrument;
use uuid::Uuid;

pub const REQUEST_ID_HEADER: HeaderName = HeaderName::from_static("x-request-id");

/// The correlation id for the current HTTP request. Stored in `Request`
/// extensions so any handler can pull it via `Extension<RequestId>`.
///
/// The inner field is `pub` to support that ergonomic access; it is not
/// currently read inside this crate, hence `#[allow(dead_code)]`.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct RequestId(pub String);

fn canonical_client_request_id(raw: &str) -> Option<String> {
    // Only the canonical hyphenated UUID shape is accepted at this trust
    // boundary. `Uuid::parse_str` intentionally accepts several looser forms,
    // so enforce the fixed length/positions/ASCII alphabet first.
    if raw.len() != 36 {
        return None;
    }
    for (index, byte) in raw.bytes().enumerate() {
        let valid = if matches!(index, 8 | 13 | 18 | 23) {
            byte == b'-'
        } else {
            byte.is_ascii_hexdigit()
        };
        if !valid {
            return None;
        }
    }

    let id = Uuid::parse_str(raw).ok()?;
    (!id.is_nil()).then(|| id.hyphenated().to_string())
}

pub async fn request_id_middleware(mut request: Request<Body>, next: Next) -> Response {
    let incoming = request
        .headers()
        .get(&REQUEST_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(canonical_client_request_id);

    let id = incoming.unwrap_or_else(|| Uuid::new_v4().to_string());
    let request_id = RequestId(id.clone());
    // Replace every downstream view of the untrusted header before auth,
    // handlers, or inner middleware can inspect it.
    request.headers_mut().insert(
        REQUEST_ID_HEADER,
        HeaderValue::from_str(&id).expect("canonical UUID is a valid header value"),
    );
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

#[cfg(test)]
mod tests {
    use super::{REQUEST_ID_HEADER, canonical_client_request_id, request_id_middleware};
    use axum::{
        Router,
        body::Body,
        http::{HeaderMap, Request},
        middleware,
        routing::get,
    };
    use http_body_util::BodyExt;
    use std::sync::{Arc, Mutex};
    use tower::ServiceExt;
    use uuid::Uuid;

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

    #[tokio::test]
    async fn malicious_client_request_id_is_replaced_before_echo_and_tracing() {
        const SENTINEL: &str = "authorization=REQUEST_ID_SECRET_SENTINEL";

        let writer = TraceWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_target(false)
            .with_writer(writer.clone())
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);

        let app = Router::new()
            .route(
                "/",
                get(|headers: HeaderMap| async move {
                    let handler_request_id = headers
                        .get(REQUEST_ID_HEADER)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string();
                    tracing::info!(
                        request_id = %handler_request_id,
                        "request-id-test-handler"
                    );
                    handler_request_id
                }),
            )
            .layer(middleware::from_fn(request_id_middleware));
        let response = app
            .oneshot(
                Request::get("/")
                    .header(REQUEST_ID_HEADER, SENTINEL)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let echoed = response
            .headers()
            .get(REQUEST_ID_HEADER)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert!(Uuid::parse_str(&echoed).is_ok(), "echoed id: {echoed}");
        assert_ne!(echoed, SENTINEL);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(
            std::str::from_utf8(&body).unwrap(),
            echoed,
            "handler must see the same normalized id that is echoed"
        );

        let output =
            String::from_utf8(writer.0.lock().expect("trace buffer poisoned").clone()).unwrap();
        assert!(output.contains("request-id-test-handler"));
        assert!(!output.contains(SENTINEL), "unsafe trace output: {output}");
    }

    #[tokio::test]
    async fn canonical_uuid_request_id_is_preserved_and_echoed() {
        const REQUEST_ID: &str = "d3c43d20-65f8-4f50-8f80-c526c127c6cc";
        let app = Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(middleware::from_fn(request_id_middleware));
        let response = app
            .oneshot(
                Request::get("/")
                    .header(REQUEST_ID_HEADER, REQUEST_ID)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            response
                .headers()
                .get(REQUEST_ID_HEADER)
                .unwrap()
                .to_str()
                .unwrap(),
            REQUEST_ID
        );
    }

    #[test]
    fn client_request_id_contract_is_canonical_non_nil_uuid_only() {
        assert_eq!(
            canonical_client_request_id("D3C43D20-65F8-4F50-8F80-C526C127C6CC").as_deref(),
            Some("d3c43d20-65f8-4f50-8f80-c526c127c6cc")
        );
        assert!(canonical_client_request_id("d3c43d2065f84f508f80c526c127c6cc").is_none());
        assert!(canonical_client_request_id("00000000-0000-0000-0000-000000000000").is_none());
        assert!(canonical_client_request_id("request_123-abc").is_none());
    }
}
