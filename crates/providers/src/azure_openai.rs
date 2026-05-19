//! HTTP-based Azure OpenAI provider.
//!
//! Port of `nanobot.providers.azure_openai_provider`, but using Azure's
//! stable Chat Completions deployment endpoint instead of the Responses
//! API (which is still GA-limited and harder to support without the
//! Python SDK).  Endpoint shape:
//!
//! ```text
//! {endpoint}/openai/deployments/{deployment}/chat/completions?api-version={ver}
//! ```
//!
//! Authentication uses Azure's `api-key` header (not `Authorization:
//! Bearer`) so the shared [`OpenAICompatProvider`] isn't a drop-in.

use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use log::warn;
use serde_json::{json, Map, Value};

use crate::provider::{ChatRequest, LLMProvider};
use crate::retry::extract_retry_after_from_text;
use crate::sanitize::{enforce_role_alternation, sanitize_empty_content};
use crate::types::{GenerationSettings, LLMResponse, ToolChoice};

const DEFAULT_API_VERSION: &str = "2024-10-21";

#[derive(Clone)]
pub struct AzureOpenAIConfig {
    /// Azure endpoint, e.g. `https://my-resource.openai.azure.com`.
    pub endpoint: String,
    pub api_key: String,
    /// Deployment name used when the caller doesn't specify `model`.
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
            return Err("Azure OpenAI endpoint is required");
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

    fn endpoint_url(&self, deployment: &str) -> String {
        let base = self.cfg.endpoint.trim_end_matches('/');
        format!(
            "{base}/openai/deployments/{deployment}/chat/completions?api-version={ver}",
            ver = self.cfg.api_version,
        )
    }

    fn supports_temperature(model: &str, reasoning_effort: Option<&str>) -> bool {
        if reasoning_effort.is_some() {
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

    fn build_body(&self, req: &ChatRequest, deployment: &str) -> Value {
        let reasoning = req.reasoning_effort.as_deref();
        let sanitized = sanitize_empty_content(&req.messages);
        // Azure rejects prefill like the generic OpenAI-compat path.
        let messages = enforce_role_alternation(&sanitized);

        let mut body = Map::new();
        // Azure's deployment-based URL already selects the model, but
        // OpenAI's SDK still echoes the field — and some users rely on it
        // to route through a proxy. Keep parity.
        body.insert("model".into(), Value::String(deployment.to_string()));
        body.insert("messages".into(), Value::Array(messages));

        if Self::supports_temperature(deployment, reasoning) {
            body.insert("temperature".into(), json!(req.temperature as f64));
        }
        body.insert("max_tokens".into(), json!(req.max_tokens.max(1)));

        if let Some(r) = reasoning {
            body.insert("reasoning_effort".into(), Value::String(r.to_string()));
        }

        if let Some(tools) = &req.tools {
            if !tools.is_empty() {
                body.insert("tools".into(), Value::Array(tools.clone()));
                body.insert(
                    "tool_choice".into(),
                    Self::tool_choice_to_value(req.tool_choice.as_ref()),
                );
            }
        }

        Value::Object(body)
    }

    async fn send(
        &self,
        deployment: &str,
        body: &Value,
    ) -> Result<reqwest::Response, reqwest::Error> {
        let mut request = self
            .client
            .post(self.endpoint_url(deployment))
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
}

#[async_trait]
impl LLMProvider for AzureOpenAIProvider {
    fn default_model(&self) -> String {
        self.cfg.default_deployment.clone()
    }

    fn generation(&self) -> GenerationSettings {
        self.generation.clone()
    }

    async fn chat(&self, req: ChatRequest) -> LLMResponse {
        let deployment = req
            .model
            .as_deref()
            .unwrap_or(&self.cfg.default_deployment)
            .to_string();
        let body = self.build_body(&req, &deployment);

        let resp = match self.send(&deployment, &body).await {
            Ok(r) => r,
            Err(e) => {
                let msg = format!("Error calling Azure OpenAI: {e}");
                return LLMResponse {
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
                };
            }
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
            Err(e) => return LLMResponse::error(format!("Error reading body: {e}")),
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            return LLMResponse::error(format!("Error: malformed JSON: {text}"));
        };
        // Reuse the OpenAI-compat parser — Azure's /chat/completions
        // returns the exact same shape.
        crate::openai_compat::parse_response(&value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_url_formats_correctly() {
        let cfg = AzureOpenAIConfig::new(
            "https://my-resource.openai.azure.com/",
            "key",
            "gpt-4o",
        );
        let p = AzureOpenAIProvider::new(cfg).unwrap();
        let url = p.endpoint_url("gpt-5-chat");
        assert!(url.contains("/openai/deployments/gpt-5-chat/chat/completions"));
        assert!(url.contains("api-version=2024-10-21"));
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
        let body = p.build_body(&req, "o3");
        assert!(body.get("temperature").is_none());
        assert_eq!(body["reasoning_effort"], "medium");
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
        let body = p.build_body(&req, "gpt-4o");
        assert_eq!(body["temperature"], 0.5);
    }

    #[test]
    fn missing_key_or_endpoint_is_rejected() {
        assert!(AzureOpenAIProvider::new(AzureOpenAIConfig::new("", "k", "d")).is_err());
        assert!(AzureOpenAIProvider::new(AzureOpenAIConfig::new("e", "", "d")).is_err());
    }
}
