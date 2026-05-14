//! Exercise the HiveGUI streaming client against an in-process axum mock
//! that emits the canonical four SSE events.

use axum::{
    response::sse::{Event, KeepAlive, Sse},
    routing::post,
    Router,
};
use futures::stream;
use futures::StreamExt;
use hivegui::client::{self, streaming, OpenResponsesRequest};
use std::convert::Infallible;
use tokio::net::TcpListener;
use url::Url;
use uuid::Uuid;

async fn handler() -> Sse<impl futures::Stream<Item = Result<Event, Infallible>>> {
    let frames: Vec<Result<Event, Infallible>> = vec![
        Ok(Event::default()
            .event("response.created")
            .data(r#"{"id":"resp_mock","object":"response","created":1,"model":"openclaw:test","status":"in_progress"}"#)),
        Ok(Event::default()
            .event("response.output_text.delta")
            .data(r#"{"id":"resp_mock","delta":"hello "}"#)),
        Ok(Event::default()
            .event("response.output_text.delta")
            .data(r#"{"id":"resp_mock","delta":"world"}"#)),
        // Forward-compat: an unknown event type the client must ignore.
        Ok(Event::default().event("response.unknown_event").data("{}")),
        Ok(Event::default()
            .event("response.completed")
            .data(r#"{"id":"resp_mock","object":"response","created":1,"model":"openclaw:test","status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"hello world"}]}],"usage":{"input_tokens":1,"output_tokens":2,"total_tokens":3}}"#)),
        Ok(Event::default().data("[DONE]")),
    ];
    Sse::new(stream::iter(frames)).keep_alive(KeepAlive::default())
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
async fn streaming_client_yields_deltas_then_completed_then_terminates() {
    let url = spawn_mock().await;
    let http = client::build_client();
    let mut stream = streaming::send(
        &http,
        &url,
        OpenResponsesRequest {
            model: "openclaw:test".to_string(),
            input: "hi".to_string(),
            instructions: None,
            stream: true,
        },
        Uuid::new_v4(),
    )
    .await
    .unwrap();

    let mut deltas: Vec<String> = Vec::new();
    let mut completed_text: Option<String> = None;

    while let Some(ev) = stream.next().await {
        match ev.unwrap() {
            streaming::StreamingEvent::Created { .. } => {}
            streaming::StreamingEvent::Delta { delta, .. } => deltas.push(delta),
            streaming::StreamingEvent::Completed { full_text, .. } => {
                completed_text = Some(full_text);
            }
        }
    }
    assert_eq!(deltas, vec!["hello ", "world"]);
    assert_eq!(completed_text.as_deref(), Some("hello world"));
}
