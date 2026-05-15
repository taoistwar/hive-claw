//! OpenResponses wire types per `contracts/openresponses-v1.md`.
//! The placeholder response generator lives in [`stub`]; per-request
//! limits live in [`limits`].

pub mod limits;
pub mod stub;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct OpenResponsesRequest {
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub input: Option<Input>,
    #[serde(default)]
    pub instructions: Option<String>,
    #[serde(default)]
    pub stream: Option<serde_json::Value>,
    #[serde(default)]
    pub tools: Option<serde_json::Value>,
    #[serde(default)]
    pub tool_choice: Option<serde_json::Value>,
    #[serde(default)]
    pub max_output_tokens: Option<serde_json::Value>,
    #[serde(default)]
    pub max_tool_calls: Option<serde_json::Value>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum Input {
    Text(String),
    Items(Vec<InputItem>),
}

#[derive(Debug, Deserialize)]
pub struct InputItem {
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub content: Option<InputItemContent>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum InputItemContent {
    Text(String),
    Items(Vec<RequestContentItem>),
}

/// Content-item array form (FR-003a). Each variant carries the fields
/// required by the contract; unknown variants are rejected as
/// `unknown content item type 'X'` at validation time.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RequestContentItem {
    InputText {
        #[serde(default)]
        text: Option<String>,
    },
    InputFile {
        #[serde(default)]
        filename: Option<String>,
        #[serde(default)]
        file_data: Option<String>,
        #[serde(default)]
        file_id: Option<String>,
    },
    InputImage {
        #[serde(default)]
        image_url: Option<String>,
    },
    #[serde(other)]
    Unknown,
}

/// Successfully validated request — what the handler consumes.
pub struct ValidatedRequest {
    pub model: String,
    pub input_text: String,
    pub stream: bool,
    pub attachments: Vec<AttachmentMeta>,
}

/// Per-attachment metadata extracted at validation time. The raw decoded
/// bytes are intentionally NOT stored — v1's placeholder never re-reads
/// them. Only filename, MIME, and decoded size flow into the stub reply.
#[derive(Debug, Clone)]
pub struct AttachmentMeta {
    pub filename: String,
    pub mime: String,
    pub size_bytes: usize,
}

#[derive(Debug)]
pub enum ValidationError {
    BadRequest(String),
    PayloadTooLarge(String),
}

impl ValidationError {
    pub fn message(&self) -> &str {
        match self {
            ValidationError::BadRequest(m) => m,
            ValidationError::PayloadTooLarge(m) => m,
        }
    }

    pub fn is_payload_too_large(&self) -> bool {
        matches!(self, ValidationError::PayloadTooLarge(_))
    }
}

#[derive(Debug, Serialize, Clone)]
pub struct OpenResponse {
    pub id: String,
    pub object: &'static str,
    pub created: i64,
    pub model: String,
    pub status: ResponseStatus,
    pub output: Vec<OutputItem>,
    pub usage: Usage,
}

#[derive(Debug, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResponseStatus {
    InProgress,
    Completed,
    Failed,
}

#[derive(Debug, Serialize, Clone)]
pub struct OutputItem {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub role: &'static str,
    pub content: Vec<ContentItem>,
}

#[derive(Debug, Serialize, Clone)]
pub struct ContentItem {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub text: String,
}

#[derive(Debug, Serialize, Clone, Copy)]
pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub total_tokens: u32,
}

#[derive(Debug, Serialize)]
pub struct ErrorEnvelope {
    pub error: ErrorBody,
}

#[derive(Debug, Serialize)]
pub struct ErrorBody {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub message: String,
}

impl ErrorEnvelope {
    pub fn invalid_request(message: impl Into<String>) -> Self {
        ErrorEnvelope {
            error: ErrorBody {
                kind: "invalid_request_error",
                message: message.into(),
            },
        }
    }

    pub fn server_error(message: impl Into<String>) -> Self {
        ErrorEnvelope {
            error: ErrorBody {
                kind: "server_error",
                message: message.into(),
            },
        }
    }
}

#[derive(Debug, Serialize)]
pub struct CreatedPayload<'a> {
    pub id: &'a str,
    pub object: &'static str,
    pub created: i64,
    pub model: &'a str,
    pub status: ResponseStatus,
}

#[derive(Debug, Serialize)]
pub struct DeltaPayload<'a> {
    pub id: &'a str,
    pub delta: &'a str,
}

pub fn validate(req: OpenResponsesRequest) -> Result<ValidatedRequest, ValidationError> {
    use ValidationError::{BadRequest, PayloadTooLarge};

    let model = req.model.unwrap_or_default();
    if model.is_empty() {
        return Err(BadRequest("field 'model' is required".into()));
    }
    if !model.starts_with("openclaw:") {
        return Err(BadRequest(
            "field 'model' must be of the form 'openclaw:<agent-id>'".into(),
        ));
    }

    let mut input_text = String::new();
    let mut attachments: Vec<AttachmentMeta> = Vec::new();
    let mut total_attachment_bytes: usize = 0;

    match req.input {
        Some(Input::Text(s)) if !s.is_empty() => {
            input_text = s;
        }
        Some(Input::Items(items)) if !items.is_empty() => {
            for item in items.into_iter() {
                let role_is_user = item.role.as_deref() == Some("user");
                let content = match item.content {
                    Some(c) => c,
                    None => continue,
                };
                match content {
                    InputItemContent::Text(s) => {
                        if role_is_user {
                            if !input_text.is_empty() {
                                input_text.push('\n');
                            }
                            input_text.push_str(&s);
                        }
                    }
                    InputItemContent::Items(content_items) => {
                        if content_items.is_empty() {
                            return Err(BadRequest(
                                "field 'content' must be non-empty when 'input' uses the array form"
                                    .into(),
                            ));
                        }
                        for ci in content_items.into_iter() {
                            validate_content_item(
                                ci,
                                role_is_user,
                                &mut input_text,
                                &mut attachments,
                                &mut total_attachment_bytes,
                            )?;
                        }
                    }
                }
            }
            if input_text.is_empty() && attachments.is_empty() {
                return Err(BadRequest(
                    "field 'input' is required and must be non-empty".into(),
                ));
            }
        }
        _ => {
            return Err(BadRequest(
                "field 'input' is required and must be non-empty".into(),
            ));
        }
    }

    let stream = match req.stream {
        None | Some(serde_json::Value::Null) => false,
        Some(serde_json::Value::Bool(b)) => b,
        Some(_) => return Err(BadRequest("field 'stream' must be a boolean".into())),
    };

    if let Some(v) = req.max_output_tokens {
        match v {
            serde_json::Value::Null => {}
            serde_json::Value::Number(n) => match n.as_i64() {
                Some(i) if i > 0 => {}
                _ => {
                    return Err(BadRequest(
                        "field 'max_output_tokens' must be a positive integer".into(),
                    ));
                }
            },
            _ => {
                return Err(BadRequest(
                    "field 'max_output_tokens' must be a positive integer".into(),
                ));
            }
        }
    }

    if total_attachment_bytes > limits::TOTAL_ATTACHMENTS_MAX_BYTES {
        return Err(PayloadTooLarge(
            "attachments exceed 4 MiB total limit".into(),
        ));
    }

    Ok(ValidatedRequest {
        model,
        input_text,
        stream,
        attachments,
    })
}

fn validate_content_item(
    item: RequestContentItem,
    role_is_user: bool,
    input_text: &mut String,
    attachments: &mut Vec<AttachmentMeta>,
    total_bytes: &mut usize,
) -> Result<(), ValidationError> {
    use ValidationError::{BadRequest, PayloadTooLarge};

    match item {
        RequestContentItem::InputText { text } => {
            let text = text.unwrap_or_default();
            if role_is_user {
                if !input_text.is_empty() {
                    input_text.push('\n');
                }
                input_text.push_str(&text);
            }
            Ok(())
        }
        RequestContentItem::InputFile {
            filename,
            file_data,
            file_id,
        } => {
            if file_id.is_some() {
                return Err(BadRequest(
                    "field 'file_id' is not supported in v1; use 'file_data'".into(),
                ));
            }
            let filename = filename.unwrap_or_default();
            let file_data = match file_data {
                Some(d) => d,
                None => {
                    return Err(BadRequest(
                        "field 'file_data' must be a data: URI with base64 encoding".into(),
                    ));
                }
            };
            let parsed = parse_data_uri(&file_data).ok_or_else(|| {
                BadRequest("field 'file_data' must be a data: URI with base64 encoding".into())
            })?;
            let decoded = B64
                .decode(parsed.payload)
                .map_err(|_| BadRequest("field 'file_data' payload is not valid base64".into()))?;
            if decoded.len() > limits::PER_FILE_MAX_BYTES {
                return Err(PayloadTooLarge(format!(
                    "file '{}' exceeds 1 MiB per-file limit",
                    filename
                )));
            }
            *total_bytes += decoded.len();
            if *total_bytes > limits::TOTAL_ATTACHMENTS_MAX_BYTES {
                return Err(PayloadTooLarge(
                    "attachments exceed 4 MiB total limit".into(),
                ));
            }
            attachments.push(AttachmentMeta {
                filename,
                mime: parsed.mime.to_string(),
                size_bytes: decoded.len(),
            });
            Ok(())
        }
        RequestContentItem::InputImage { image_url } => {
            let image_url = image_url.unwrap_or_default();
            let parsed = parse_data_uri(&image_url).ok_or_else(|| {
                BadRequest("field 'image_url' must be a data:image/*;base64 URI".into())
            })?;
            if !parsed.mime.starts_with("image/") {
                return Err(BadRequest(
                    "field 'image_url' must be a data:image/*;base64 URI".into(),
                ));
            }
            let decoded = B64
                .decode(parsed.payload)
                .map_err(|_| BadRequest("field 'image_url' payload is not valid base64".into()))?;
            if decoded.len() > limits::PER_FILE_MAX_BYTES {
                return Err(PayloadTooLarge("image exceeds 1 MiB per-file limit".into()));
            }
            *total_bytes += decoded.len();
            if *total_bytes > limits::TOTAL_ATTACHMENTS_MAX_BYTES {
                return Err(PayloadTooLarge(
                    "attachments exceed 4 MiB total limit".into(),
                ));
            }
            attachments.push(AttachmentMeta {
                filename: "image".to_string(),
                mime: parsed.mime.to_string(),
                size_bytes: decoded.len(),
            });
            Ok(())
        }
        RequestContentItem::Unknown => Err(BadRequest("unknown content item type 'X'".replace(
            "X",
            // We can't recover the original tag because serde already consumed it
            // and routed to `Unknown`; the contract just requires the message
            // contain "unknown content item type".
            "<unrecognised>",
        ))),
    }
}

struct ParsedDataUri<'a> {
    mime: &'a str,
    payload: &'a str,
}

fn parse_data_uri(s: &str) -> Option<ParsedDataUri<'_>> {
    // Format: data:<mime>;base64,<payload>
    let rest = s.strip_prefix("data:")?;
    let (head, payload) = rest.split_once(',')?;
    let (mime, encoding) = head.split_once(';')?;
    if encoding != "base64" {
        return None;
    }
    if mime.is_empty() {
        return None;
    }
    Some(ParsedDataUri { mime, payload })
}
