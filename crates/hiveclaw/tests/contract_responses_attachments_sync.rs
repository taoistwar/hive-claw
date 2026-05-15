//! Contract test for the content-array form of `POST /v1/responses` in
//! synchronous mode, per `specs/001-hiveclaw-hivegui/contracts/openresponses-v1.md`
//! §`input` content-item array form + §Stub-reply enrichment.

use std::time::{Duration, Instant};

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
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

fn data_uri(mime: &str, bytes: &[u8]) -> String {
    format!("data:{};base64,{}", mime, B64.encode(bytes))
}

#[tokio::test]
async fn sync_with_input_file_acknowledges_attachment_metadata() {
    let base = spawn_app().await;
    let client = reqwest::Client::new();

    let file_bytes = b"SELECT * FROM events;";
    let body = json!({
        "model": "openclaw:hiveclaw-placeholder-v1",
        "input": [{
            "role": "user",
            "content": [
                {"type": "input_text", "text": "解释这段 SQL"},
                {"type": "input_file",
                 "filename": "query.hql",
                 "file_data": data_uri("text/plain", file_bytes)},
            ]
        }]
    });

    let resp = client
        .post(format!("{base}/v1/responses"))
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 200);
    let parsed: serde_json::Value = resp.json().await.unwrap();
    let text = parsed
        .pointer("/output/0/content/0/text")
        .and_then(|v| v.as_str())
        .unwrap();
    assert!(text.contains("附件："), "missing 附件 marker: {text:?}");
    assert!(text.contains("query.hql"), "missing filename: {text:?}");
    assert!(text.contains("text/plain"), "missing mime: {text:?}");
    // The base text MUST still be present so deterministic v1 tests keep working.
    assert!(
        text.contains("HiveClaw 占位回复"),
        "missing base reply: {text:?}"
    );
}

#[tokio::test]
async fn sync_attachment_p95_under_500ms_on_warm_loopback() {
    // SC-007: requests carrying any attachment have p95 < 500ms sync.
    let base = spawn_app().await;
    let client = reqwest::Client::new();
    let payload = vec![b'x'; 256 * 1024]; // 256 KiB
    let body = json!({
        "model": "openclaw:hiveclaw-placeholder-v1",
        "input": [{
            "role": "user",
            "content": [
                {"type": "input_text", "text": "p95 sample"},
                {"type": "input_file",
                 "filename": "blob.bin",
                 "file_data": data_uri("application/octet-stream", &payload)}
            ]
        }]
    });

    // Warm-up.
    for _ in 0..3 {
        let _ = client
            .post(format!("{base}/v1/responses"))
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await;
    }

    let mut samples = Vec::with_capacity(50);
    for _ in 0..50 {
        let start = Instant::now();
        let resp = client
            .post(format!("{base}/v1/responses"))
            .header("content-type", "application/json")
            .json(&body)
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
        p95 < Duration::from_millis(500),
        "SC-007 sync p95 budget exceeded: {p95:?}"
    );
}
