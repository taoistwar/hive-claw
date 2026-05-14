//! OpenResponses wire types per `contracts/openresponses-v1.md`.
//! These structs are the public contract surface; the placeholder
//! response generator lives in [`stub`].

pub mod stub;

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
    pub content: Option<String>,
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
    pub kind: &'static str, // "message"
    pub role: &'static str, // "assistant"
    pub content: Vec<ContentItem>,
}

#[derive(Debug, Serialize, Clone)]
pub struct ContentItem {
    #[serde(rename = "type")]
    pub kind: &'static str, // "output_text"
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
    pub kind: &'static str, // "invalid_request_error" | "server_error"
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

/// SSE event payloads emitted in streaming mode. The wire shape mirrors
/// `contracts/openresponses-v1.md` §Response — streaming.
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

/// Extracted, validated request after `serde` parsing succeeded.
pub struct ValidatedRequest {
    pub model: String,
    pub input_text: String,
    pub stream: bool,
}

#[derive(Debug)]
pub struct ValidationError {
    pub message: &'static str,
}

pub fn validate(req: OpenResponsesRequest) -> Result<ValidatedRequest, ValidationError> {
    let model = req.model.unwrap_or_default();
    if model.is_empty() {
        return Err(ValidationError {
            message: "field 'model' is required",
        });
    }
    if !model.starts_with("openclaw:") {
        return Err(ValidationError {
            message: "field 'model' must be of the form 'openclaw:<agent-id>'",
        });
    }

    let input_text = match req.input {
        Some(Input::Text(s)) if !s.is_empty() => s,
        Some(Input::Items(items)) if !items.is_empty() => {
            let mut acc = String::new();
            for it in items.iter() {
                if it.role.as_deref() == Some("user") {
                    if let Some(c) = &it.content {
                        if !acc.is_empty() {
                            acc.push('\n');
                        }
                        acc.push_str(c);
                    }
                }
            }
            if acc.is_empty() {
                return Err(ValidationError {
                    message: "field 'input' is required and must be non-empty",
                });
            }
            acc
        }
        _ => {
            return Err(ValidationError {
                message: "field 'input' is required and must be non-empty",
            });
        }
    };

    let stream = match req.stream {
        None | Some(serde_json::Value::Null) => false,
        Some(serde_json::Value::Bool(b)) => b,
        Some(_) => {
            return Err(ValidationError {
                message: "field 'stream' must be a boolean",
            });
        }
    };

    if let Some(v) = req.max_output_tokens {
        match v {
            serde_json::Value::Null => {}
            serde_json::Value::Number(n) => match n.as_i64() {
                Some(i) if i > 0 => {}
                _ => {
                    return Err(ValidationError {
                        message: "field 'max_output_tokens' must be a positive integer",
                    });
                }
            },
            _ => {
                return Err(ValidationError {
                    message: "field 'max_output_tokens' must be a positive integer",
                });
            }
        }
    }

    Ok(ValidatedRequest {
        model,
        input_text,
        stream,
    })
}
