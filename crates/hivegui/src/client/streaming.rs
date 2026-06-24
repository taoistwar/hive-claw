use eventsource_stream::Eventsource;
use futures::{Stream, StreamExt};
use serde::Deserialize;
use uuid::Uuid;

use super::{ClientError, OpenResponsesRequest};

#[derive(Debug, Clone)]
pub enum StreamingEvent {
    Created {
        response_id: String,
    },
    Delta {
        response_id: String,
        delta: String,
    },
    Completed {
        response_id: String,
        full_text: String,
    },
}

#[derive(Debug, Deserialize)]
struct CreatedFrame {
    id: String,
}

#[derive(Debug, Deserialize)]
struct DeltaFrame {
    id: String,
    delta: String,
}

#[derive(Debug, Deserialize)]
struct CompletedFrame {
    id: String,
    output: Vec<OutputItem>,
}

#[derive(Debug, Deserialize)]
struct OutputItem {
    content: Vec<ContentItem>,
}

#[derive(Debug, Deserialize)]
struct ContentItem {
    #[serde(rename = "type")]
    kind: String,
    text: String,
}

pub async fn send(
    client: &reqwest::Client,
    url: &url::Url,
    mut req: OpenResponsesRequest,
    request_id: Uuid,
) -> Result<impl Stream<Item = Result<StreamingEvent, ClientError>> + Unpin, ClientError> {
    req.stream = true;
    let resp = client
        .post(super::endpoint(url))
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .header("x-request-id", request_id.to_string())
        .json(&req)
        .send()
        .await
        .map_err(|e| ClientError::Unreachable(e.to_string()))?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(ClientError::HttpStatus {
            status: status.as_u16(),
            body,
        });
    }

    let events = resp.bytes_stream().eventsource();

    let stream = events.filter_map(|event| async move {
        let ev = match event {
            Ok(ev) => ev,
            Err(e) => return Some(Err(ClientError::StreamingProtocol(e.to_string()))),
        };
        // The terminator frame: `data: [DONE]` with no event name.
        if ev.event.is_empty() && ev.data.trim() == "[DONE]" {
            return None;
        }
        match ev.event.as_str() {
            "response.created" => match serde_json::from_str::<CreatedFrame>(&ev.data) {
                Ok(c) => Some(Ok(StreamingEvent::Created { response_id: c.id })),
                Err(e) => Some(Err(ClientError::StreamingProtocol(format!(
                    "bad created frame: {e}"
                )))),
            },
            "response.output_text.delta" => match serde_json::from_str::<DeltaFrame>(&ev.data) {
                Ok(d) => Some(Ok(StreamingEvent::Delta {
                    response_id: d.id,
                    delta: d.delta,
                })),
                Err(e) => Some(Err(ClientError::StreamingProtocol(format!(
                    "bad delta frame: {e}"
                )))),
            },
            "response.completed" => match serde_json::from_str::<CompletedFrame>(&ev.data) {
                Ok(c) => {
                    let text = c
                        .output
                        .into_iter()
                        .next()
                        .and_then(|o| o.content.into_iter().find(|x| x.kind == "output_text"))
                        .map(|x| x.text)
                        .unwrap_or_default();
                    Some(Ok(StreamingEvent::Completed {
                        response_id: c.id,
                        full_text: text,
                    }))
                }
                Err(e) => Some(Err(ClientError::StreamingProtocol(format!(
                    "bad completed frame: {e}"
                )))),
            },
            "response.failed" => Some(Err(ClientError::StreamingProtocol(
                "server emitted response.failed".to_string(),
            ))),
            // Forward-compat: unknown event types are ignored.
            _ => None,
        }
    });

    Ok(Box::pin(stream))
}
