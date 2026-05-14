//! Contract test for `POST /v1/responses` in streaming (SSE) mode.

use std::time::{Duration, Instant};

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

#[tokio::test]
async fn streaming_response_matches_contract() {
    let base = spawn_app().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{base}/v1/responses"))
        .header("content-type", "application/json")
        .json(&json!({
            "model": "openclaw:hiveclaw-placeholder-v1",
            "input": "hello",
            "stream": true,
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 200);
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    assert!(
        ct.starts_with("text/event-stream"),
        "got content-type: {ct}"
    );
    let cache_control = resp
        .headers()
        .get("cache-control")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert_eq!(cache_control, "no-store");

    let mut events = resp.bytes_stream().eventsource();

    // Expect: response.created → 1..N response.output_text.delta → response.completed → [DONE]
    let first = events.next().await.unwrap().unwrap();
    assert_eq!(first.event, "response.created");
    let parsed: serde_json::Value = serde_json::from_str(&first.data).unwrap();
    assert_eq!(
        parsed.get("status").and_then(|v| v.as_str()),
        Some("in_progress")
    );

    let mut deltas: Vec<String> = Vec::new();
    let mut completed_payload: Option<serde_json::Value> = None;
    let mut done = false;

    while let Some(ev) = events.next().await {
        let ev = ev.unwrap();
        // The terminator frame has no `event:` field; eventsource-stream
        // defaults that to "message" per the SSE spec. Match on data first.
        if ev.data.trim() == "[DONE]" {
            done = true;
            break;
        }
        match ev.event.as_str() {
            "response.output_text.delta" => {
                let v: serde_json::Value = serde_json::from_str(&ev.data).unwrap();
                deltas.push(v.get("delta").and_then(|x| x.as_str()).unwrap().to_string());
            }
            "response.completed" => {
                completed_payload = Some(serde_json::from_str(&ev.data).unwrap());
            }
            other => panic!("unexpected event type: {other}"),
        }
    }

    assert!(!deltas.is_empty(), "must emit at least one delta");
    assert!(done, "stream must terminate with [DONE]");
    let completed = completed_payload.expect("response.completed must be emitted");
    let final_text = completed
        .pointer("/output/0/content/0/text")
        .and_then(|v| v.as_str())
        .unwrap();
    let joined: String = deltas.join("");
    assert_eq!(joined, final_text);
}

#[tokio::test]
async fn streaming_ttfb_under_200ms() {
    // SC-006: time-to-first-event p95 < 200ms on warm loopback.
    let base = spawn_app().await;
    let client = reqwest::Client::new();

    // Warm-up.
    for _ in 0..3 {
        let _ = client
            .post(format!("{base}/v1/responses"))
            .header("content-type", "application/json")
            .json(&json!({"model":"openclaw:x","input":"hi","stream":true}))
            .send()
            .await;
    }

    let mut samples = Vec::with_capacity(50);
    for _ in 0..50 {
        let start = Instant::now();
        let resp = client
            .post(format!("{base}/v1/responses"))
            .header("content-type", "application/json")
            .json(&json!({"model":"openclaw:x","input":"hi","stream":true}))
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
        p95 < Duration::from_millis(200),
        "streaming TTFB p95 budget exceeded: {p95:?}"
    );
}
