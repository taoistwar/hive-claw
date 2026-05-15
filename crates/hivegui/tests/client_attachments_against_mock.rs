//! HiveGUI client → mock-HiveClaw with the content-array form. The mock
//! asserts the outbound JSON has the FR-003a shape and that the file
//! bytes round-trip through base64.

use axum::{extract::Request, http::StatusCode, response::IntoResponse, routing::post, Router};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use hivegui::client::{self, sync, OpenResponsesRequest};
use hivegui::model::conversation::{Attachment, AttachmentId, AttachmentPayload};
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;
use url::Url;
use uuid::Uuid;

struct CapturedRequest {
    body: serde_json::Value,
}

async fn make_mock(captured: Arc<Mutex<Option<CapturedRequest>>>) -> Url {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let captured_for_handler = captured.clone();
    let app = Router::new().route(
        "/v1/responses",
        post(move |req: Request| {
            let captured = captured_for_handler.clone();
            async move {
                let bytes = axum::body::to_bytes(req.into_body(), 8 * 1024 * 1024)
                    .await
                    .unwrap();
                let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                *captured.lock().unwrap() = Some(CapturedRequest { body: parsed });
                (
                    StatusCode::OK,
                    axum::Json(serde_json::json!({
                        "id": "resp_mock",
                        "object": "response",
                        "created": 1,
                        "model": "openclaw:test",
                        "status": "completed",
                        "output": [{
                            "type": "message",
                            "role": "assistant",
                            "content": [{
                                "type": "output_text",
                                "text": "HiveClaw 占位回复：已收到你的请求。附件：query.hql (5 B, text/plain)"
                            }]
                        }],
                        "usage": {"input_tokens": 1, "output_tokens": 2, "total_tokens": 3}
                    })),
                )
                    .into_response()
            }
        }),
    );
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Url::parse(&format!("http://{addr}")).unwrap()
}

#[tokio::test]
async fn sync_client_serialises_content_array_with_attachment() {
    let captured: Arc<Mutex<Option<CapturedRequest>>> = Arc::new(Mutex::new(None));
    let url = make_mock(captured.clone()).await;
    let http = client::build_client();

    let original_bytes = b"HELLO";
    let base64_data = B64.encode(original_bytes);
    let attachment = Attachment {
        id: AttachmentId(Uuid::new_v4()),
        filename: "query.hql".into(),
        mime: "text/plain".into(),
        size_bytes: original_bytes.len() as u64,
        payload: AttachmentPayload::Inline {
            base64_data_uri: format!("data:text/plain;base64,{}", base64_data),
        },
    };

    let req = OpenResponsesRequest::from_user_turn(
        "openclaw:test",
        "解释这段 SQL",
        std::slice::from_ref(&attachment),
        false,
    );

    let reply = sync::send(&http, &url, req, Uuid::new_v4()).await.unwrap();
    assert!(reply.text.contains("附件："));

    // Mock-side assertions on the outbound wire JSON.
    let captured = captured.lock().unwrap();
    let body = &captured.as_ref().unwrap().body;
    let input = body.get("input").and_then(|v| v.as_array()).unwrap();
    let content = input[0].get("content").and_then(|v| v.as_array()).unwrap();
    assert_eq!(content.len(), 2);
    assert_eq!(
        content[0].get("type").and_then(|v| v.as_str()),
        Some("input_text")
    );
    assert_eq!(
        content[0].get("text").and_then(|v| v.as_str()),
        Some("解释这段 SQL")
    );
    assert_eq!(
        content[1].get("type").and_then(|v| v.as_str()),
        Some("input_file")
    );
    assert_eq!(
        content[1].get("filename").and_then(|v| v.as_str()),
        Some("query.hql")
    );
    let file_data = content[1]
        .get("file_data")
        .and_then(|v| v.as_str())
        .unwrap();
    let stripped = file_data
        .strip_prefix("data:text/plain;base64,")
        .expect("data: URI prefix");
    let decoded = B64.decode(stripped).expect("valid base64 payload");
    assert_eq!(decoded, original_bytes, "file bytes must round-trip");
}
