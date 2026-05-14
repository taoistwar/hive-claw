use uuid::Uuid;

use super::{AssistantReply, ClientError, OpenResponse, OpenResponsesRequest};

pub async fn send(
    client: &reqwest::Client,
    url: &url::Url,
    req: OpenResponsesRequest,
    request_id: Uuid,
) -> Result<AssistantReply, ClientError> {
    let resp = client
        .post(super::endpoint(url))
        .header("content-type", "application/json")
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

    let parsed: OpenResponse = resp
        .json()
        .await
        .map_err(|e| ClientError::MalformedBody(e.to_string()))?;

    let text = parsed
        .output
        .into_iter()
        .next()
        .and_then(|o| o.content.into_iter().find(|c| c.kind == "output_text"))
        .map(|c| c.text)
        .ok_or_else(|| {
            ClientError::MalformedBody("response has no output_text content".to_string())
        })?;

    Ok(AssistantReply {
        response_id: parsed.id,
        text,
    })
}
