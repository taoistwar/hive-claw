pub mod streaming;
pub mod sync;

use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;
use uuid::Uuid;

/// HiveGUI's mirror of the OpenResponses request payload. Duplicated from
/// HiveClaw rather than depending on the `hiveclaw` crate (constitution
/// Principle V: two binaries, no internal-library coupling).
#[derive(Debug, Serialize)]
pub struct OpenResponsesRequest {
    pub model: String,
    pub input: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    pub stream: bool,
}

#[derive(Debug, Deserialize)]
pub struct OpenResponse {
    pub id: String,
    pub status: String,
    pub output: Vec<OutputItem>,
}

#[derive(Debug, Deserialize)]
pub struct OutputItem {
    pub content: Vec<ContentItem>,
}

#[derive(Debug, Deserialize)]
pub struct ContentItem {
    #[serde(rename = "type")]
    pub kind: String,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct AssistantReply {
    pub response_id: String,
    pub text: String,
}

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("HiveClaw is unreachable: {0}")]
    Unreachable(String),
    #[error("HiveClaw returned HTTP {status}: {body}")]
    HttpStatus { status: u16, body: String },
    #[error("malformed response body: {0}")]
    MalformedBody(String),
    #[error("streaming protocol error: {0}")]
    StreamingProtocol(String),
}

pub fn build_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .user_agent(format!("hivegui/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .expect("reqwest client should build with rustls-tls feature enabled")
}

pub fn endpoint(base: &Url) -> Url {
    base.join("/v1/responses")
        .expect("'/v1/responses' is a valid relative URL")
}

pub fn request_id() -> Uuid {
    Uuid::new_v4()
}
