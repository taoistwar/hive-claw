//! Exercise the HiveGUI sync client against an in-process axum mock that
//! returns canned synchronous OpenResponses JSON.

use axum::{routing::post, Json, Router};
use hivegui::client::{self, sync, OpenResponsesRequest};
use serde_json::json;
use tokio::net::TcpListener;
use url::Url;
use uuid::Uuid;

async fn handler() -> Json<serde_json::Value> {
    Json(json!({
        "id": "resp_abc123",
        "object": "response",
        "created": 1715000000,
        "model": "openclaw:test",
        "status": "completed",
        "output": [{
            "type": "message",
            "role": "assistant",
            "content": [{ "type": "output_text", "text": "mock reply" }]
        }],
        "usage": { "input_tokens": 1, "output_tokens": 2, "total_tokens": 3 }
    }))
}

async fn spawn_mock() -> Url {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = Router::new().route("/v1/responses", post(handler));
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Url::parse(&format!("http://{addr}")).unwrap()
}

#[tokio::test]
async fn sync_client_parses_reply() {
    let url = spawn_mock().await;
    let http = client::build_client();
    let reply = sync::send(
        &http,
        &url,
        OpenResponsesRequest {
            model: "openclaw:test".to_string(),
            input: "hi".to_string(),
            instructions: None,
            stream: false,
        },
        Uuid::new_v4(),
    )
    .await
    .unwrap();
    assert_eq!(reply.text, "mock reply");
    assert_eq!(reply.response_id, "resp_abc123");
}
