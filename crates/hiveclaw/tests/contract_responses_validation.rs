//! Contract test for validation rules in
//! `specs/001-hiveclaw-hivegui/contracts/openresponses-v1.md` §Validation rules.

use hiveclaw::http;
use serde_json::json;
use tokio::net::TcpListener;

async fn spawn_app() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, http::router()).await;
    });
    format!("http://{addr}")
}

async fn assert_invalid(body_json: serde_json::Value, status: u16, expect_msg: &str) {
    let base = spawn_app().await;
    let resp = reqwest::Client::new()
        .post(format!("{base}/v1/responses"))
        .header("content-type", "application/json")
        .body(body_json.to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status().as_u16(),
        status,
        "expected {status} for body {body_json}"
    );
    let parsed: serde_json::Value = resp.json().await.unwrap();
    let kind = parsed
        .pointer("/error/type")
        .and_then(|v| v.as_str())
        .unwrap();
    let msg = parsed
        .pointer("/error/message")
        .and_then(|v| v.as_str())
        .unwrap();
    assert_eq!(kind, "invalid_request_error");
    assert!(
        msg.contains(expect_msg),
        "message {msg:?} does not contain {expect_msg:?}"
    );
}

#[tokio::test]
async fn rejects_missing_model() {
    assert_invalid(json!({"input": "hi"}), 400, "field 'model' is required").await;
}

#[tokio::test]
async fn rejects_non_openclaw_model() {
    assert_invalid(
        json!({"model": "gpt-4", "input": "hi"}),
        400,
        "openclaw:<agent-id>",
    )
    .await;
}

#[tokio::test]
async fn rejects_missing_input() {
    assert_invalid(
        json!({"model": "openclaw:x"}),
        400,
        "field 'input' is required",
    )
    .await;
}

#[tokio::test]
async fn rejects_empty_input_string() {
    assert_invalid(
        json!({"model": "openclaw:x", "input": ""}),
        400,
        "field 'input' is required",
    )
    .await;
}

#[tokio::test]
async fn rejects_non_boolean_stream() {
    assert_invalid(
        json!({"model": "openclaw:x", "input": "hi", "stream": "yes"}),
        400,
        "field 'stream' must be a boolean",
    )
    .await;
}

#[tokio::test]
async fn rejects_zero_max_output_tokens() {
    assert_invalid(
        json!({"model": "openclaw:x", "input": "hi", "max_output_tokens": 0}),
        400,
        "field 'max_output_tokens' must be a positive integer",
    )
    .await;
}

#[tokio::test]
async fn rejects_non_json_content_type() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, http::router()).await;
    });
    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/v1/responses"))
        .header("content-type", "text/plain")
        .body("hello")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 415);
    let parsed: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        parsed.pointer("/error/type").and_then(|v| v.as_str()),
        Some("invalid_request_error")
    );
    let msg = parsed
        .pointer("/error/message")
        .and_then(|v| v.as_str())
        .unwrap();
    assert!(msg.contains("application/json"));
}

#[tokio::test]
async fn rejects_invalid_json_body() {
    let base = spawn_app().await;
    let resp = reqwest::Client::new()
        .post(format!("{base}/v1/responses"))
        .header("content-type", "application/json")
        .body("{not json")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 400);
    let parsed: serde_json::Value = resp.json().await.unwrap();
    let msg = parsed
        .pointer("/error/message")
        .and_then(|v| v.as_str())
        .unwrap();
    assert!(msg.contains("not valid JSON"));
}
