//! Azure OpenAI provider using the Responses API over raw HTTP.
//!
//! Port of ``nanobot.providers.azure_openai_provider``. Uses ``reqwest``
//! to POST to ``{endpoint}/openai/v1/responses``, mirroring the Python
//! ``AsyncOpenAI(base_url="{endpoint}/openai/v1/").responses.create()``
//! call.

use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use log::warn;
use serde_json::{Map, Value, json};

use crate::base::extract_retry_after_from_text;
use crate::base::{ChatRequest, LLMProvider};
use crate::base::{GenerationSettings, LLMResponse, ToolChoice};
use crate::base::{enforce_role_alternation, sanitize_empty_content};
use crate::responses::{consume_sse, convert_messages, convert_tools, parse_response_output};

const DEFAULT_API_VERSION: &str = "2024-10-21";

#[derive(Clone)]
pub struct AzureOpenAIConfig {
    /// Azure endpoint, e.g. ``https://my-resource.openai.azure.com``.
    pub endpoint: String,
    pub api_key: String,
    /// Deployment name used when the caller doesn't specify ``model``.
    pub default_deployment: String,
    pub api_version: String,
    pub extra_headers: HashMap<String, String>,
    pub timeout: Duration,
}

impl AzureOpenAIConfig {
    pub fn new(
        endpoint: impl Into<String>,
        api_key: impl Into<String>,
        default_deployment: impl Into<String>,
    ) -> Self {
        Self {
            endpoint: endpoint.into(),
            api_key: api_key.into(),
            default_deployment: default_deployment.into(),
            api_version: DEFAULT_API_VERSION.into(),
            extra_headers: HashMap::new(),
            timeout: Duration::from_secs(120),
        }
    }

    pub fn with_api_version(mut self, v: impl Into<String>) -> Self {
        self.api_version = v.into();
        self
    }
    pub fn with_extra_header(mut self, k: impl Into<String>, v: impl Into<String>) -> Self {
        self.extra_headers.insert(k.into(), v.into());
        self
    }
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

pub struct AzureOpenAIProvider {
    cfg: AzureOpenAIConfig,
    client: reqwest::Client,
    generation: GenerationSettings,
}

impl AzureOpenAIProvider {
    pub fn new(cfg: AzureOpenAIConfig) -> Result<Self, &'static str> {
        if cfg.api_key.is_empty() {
            return Err("Azure OpenAI api_key is required");
        }
        if cfg.endpoint.is_empty() {
            return Err("Azure OpenAI api_base is required");
        }
        let client = reqwest::Client::builder()
            .timeout(cfg.timeout)
            .build()
            .map_err(|_| "failed to build reqwest client")?;
        Ok(Self {
            cfg,
            client,
            generation: GenerationSettings::default(),
        })
    }

    pub fn with_generation(mut self, generation: GenerationSettings) -> Self {
        self.generation = generation;
        self
    }

    /// Responses API endpoint: ``{endpoint}/openai/v1/responses``
    fn responses_url(&self) -> String {
        let base = self.cfg.endpoint.trim_end_matches('/');
        format!("{base}/openai/v1/responses")
    }

    fn supports_temperature(model: &str, reasoning_effort: Option<&str>) -> bool {
        if reasoning_effort.is_some() && reasoning_effort.unwrap_or("").to_lowercase() != "none" {
            return false;
        }
        let m = model.to_ascii_lowercase();
        !(m.contains("gpt-5") || m.contains("o1") || m.contains("o3") || m.contains("o4"))
    }

    fn tool_choice_to_value(tc: Option<&ToolChoice>) -> Value {
        match tc {
            Some(ToolChoice::Required) => Value::String("required".into()),
            Some(ToolChoice::None) => Value::String("none".into()),
            Some(ToolChoice::Specific(v)) => v.clone(),
            _ => Value::String("auto".into()),
        }
    }

    fn build_body(&self, req: &ChatRequest, deployment: &str, stream: bool) -> Value {
        let reasoning = req.reasoning_effort.as_deref();
        let sanitized = sanitize_empty_content(&req.messages);
        let messages = enforce_role_alternation(&sanitized);
        let (instructions, input_items) = convert_messages(&messages);

        let mut body = Map::new();
        body.insert("model".into(), Value::String(deployment.to_string()));
        body.insert(
            "instructions".into(),
            if instructions.is_empty() {
                Value::Null
            } else {
                Value::String(instructions)
            },
        );
        body.insert("input".into(), Value::Array(input_items));
        body.insert("max_output_tokens".into(), json!(req.max_tokens.max(1)));
        body.insert("store".into(), Value::Bool(false));
        body.insert("stream".into(), Value::Bool(stream));

        if Self::supports_temperature(deployment, reasoning) {
            body.insert("temperature".into(), json!(req.temperature as f64));
        }

        if let Some(r) = reasoning
            .filter(|reasoning| !reasoning.is_empty() && reasoning.to_lowercase() != "none")
        {
            body.insert("reasoning".into(), json!({"effort": r}));
            body.insert(
                "include".into(),
                Value::Array(vec![Value::String("reasoning.encrypted_content".into())]),
            );
        }

        if let Some(tools) = req.tools.as_ref().filter(|tools| !tools.is_empty()) {
            body.insert("tools".into(), Value::Array(convert_tools(tools)));
            body.insert(
                "tool_choice".into(),
                Self::tool_choice_to_value(req.tool_choice.as_ref()),
            );
        }

        Value::Object(body)
    }

    async fn send(&self, body: &Value, _stream: bool) -> Result<reqwest::Response, reqwest::Error> {
        let mut request = self
            .client
            .post(self.responses_url())
            .header("Content-Type", "application/json")
            .header("api-key", self.cfg.api_key.as_str());
        for (k, v) in &self.cfg.extra_headers {
            request = request.header(k, v);
        }
        request.json(body).send().await
    }

    fn parse_error_response(
        status: reqwest::StatusCode,
        headers: &reqwest::header::HeaderMap,
        payload: &str,
    ) -> LLMResponse {
        let status_i32 = status.as_u16() as i32;
        let msg = if payload.trim().is_empty() {
            format!("Error calling Azure OpenAI: HTTP {status_i32}")
        } else {
            let preview: String = payload.trim().chars().take(500).collect();
            format!("Error: {preview}")
        };
        let retry_after = headers
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<f64>().ok())
            .or_else(|| extract_retry_after_from_text(Some(&msg)));
        LLMResponse {
            content: Some(msg),
            finish_reason: "error".into(),
            retry_after,
            error_status_code: Some(status_i32),
            error_retry_after_s: retry_after,
            ..Default::default()
        }
    }

    fn handle_error(e: &reqwest::Error, body_text: Option<&str>) -> LLMResponse {
        let msg = match body_text {
            Some(b) if !b.trim().is_empty() => {
                format!("Error: {}", b.trim().chars().take(500).collect::<String>())
            }
            _ => format!("Error calling Azure OpenAI: {e}"),
        };
        LLMResponse {
            content: Some(msg),
            finish_reason: "error".into(),
            error_kind: if e.is_timeout() {
                Some("timeout".into())
            } else if e.is_connect() {
                Some("connection".into())
            } else {
                None
            },
            ..Default::default()
        }
    }
}

#[async_trait]
impl LLMProvider for AzureOpenAIProvider {
    fn default_model(&self) -> String {
        self.cfg.default_deployment.clone()
    }

    fn generation(&self) -> GenerationSettings {
        self.generation.clone()
    }

    fn supports_progress_deltas(&self) -> bool {
        true
    }

    async fn chat(&self, req: ChatRequest) -> LLMResponse {
        let deployment = req
            .model
            .as_deref()
            .unwrap_or(&self.cfg.default_deployment)
            .to_string();
        let body = self.build_body(&req, &deployment, false);

        let resp = match self.send(&body, false).await {
            Ok(r) => r,
            Err(e) => return Self::handle_error(&e, None),
        };
        let status = resp.status();
        let headers = resp.headers().clone();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            warn!("Azure OpenAI error {status}: {text}");
            return Self::parse_error_response(status, &headers, &text);
        }
        let text = match resp.text().await {
            Ok(t) => t,
            Err(e) => {
                return LLMResponse::error(format!("Error reading body: {e}"));
            }
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            return LLMResponse::error(
                "Error: malformed JSON response from Azure OpenAI".to_string(),
            );
        };
        parse_response_output(&value)
    }

    async fn chat_stream(
        &self,
        req: ChatRequest,
        on_delta: Option<crate::base::StreamDeltaCallback>,
        on_tool_call_delta: Option<crate::responses::ToolCallDeltaCallback>,
    ) -> LLMResponse {
        let deployment = req
            .model
            .as_deref()
            .unwrap_or(&self.cfg.default_deployment)
            .to_string();
        let body = self.build_body(&req, &deployment, true);

        let resp = match self.send(&body, true).await {
            Ok(r) => r,
            Err(e) => return Self::handle_error(&e, None),
        };
        let status = resp.status();
        let headers = resp.headers().clone();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            warn!("Azure OpenAI stream error {status}: {text}");
            return Self::parse_error_response(status, &headers, &text);
        }
        let body_text = match resp.text().await {
            Ok(t) => t,
            Err(e) => {
                return LLMResponse::error(format!("Error reading stream: {e}"));
            }
        };

        let (content, tool_calls, finish_reason) =
            consume_sse(&body_text, on_delta.clone(), on_tool_call_delta.clone()).await;

        LLMResponse {
            content: (!content.is_empty()).then_some(content),
            tool_calls,
            finish_reason,
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn responses_url_formats_correctly() {
        let cfg = AzureOpenAIConfig::new("https://my-resource.openai.azure.com/", "key", "gpt-4o");
        let p = AzureOpenAIProvider::new(cfg).unwrap();
        let url = p.responses_url();
        assert!(url.contains("/openai/v1/responses"));
    }

    #[test]
    fn build_body_drops_temperature_for_reasoning() {
        let cfg = AzureOpenAIConfig::new("https://x/", "k", "o3");
        let p = AzureOpenAIProvider::new(cfg).unwrap();
        let req = ChatRequest {
            messages: vec![json!({"role":"user","content":"hi"})],
            tools: None,
            model: None,
            max_tokens: 100,
            temperature: 0.5,
            reasoning_effort: Some("medium".into()),
            tool_choice: None,
        };
        let body = p.build_body(&req, "o3", false);
        assert!(body.get("temperature").is_none());
        assert_eq!(body["reasoning"]["effort"], "medium");
    }

    #[test]
    fn build_body_keeps_temperature_for_chat_models() {
        let cfg = AzureOpenAIConfig::new("https://x/", "k", "gpt-4o");
        let p = AzureOpenAIProvider::new(cfg).unwrap();
        let req = ChatRequest {
            messages: vec![json!({"role":"user","content":"hi"})],
            tools: None,
            model: None,
            max_tokens: 100,
            temperature: 0.5,
            reasoning_effort: None,
            tool_choice: None,
        };
        let body = p.build_body(&req, "gpt-4o", false);
        assert_eq!(body["temperature"], 0.5);
    }

    #[test]
    fn missing_key_or_endpoint_is_rejected() {
        assert!(AzureOpenAIProvider::new(AzureOpenAIConfig::new("", "k", "d")).is_err());
        assert!(AzureOpenAIProvider::new(AzureOpenAIConfig::new("e", "", "d")).is_err());
    }

    #[test]
    fn build_body_uses_responses_api_fields() {
        let cfg = AzureOpenAIConfig::new("https://x/", "k", "gpt-4o");
        let p = AzureOpenAIProvider::new(cfg).unwrap();
        let req = ChatRequest {
            messages: vec![
                json!({"role":"system","content":"sys"}),
                json!({"role":"user","content":"hi"}),
            ],
            tools: None,
            model: None,
            max_tokens: 200,
            temperature: 0.7,
            reasoning_effort: None,
            tool_choice: None,
        };
        let body = p.build_body(&req, "gpt-4o", false);
        assert_eq!(body["model"], "gpt-4o");
        assert_eq!(body["instructions"], "sys");
        assert!(body["input"].is_array());
        assert_eq!(body["max_output_tokens"], 200);
        assert_eq!(body["store"], false);
        assert_eq!(body["stream"], false);
    }
}
