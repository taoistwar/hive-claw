//! Contract test for `POST /v1/responses` in synchronous mode.
//! Asserts the response schema documented in
//! `specs/001-hiveclaw-hivegui/contracts/openresponses-v1.md` §Response — synchronous.

use std::time::{Duration, Instant};

use hiveclaw::http;
use serde_json::json;
use tokio::net::TcpListener;

async fn spawn_app() -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, http::router()).await;
    });
    (format!("http://{addr}"), handle)
}

#[tokio::test]
async fn sync_response_matches_contract() {
    let (base, _h) = spawn_app().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{base}/v1/responses"))
        .header("content-type", "application/json")
        .json(&json!({
            "model": "openclaw:hiveclaw-placeholder-v1",
            "input": "hello",
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(
        resp.headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or(""),
        "application/json"
    );
    assert!(resp.headers().get("x-request-id").is_some());

    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body.get("id").and_then(|v| v.as_str()).is_some());
    assert_eq!(
        body.get("object").and_then(|v| v.as_str()),
        Some("response")
    );
    assert!(body.get("created").and_then(|v| v.as_i64()).is_some());
    assert_eq!(
        body.get("model").and_then(|v| v.as_str()),
        Some("openclaw:hiveclaw-placeholder-v1")
    );
    assert_eq!(
        body.get("status").and_then(|v| v.as_str()),
        Some("completed")
    );

    let output = body.get("output").and_then(|v| v.as_array()).unwrap();
    assert_eq!(output.len(), 1);
    let content = output[0].get("content").and_then(|v| v.as_array()).unwrap();
    assert_eq!(
        content[0].get("type").and_then(|v| v.as_str()),
        Some("output_text")
    );
    assert!(content[0].get("text").and_then(|v| v.as_str()).is_some());

    let usage = body.get("usage").unwrap();
    let input_tokens = usage.get("input_tokens").and_then(|v| v.as_u64()).unwrap();
    let output_tokens = usage.get("output_tokens").and_then(|v| v.as_u64()).unwrap();
    let total_tokens = usage.get("total_tokens").and_then(|v| v.as_u64()).unwrap();
    assert_eq!(total_tokens, input_tokens + output_tokens);
}

#[tokio::test]
async fn echoes_x_request_id_when_provided() {
    let (base, _h) = spawn_app().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{base}/v1/responses"))
        .header("content-type", "application/json")
        .header("x-request-id", "test-rid-123")
        .json(&json!({"model":"openclaw:x","input":"hi"}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.headers()
            .get("x-request-id")
            .and_then(|v| v.to_str().ok()),
        Some("test-rid-123")
    );
}

#[tokio::test]
async fn sync_p95_latency_under_200ms_on_warm_loopback() {
    // SC-006: under representative load, p95 total response time MUST be
    // < 200ms. On warm loopback against the placeholder this is trivially
    // achieved; the test exists to guard against regressions (e.g., a
    // future change that synchronously blocks the runtime).
    let (base, _h) = spawn_app().await;
    let client = reqwest::Client::new();

    // Warm-up.
    for _ in 0..3 {
        let _ = client
            .post(format!("{base}/v1/responses"))
            .header("content-type", "application/json")
            .json(&json!({"model":"openclaw:x","input":"hi"}))
            .send()
            .await;
    }

    let mut samples = Vec::with_capacity(50);
    for _ in 0..50 {
        let start = Instant::now();
        let resp = client
            .post(format!("{base}/v1/responses"))
            .header("content-type", "application/json")
            .json(&json!({"model":"openclaw:x","input":"hi"}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 200);
        let _ = resp.bytes().await.unwrap();
        samples.push(start.elapsed());
    }
    samples.sort();
    let p95 = samples[(samples.len() * 95) / 100];
    assert!(
        p95 < Duration::from_millis(200),
        "sync p95 budget exceeded: {p95:?}"
    );
}
