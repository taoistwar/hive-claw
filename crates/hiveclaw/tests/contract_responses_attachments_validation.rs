//! Validation contract for the content-array form, one test per row in
//! `contracts/openresponses-v1.md` §Validation rules (the rows added by
//! T059 for FR-003a).

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

async fn post(body: serde_json::Value) -> reqwest::Response {
    let base = spawn_app().await;
    reqwest::Client::new()
        .post(format!("{base}/v1/responses"))
        .header("content-type", "application/json")
        .body(body.to_string())
        .send()
        .await
        .unwrap()
}

async fn assert_error(body: serde_json::Value, status: u16, expect_msg: &str) {
    let resp = post(body).await;
    assert_eq!(resp.status().as_u16(), status, "expected {status}");
    let parsed: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        parsed.pointer("/error/type").and_then(|v| v.as_str()),
        Some("invalid_request_error")
    );
    let msg = parsed
        .pointer("/error/message")
        .and_then(|v| v.as_str())
        .unwrap();
    assert!(
        msg.contains(expect_msg),
        "message {msg:?} does not contain {expect_msg:?}"
    );
}

#[tokio::test]
async fn rejects_empty_content_array() {
    assert_error(
        json!({"model":"openclaw:x","input":[{"role":"user","content":[]}]}),
        400,
        "'content' must be non-empty",
    )
    .await;
}

#[tokio::test]
async fn rejects_unknown_content_item_type() {
    assert_error(
        json!({"model":"openclaw:x","input":[{
            "role":"user",
            "content":[{"type":"input_audio","audio_url":"x"}]
        }]}),
        400,
        "unknown content item type",
    )
    .await;
}

#[tokio::test]
async fn rejects_file_id_in_v1() {
    assert_error(
        json!({"model":"openclaw:x","input":[{
            "role":"user",
            "content":[{"type":"input_file","filename":"x","file_id":"file_abc"}]
        }]}),
        400,
        "'file_id' is not supported in v1",
    )
    .await;
}

#[tokio::test]
async fn rejects_non_data_uri_file_data() {
    assert_error(
        json!({"model":"openclaw:x","input":[{
            "role":"user",
            "content":[{"type":"input_file","filename":"x","file_data":"not a data uri"}]
        }]}),
        400,
        "must be a data: URI",
    )
    .await;
}

#[tokio::test]
async fn rejects_bad_base64_payload() {
    assert_error(
        json!({"model":"openclaw:x","input":[{
            "role":"user",
            "content":[{"type":"input_file","filename":"x","file_data":"data:text/plain;base64,@@@"}]
        }]}),
        400,
        "not valid base64",
    )
    .await;
}

#[tokio::test]
async fn rejects_per_file_oversize() {
    // 1 MiB + 1 byte after decode.
    let payload = vec![b'a'; 1024 * 1024 + 1];
    let resp = post(json!({
        "model":"openclaw:x",
        "input":[{
            "role":"user",
            "content":[{
                "type":"input_file",
                "filename":"big.txt",
                "file_data": data_uri("text/plain", &payload)
            }]
        }]
    }))
    .await;
    assert_eq!(resp.status().as_u16(), 413);
    let parsed: serde_json::Value = resp.json().await.unwrap();
    let msg = parsed
        .pointer("/error/message")
        .and_then(|v| v.as_str())
        .unwrap();
    assert!(
        msg.contains("big.txt"),
        "missing filename in error: {msg:?}"
    );
    assert!(msg.contains("1 MiB"), "missing limit in error: {msg:?}");
}

#[tokio::test]
async fn rejects_total_oversize() {
    // Five 900 KiB files = 4.4 MiB > 4 MiB cap.
    let chunk = vec![b'b'; 900 * 1024];
    let items: Vec<serde_json::Value> = (0..5)
        .map(|i| {
            json!({
                "type": "input_file",
                "filename": format!("f{i}.bin"),
                "file_data": data_uri("application/octet-stream", &chunk)
            })
        })
        .collect();
    let resp = post(json!({
        "model":"openclaw:x",
        "input":[{"role":"user","content": items}]
    }))
    .await;
    assert_eq!(resp.status().as_u16(), 413);
    let parsed: serde_json::Value = resp.json().await.unwrap();
    let msg = parsed
        .pointer("/error/message")
        .and_then(|v| v.as_str())
        .unwrap();
    assert!(
        msg.contains("4 MiB"),
        "missing total limit in error: {msg:?}"
    );
}

#[tokio::test]
async fn rejects_bad_image_url_mime() {
    assert_error(
        json!({"model":"openclaw:x","input":[{
            "role":"user",
            "content":[{
                "type":"input_image",
                "image_url":"data:text/plain;base64,aGVsbG8="
            }]
        }]}),
        400,
        "data:image/*",
    )
    .await;
}
