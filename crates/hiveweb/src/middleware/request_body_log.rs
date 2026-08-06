use axum::extract::Request;
use axum::http::{Method, Uri, header};
use axum::middleware::Next;
use axum::response::Response;

pub async fn log_request_body_middleware(request: Request, next: Next) -> Response {
    if crate::app_mode::get().is_production() || request.method() != Method::POST {
        return next.run(request).await;
    }

    let content_length = request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());
    emit_post_request_metadata(request.uri(), content_length);

    next.run(request).await
}

fn emit_post_request_metadata(_uri: &Uri, content_length: Option<u64>) {
    tracing::debug!(method = "POST", content_length, "POST request metadata");
}

#[cfg(test)]
mod tests {
    use super::emit_post_request_metadata;
    use axum::http::Uri;
    use std::sync::{Arc, Mutex};

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

    #[test]
    fn post_metadata_omits_query_and_body() {
        let writer = TraceWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_target(false)
            .with_max_level(tracing::Level::DEBUG)
            .with_writer(writer.clone())
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);

        let uri: Uri = "/api/reset-token/PATH_TOKEN_SENTINEL?token=QUERY_TOKEN_SENTINEL"
            .parse()
            .unwrap();
        emit_post_request_metadata(&uri, Some(20));

        let output =
            String::from_utf8(writer.0.lock().expect("trace buffer poisoned").clone()).unwrap();
        assert!(output.contains("POST request metadata"));
        assert!(
            output.contains("content_length=20"),
            "missing content length metadata: {output}"
        );
        assert!(!output.contains("PATH_TOKEN_SENTINEL"));
        assert!(!output.contains("QUERY_TOKEN_SENTINEL"));
        assert!(!output.contains("BODY_SENTINEL"));
    }
}
