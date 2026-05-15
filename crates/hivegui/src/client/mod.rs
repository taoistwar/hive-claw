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
    pub input: InputForm,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    pub stream: bool,
}

/// `input` carries either a plain string (text-only path) or the
/// content-item array form (FR-003a). HiveGUI picks the fast path when
/// no attachments are present so the wire stays trivially compatible
/// with the v1 baseline tests.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum InputForm {
    Text(String),
    Items(Vec<InputItem>),
}

#[derive(Debug, Serialize)]
pub struct InputItem {
    pub role: String,
    pub content: Vec<RequestContentItem>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RequestContentItem {
    InputText { text: String },
    InputFile { filename: String, file_data: String },
    InputImage { image_url: String },
}

impl OpenResponsesRequest {
    /// Build the wire request from a user turn. When no attachments are
    /// present we use the legacy string form — this keeps the existing
    /// v1 mock tests (T029 / T030) intact.
    pub fn from_user_turn(
        model: impl Into<String>,
        text: &str,
        attachments: &[crate::model::conversation::Attachment],
        stream: bool,
    ) -> Self {
        let model = model.into();
        let input = if attachments.is_empty() {
            InputForm::Text(text.to_string())
        } else {
            let mut content: Vec<RequestContentItem> = Vec::with_capacity(attachments.len() + 1);
            if !text.is_empty() {
                content.push(RequestContentItem::InputText {
                    text: text.to_string(),
                });
            }
            for a in attachments {
                let file_data = match &a.payload {
                    crate::model::conversation::AttachmentPayload::Inline { base64_data_uri } => {
                        base64_data_uri.clone()
                    }
                };
                content.push(RequestContentItem::InputFile {
                    filename: a.filename.clone(),
                    file_data,
                });
            }
            InputForm::Items(vec![InputItem {
                role: "user".to_string(),
                content,
            }])
        };

        OpenResponsesRequest {
            model,
            input,
            instructions: None,
            stream,
        }
    }
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
        .no_proxy()
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
