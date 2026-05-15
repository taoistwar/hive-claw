//! Contract test for the content-array form of `POST /v1/responses` in
//! streaming mode, per `specs/001-hiveclaw-hivegui/contracts/openresponses-v1.md`
//! §Stub-reply enrichment.

use std::time::{Duration, Instant};

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use eventsource_stream::Eventsource;
use futures::StreamExt;
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
async fn streaming_with_input_file_emits_enriched_completed_text() {
    let base = spawn_app().await;
    let client = reqwest::Client::new();

    let body = json!({
        "model": "openclaw:hiveclaw-placeholder-v1",
        "input": [{
            "role": "user",
            "content": [
                {"type": "input_text", "text": "请分析"},
                {"type": "input_file",
                 "filename": "schema.json",
                 "file_data": data_uri("application/json", b"{\"id\": 1}")}
            ]
        }],
        "stream": true
    });

    let resp = client
        .post(format!("{base}/v1/responses"))
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);

    let mut events = resp.bytes_stream().eventsource();
    let mut deltas: Vec<String> = Vec::new();
    let mut completed_text: Option<String> = None;
    let mut done = false;

    while let Some(ev) = events.next().await {
        let ev = ev.unwrap();
        if ev.data.trim() == "[DONE]" {
            done = true;
            break;
        }
        match ev.event.as_str() {
            "response.created" => {}
            "response.output_text.delta" => {
                let v: serde_json::Value = serde_json::from_str(&ev.data).unwrap();
                deltas.push(v.get("delta").and_then(|x| x.as_str()).unwrap().to_string());
            }
            "response.completed" => {
                let v: serde_json::Value = serde_json::from_str(&ev.data).unwrap();
                completed_text = v
                    .pointer("/output/0/content/0/text")
                    .and_then(|s| s.as_str())
                    .map(str::to_string);
            }
            other => panic!("unexpected event type: {other}"),
        }
    }

    assert!(done, "stream must terminate with [DONE]");
    let final_text = completed_text.expect("response.completed must arrive");
    assert!(
        final_text.contains("附件："),
        "missing 附件 marker: {final_text:?}"
    );
    assert!(
        final_text.contains("schema.json"),
        "missing filename: {final_text:?}"
    );
    assert!(
        final_text.contains("application/json"),
        "missing mime: {final_text:?}"
    );

    let joined: String = deltas.join("");
    assert_eq!(
        joined, final_text,
        "concatenated deltas must equal completed text"
    );
}

#[tokio::test]
async fn streaming_attachment_ttfb_under_500ms() {
    // SC-007: TTFB p95 < 500ms with 256 KiB attachment.
    let base = spawn_app().await;
    let client = reqwest::Client::new();
    let payload = vec![b'x'; 256 * 1024];
    let body = json!({
        "model": "openclaw:hiveclaw-placeholder-v1",
        "input": [{
            "role": "user",
            "content": [
                {"type": "input_text", "text": "TTFB"},
                {"type": "input_file",
                 "filename": "blob.bin",
                 "file_data": data_uri("application/octet-stream", &payload)}
            ]
        }],
        "stream": true
    });

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
        let mut events = resp.bytes_stream().eventsource();
        let _first = events.next().await.unwrap().unwrap();
        samples.push(start.elapsed());
    }
    samples.sort();
    let p95 = samples[(samples.len() * 95) / 100];
    assert!(
        p95 < Duration::from_millis(500),
        "SC-007 streaming TTFB p95 budget exceeded: {p95:?}"
    );
}
