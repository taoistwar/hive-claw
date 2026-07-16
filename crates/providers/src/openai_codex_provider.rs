//! OpenAI Codex Responses API provider.
//!
//! Talks to `https://chatgpt.com/backend-api/codex/responses` using a
//! pre-persisted OAuth token (see [`crate::oauth`]). This mirrors
//! `nanobot.providers.openai_codex_provider`.
//!
//! The token must be obtained elsewhere (via the `codex` CLI or
//! equivalent OAuth dance). We load it from `~/.nanobot/codex-auth.json`
//! or `~/.codex/auth.json`, matching the original Python fallback.

use std::time::Duration;

use async_trait::async_trait;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::base::extract_retry_after_from_text;
use crate::base::{ChatRequest, LLMProvider};
use crate::base::{GenerationSettings, LLMResponse, ToolChoice};
use crate::oauth::{FileTokenStorage, OAuthToken};
use crate::responses::{consume_events, convert_messages, convert_tools, parse_sse_events};

pub const DEFAULT_CODEX_URL: &str = "https://chatgpt.com/backend-api/codex/responses";
pub const DEFAULT_ORIGINATOR: &str = "nanobot";
pub const TOKEN_FILENAME: &str = "codex-auth.json";
pub const TOKEN_APP_NAME: &str = "nanobot";

/// Configuration for [`OpenAICodexProvider`].
#[derive(Debug, Clone)]
pub struct OpenAICodexConfig {
    pub default_model: String,
    pub codex_url: String,
    pub originator: String,
    /// Override the stored token (mostly for tests).
    pub static_token: Option<OAuthToken>,
    pub timeout: Duration,
}

impl OpenAICodexConfig {
    pub fn new(default_model: impl Into<String>) -> Self {
        Self {
            default_model: default_model.into(),
            ..Self::default()
        }
    }
    pub fn with_codex_url(mut self, url: impl Into<String>) -> Self {
        self.codex_url = url.into();
        self
    }
    pub fn with_static_token(mut self, token: OAuthToken) -> Self {
        self.static_token = Some(token);
        self
    }
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

impl Default for OpenAICodexConfig {
    fn default() -> Self {
        Self {
            default_model: "openai-codex/gpt-5.1-codex".into(),
            codex_url: DEFAULT_CODEX_URL.into(),
            originator: DEFAULT_ORIGINATOR.into(),
            static_token: None,
            timeout: Duration::from_secs(60),
        }
    }
}

pub struct OpenAICodexProvider {
    config: OpenAICodexConfig,
    client: reqwest::Client,
    storage: FileTokenStorage,
}

impl OpenAICodexProvider {
    pub fn new(config: OpenAICodexConfig) -> Result<Self, reqwest::Error> {
        let client = reqwest::Client::builder().timeout(config.timeout).build()?;
        let storage = FileTokenStorage::new(TOKEN_FILENAME, TOKEN_APP_NAME, true);
        Ok(Self {
            config,
            client,
            storage,
        })
    }

    pub fn storage(&self) -> &FileTokenStorage {
        &self.storage
    }

    pub fn default_url() -> &'static str {
        DEFAULT_CODEX_URL
    }

    fn load_token(&self) -> Option<OAuthToken> {
        if let Some(t) = &self.config.static_token {
            return Some(t.clone());
        }
        self.storage.load()
    }

    fn build_headers(&self, token: &OAuthToken) -> Result<HeaderMap, String> {
        let mut headers = HeaderMap::new();
        let bearer = format!("Bearer {}", token.access);
        headers.insert(
            HeaderName::from_static("authorization"),
            HeaderValue::from_str(&bearer).map_err(|e| e.to_string())?,
        );
        if let Some(account) = token.account_id.as_deref() {
            headers.insert(
                HeaderName::from_static("chatgpt-account-id"),
                HeaderValue::from_str(account).map_err(|e| e.to_string())?,
            );
        }
        headers.insert(
            HeaderName::from_static("openai-beta"),
            HeaderValue::from_static("responses=experimental"),
        );
        headers.insert(
            HeaderName::from_static("originator"),
            HeaderValue::from_str(&self.config.originator).map_err(|e| e.to_string())?,
        );
        headers.insert(
            HeaderName::from_static("user-agent"),
            HeaderValue::from_static("nanobot (python)"),
        );
        headers.insert(
            HeaderName::from_static("accept"),
            HeaderValue::from_static("text/event-stream"),
        );
        headers.insert(
            HeaderName::from_static("content-type"),
            HeaderValue::from_static("application/json"),
        );
        Ok(headers)
    }

    fn build_body(&self, req: &ChatRequest) -> Value {
        let model = req
            .model
            .clone()
            .unwrap_or_else(|| self.config.default_model.clone());
        let model_stripped = strip_model_prefix(&model);
        let (system_prompt, input_items) = convert_messages(&req.messages);

        let mut body = json!({
            "model": model_stripped,
            "store": false,
            "stream": true,
            "instructions": system_prompt,
            "input": input_items,
            "text": {"verbosity": "medium"},
            "include": ["reasoning.encrypted_content"],
            "prompt_cache_key": prompt_cache_key(&req.messages),
            "parallel_tool_calls": true,
        });

        body["tool_choice"] = match &req.tool_choice {
            Some(ToolChoice::Auto) | None => Value::String("auto".into()),
            Some(ToolChoice::Required) => Value::String("required".into()),
            Some(ToolChoice::None) => Value::String("none".into()),
            Some(ToolChoice::Specific(v)) => v.clone(),
        };

        if let Some(effort) = req.reasoning_effort.as_deref() {
            if effort.to_lowercase() != "none" {
                body["reasoning"] = json!({"effort": effort});
            }
        }
        if let Some(tools) = req.tools.as_ref() {
            body["tools"] = Value::Array(convert_tools(tools));
        }
        body
    }
}

#[async_trait]
impl LLMProvider for OpenAICodexProvider {
    fn default_model(&self) -> String {
        self.config.default_model.clone()
    }

    fn generation(&self) -> GenerationSettings {
        GenerationSettings {
            temperature: 0.7,
            max_tokens: 4096,
            reasoning_effort: None,
        }
    }

    async fn chat(&self, req: ChatRequest) -> LLMResponse {
        let Some(token) = self.load_token() else {
            return LLMResponse::error(
                "OpenAI Codex is not logged in. Run the Codex CLI to obtain an OAuth token.",
            );
        };
        let headers = match self.build_headers(&token) {
            Ok(h) => h,
            Err(e) => return LLMResponse::error(format!("Invalid Codex headers: {e}")),
        };
        let body = self.build_body(&req);

        let response = match self
            .client
            .post(&self.config.codex_url)
            .headers(headers)
            .json(&body)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                return LLMResponse {
                    content: Some(format!("Error calling Codex: {e}")),
                    finish_reason: "error".into(),
                    error_kind: Some(classify_reqwest_error(&e)),
                    ..Default::default()
                };
            }
        };

        let status = response.status();
        if !status.is_success() {
            let retry_after_hdr = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<f64>().ok());
            let text = response.text().await.unwrap_or_default();
            let retry_after =
                retry_after_hdr.or_else(|| extract_retry_after_from_text(Some(&text)));
            let message = friendly_error(status.as_u16(), &text);
            return LLMResponse {
                content: Some(message),
                finish_reason: "error".into(),
                error_status_code: Some(status.as_u16() as i32),
                retry_after,
                error_retry_after_s: retry_after,
                ..Default::default()
            };
        }

        // Read the full SSE body; for parity with Python we don't do
        // true streaming here (the caller doesn't receive deltas).
        let body_text = match response.text().await {
            Ok(t) => t,
            Err(e) => return LLMResponse::error(format!("Error reading Codex stream: {e}")),
        };
        let events = parse_sse_events(&body_text);
        match consume_events(&events) {
            Ok((content, tool_calls, finish_reason)) => LLMResponse {
                content: (!content.is_empty()).then_some(content),
                tool_calls,
                finish_reason,
                ..Default::default()
            },
            Err(err) => LLMResponse {
                content: Some(err.clone()),
                finish_reason: "error".into(),
                retry_after: extract_retry_after_from_text(Some(&err)),
                ..Default::default()
            },
        }
    }
}

fn strip_model_prefix(model: &str) -> String {
    for prefix in ["openai-codex/", "openai_codex/"] {
        if let Some(rest) = model.strip_prefix(prefix) {
            return rest.to_string();
        }
    }
    model.to_string()
}

fn sort_value(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let sorted: std::collections::BTreeMap<_, _> = map
                .iter()
                .map(|(k, v)| (k.clone(), sort_value(v)))
                .collect();
            Value::Object(sorted.into_iter().collect())
        }
        Value::Array(arr) => Value::Array(arr.iter().map(sort_value).collect()),
        other => other.clone(),
    }
}

fn prompt_cache_key(messages: &[Value]) -> String {
    let sorted = messages.iter().map(sort_value).collect::<Vec<_>>();
    let raw = serde_json::to_string(&sorted).unwrap_or_else(|_| "[]".into());
    let mut hasher = Sha256::new();
    hasher.update(raw.as_bytes());
    let digest = hasher.finalize();
    let mut s = String::with_capacity(64);
    for b in digest {
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
    }
    s
}

fn friendly_error(status: u16, raw: &str) -> String {
    if status == 429 {
        return "ChatGPT usage quota exceeded or rate limit triggered. Please try again later."
            .to_string();
    }
    format!("HTTP {status}: {raw}")
}

fn classify_reqwest_error(err: &reqwest::Error) -> String {
    if err.is_timeout() {
        "timeout".into()
    } else if err.is_connect() {
        "connection".into()
    } else {
        "other".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_openai_codex_prefix() {
        assert_eq!(strip_model_prefix("openai-codex/gpt-5"), "gpt-5");
        assert_eq!(strip_model_prefix("openai_codex/gpt-5"), "gpt-5");
        assert_eq!(strip_model_prefix("gpt-5"), "gpt-5");
    }

    #[test]
    fn prompt_cache_key_stable() {
        let messages = vec![json!({"role":"user","content":"hi"})];
        let a = prompt_cache_key(&messages);
        let b = prompt_cache_key(&messages);
        assert_eq!(a, b);
        assert_eq!(a.len(), 64);
    }

    #[test]
    fn build_body_includes_tools_and_effort() {
        let cfg = OpenAICodexConfig::default();
        let client = reqwest::Client::new();
        let provider = OpenAICodexProvider {
            config: cfg,
            client,
            storage: FileTokenStorage::new(TOKEN_FILENAME, TOKEN_APP_NAME, true),
        };
        let req = ChatRequest {
            messages: vec![
                json!({"role":"system","content":"sys"}),
                json!({"role":"user","content":"hi"}),
            ],
            tools: Some(vec![json!({
                "type":"function",
                "function":{"name":"x","description":"d","parameters":{"type":"object"}}
            })]),
            model: Some("openai-codex/gpt-5".into()),
            reasoning_effort: Some("medium".into()),
            ..Default::default()
        };
        let body = provider.build_body(&req);
        assert_eq!(body["model"], "gpt-5");
        assert_eq!(body["instructions"], "sys");
        assert_eq!(body["stream"], true);
        assert_eq!(body["tool_choice"], "auto");
        assert!(body["tools"].is_array());
        assert_eq!(body["tools"][0]["name"], "x");
        assert_eq!(body["reasoning"]["effort"], "medium");
    }

    #[test]
    fn friendly_error_429() {
        let s = friendly_error(429, "quota exceeded");
        assert!(s.starts_with("ChatGPT"));
    }

    #[test]
    fn friendly_error_other() {
        let s = friendly_error(500, "oops");
        assert!(s.contains("HTTP 500"));
        assert!(s.contains("oops"));
    }
}
