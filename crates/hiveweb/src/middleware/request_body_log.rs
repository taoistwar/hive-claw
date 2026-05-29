use axum::body::Body;
use axum::extract::Request;
use axum::http::Method;
use axum::middleware::Next;
use axum::response::Response;
use http_body_util::BodyExt;
use serde_json::Value;

pub async fn log_request_body_middleware(
    request: Request,
    next: Next,
) -> Response {
    let is_dev = std::env::var("APP_ENV")
        .map(|v| v == "development" || v == "dev")
        .unwrap_or(true);

    if !is_dev || request.method() != Method::POST {
        return next.run(request).await;
    }

    let uri = request.uri().to_string();
    let (parts, body) = request.into_parts();

    let body_bytes = match body.collect().await {
        Ok(buf) => buf.to_bytes(),
        Err(e) => {
            tracing::debug!(error = %e, "failed to read request body");
            let req = Request::from_parts(parts, Body::empty());
            return next.run(req).await;
        }
    };

    let body_str = String::from_utf8_lossy(&body_bytes);

    if !body_str.is_empty() {
        let json_log = if body_str.len() > 4096 {
            match serde_json::from_str::<Value>(&body_str) {
                Ok(_) => {
                    let truncated = format!("{}...(truncated, original {} bytes)", 
                        &body_str[..4096], body_str.len());
                    serde_json::to_string(&serde_json::json!({
                        "_truncated": true,
                        "preview": truncated
                    })).unwrap_or_else(|_| body_str.to_string())
                }
                Err(_) => format!("{}...(truncated, original {} bytes)", 
                    &body_str[..4096], body_str.len()),
            }
        } else {
            match serde_json::from_str::<Value>(&body_str) {
                Ok(_) => serde_json::to_string(&body_str)
                    .unwrap_or_else(|_| body_str.to_string()),
                Err(_) => body_str.to_string(),
            }
        };

        tracing::debug!(
            method = "POST",
            uri = %uri,
            body = %json_log,
            "POST request body"
        );
    }

    let req = Request::from_parts(parts, Body::from(body_bytes));
    next.run(req).await
}
