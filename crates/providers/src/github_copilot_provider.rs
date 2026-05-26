//! GitHub Copilot OAuth-backed provider.
//!
//! Port of `nanobot.providers.github_copilot_provider`:
//! * Device-flow login that persists a GitHub OAuth token.
//! * Runtime wrapper that exchanges that token for short-lived Copilot
//!   access tokens, then delegates to the OpenAI-compatible provider.

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::RwLock;

use crate::oauth::{FileTokenStorage, OAuthToken};
use crate::openai_compat_provider::{OpenAICompatConfig, OpenAICompatProvider};
use crate::base::{ChatRequest, LLMProvider};
use crate::registry::find_by_name;
use crate::base::{GenerationSettings, LLMResponse};

pub const GITHUB_DEVICE_CODE_URL: &str = "https://github.com/login/device/code";
pub const GITHUB_ACCESS_TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
pub const GITHUB_USER_URL: &str = "https://api.github.com/user";
pub const COPILOT_TOKEN_URL: &str = "https://api.github.com/copilot_internal/v2/token";
pub const COPILOT_BASE_URL: &str = "https://api.githubcopilot.com";
pub const GITHUB_COPILOT_CLIENT_ID: &str = "Iv1.b507a08c87ecfe98";
pub const GITHUB_COPILOT_SCOPE: &str = "read:user";
pub const TOKEN_FILENAME: &str = "github-copilot.json";
pub const TOKEN_APP_NAME: &str = "nanobot";
pub const USER_AGENT: &str = "nanobot/0.1";
pub const EDITOR_VERSION: &str = "vscode/1.99.0";
pub const EDITOR_PLUGIN_VERSION: &str = "copilot-chat/0.26.0";

const EXPIRY_SKEW_SECONDS: u64 = 60;
const LONG_LIVED_TOKEN_SECONDS: i64 = 315_360_000;

/// Persisted GitHub OAuth token (or `None` if not logged in).
pub fn get_github_copilot_login_status() -> Option<OAuthToken> {
    storage().load()
}

fn storage() -> FileTokenStorage {
    FileTokenStorage::new(TOKEN_FILENAME, TOKEN_APP_NAME, false)
}

fn copilot_headers(token: &str) -> Vec<(&'static str, String)> {
    vec![
        ("Authorization", format!("token {token}")),
        ("Accept", "application/json".into()),
        ("User-Agent", USER_AGENT.into()),
        ("Editor-Version", EDITOR_VERSION.into()),
        ("Editor-Plugin-Version", EDITOR_PLUGIN_VERSION.into()),
    ]
}

/// Result payload from [`DeviceCodeInit::request`].
#[derive(Debug, Clone)]
pub struct DeviceCodeInfo {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: String,
    pub interval: u64,
    pub expires_in: u64,
}

/// Two-phase device flow helper:
///
/// 1. [`DeviceFlow::request`] – fetch a device code.
/// 2. [`DeviceFlow::poll_until_token`] – poll the token endpoint until
///    the user finishes authorising.
///
/// We intentionally split these out of `login()` so callers can render
/// the `user_code` however they like (CLI print, UI modal, …).
pub struct DeviceFlow {
    client: reqwest::Client,
}

impl DeviceFlow {
    pub fn new() -> Result<Self, reqwest::Error> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .build()?;
        Ok(Self { client })
    }

    pub async fn request(&self) -> Result<DeviceCodeInfo, String> {
        let response = self
            .client
            .post(GITHUB_DEVICE_CODE_URL)
            .header("Accept", "application/json")
            .header("User-Agent", USER_AGENT)
            .form(&[
                ("client_id", GITHUB_COPILOT_CLIENT_ID),
                ("scope", GITHUB_COPILOT_SCOPE),
            ])
            .send()
            .await
            .map_err(|e| format!("device_code request failed: {e}"))?;
        if !response.status().is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("device_code HTTP error: {text}"));
        }
        let body: Value = response
            .json()
            .await
            .map_err(|e| format!("device_code bad JSON: {e}"))?;

        let device_code = body
            .get("device_code")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "device_code missing".to_string())?
            .to_string();
        let user_code = body
            .get("user_code")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let verification_uri = body
            .get("verification_uri")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let verification_uri_complete = body
            .get("verification_uri_complete")
            .and_then(|v| v.as_str())
            .unwrap_or(&verification_uri)
            .to_string();
        let interval = body.get("interval").and_then(|v| v.as_u64()).unwrap_or(5).max(1);
        let expires_in = body
            .get("expires_in")
            .and_then(|v| v.as_u64())
            .unwrap_or(900);

        Ok(DeviceCodeInfo {
            device_code,
            user_code,
            verification_uri,
            verification_uri_complete,
            interval,
            expires_in,
        })
    }

    /// Poll the access-token endpoint until a token is issued or the
    /// device code expires. Returns the final persisted [`OAuthToken`].
    pub async fn poll_until_token(&self, info: &DeviceCodeInfo) -> Result<OAuthToken, String> {
        let deadline = Instant::now() + Duration::from_secs(info.expires_in);
        let mut interval = info.interval;
        let (access_token, token_expires_in) = loop {
            if Instant::now() >= deadline {
                return Err("GitHub device flow timed out.".to_string());
            }
            let poll = self
                .client
                .post(GITHUB_ACCESS_TOKEN_URL)
                .header("Accept", "application/json")
                .header("User-Agent", USER_AGENT)
                .form(&[
                    ("client_id", GITHUB_COPILOT_CLIENT_ID),
                    ("device_code", info.device_code.as_str()),
                    (
                        "grant_type",
                        "urn:ietf:params:oauth:grant-type:device_code",
                    ),
                ])
                .send()
                .await
                .map_err(|e| format!("access_token request failed: {e}"))?;
            if !poll.status().is_success() {
                let text = poll.text().await.unwrap_or_default();
                return Err(format!("access_token HTTP error: {text}"));
            }
            let body: Value = poll
                .json()
                .await
                .map_err(|e| format!("access_token bad JSON: {e}"))?;

            if let Some(token) = body.get("access_token").and_then(|v| v.as_str()) {
                let expires_in = body
                    .get("expires_in")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(LONG_LIVED_TOKEN_SECONDS);
                break (token.to_string(), expires_in);
            }
            let error = body.get("error").and_then(|v| v.as_str()).unwrap_or("");
            match error {
                "authorization_pending" => {}
                "slow_down" => interval += 5,
                "expired_token" => {
                    return Err("GitHub device code expired. Please run login again.".into());
                }
                "access_denied" => return Err("GitHub device flow was denied.".into()),
                "" => {}
                other => {
                    let desc = body
                        .get("error_description")
                        .and_then(|v| v.as_str())
                        .unwrap_or(other);
                    return Err(desc.to_string());
                }
            }
            tokio::time::sleep(Duration::from_secs(interval)).await;
        };

        // Resolve account_id via /user.
        let account_id = self
            .client
            .get(GITHUB_USER_URL)
            .header("Authorization", format!("Bearer {access_token}"))
            .header("Accept", "application/vnd.github+json")
            .header("User-Agent", USER_AGENT)
            .send()
            .await
            .ok()
            .and_then(|resp| if resp.status().is_success() { Some(resp) } else { None })
            .map(|resp| async move {
                resp.json::<Value>()
                    .await
                    .ok()
                    .and_then(|body| {
                        body.get("login")
                            .and_then(|v| v.as_str())
                            .map(String::from)
                            .or_else(|| {
                                body.get("id")
                                    .and_then(|v| v.as_u64())
                                    .map(|n| n.to_string())
                            })
                    })
            });
        let account_id = if let Some(fut) = account_id { fut.await } else { None };

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let expires_ms = now_ms + token_expires_in * 1_000;
        let token = OAuthToken {
            access: access_token,
            refresh: String::new(),
            expires: expires_ms,
            account_id,
        };
        storage().save(&token).map_err(|e| e.to_string())?;
        Ok(token)
    }
}

/// High-level helper: run the full device flow, printing prompts via
/// the supplied closure (defaults to stderr).
pub async fn login_github_copilot<F>(mut printer: F) -> Result<OAuthToken, String>
where
    F: FnMut(&str),
{
    let flow = DeviceFlow::new().map_err(|e| e.to_string())?;
    let info = flow.request().await?;
    printer(&format!("Open: {}", info.verification_uri));
    printer(&format!("Code: {}", info.user_code));
    flow.poll_until_token(&info).await
}

/// Cached short-lived Copilot access token.
#[derive(Debug, Clone, Default)]
struct CopilotTokenCache {
    token: Option<String>,
    expires_at_ms: i64,
}

/// Copilot-backed provider. Uses OpenAI-compatible wire format via
/// `api.githubcopilot.com`, refreshing the access token on demand.
pub struct GitHubCopilotProvider {
    default_model: String,
    client: reqwest::Client,
    cache: Arc<RwLock<CopilotTokenCache>>,
    generation: GenerationSettings,
    spec: Option<&'static crate::registry::ProviderSpec>,
    /// When set, bypass HTTP token exchange (tests).
    static_copilot_token: Option<String>,
    extra_base_url: Option<String>,
}

impl GitHubCopilotProvider {
    pub fn new(default_model: impl Into<String>) -> Result<Self, reqwest::Error> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .build()?;
        Ok(Self {
            default_model: default_model.into(),
            client,
            cache: Arc::new(RwLock::new(CopilotTokenCache::default())),
            generation: GenerationSettings::default(),
            spec: find_by_name("github_copilot"),
            static_copilot_token: None,
            extra_base_url: None,
        })
    }

    /// Override the Copilot base URL (used by tests against a mock server).
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.extra_base_url = Some(base_url.into());
        self
    }

    /// Supply a pre-acquired Copilot access token (tests).
    pub fn with_static_token(mut self, token: impl Into<String>) -> Self {
        self.static_copilot_token = Some(token.into());
        self
    }

    async fn get_copilot_access_token(&self) -> Result<String, String> {
        if let Some(t) = &self.static_copilot_token {
            return Ok(t.clone());
        }

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        {
            let guard = self.cache.read().await;
            if let Some(tok) = &guard.token {
                if now_ms + (EXPIRY_SKEW_SECONDS as i64) * 1_000 < guard.expires_at_ms {
                    return Ok(tok.clone());
                }
            }
        }

        let github_token = storage().load().ok_or_else(|| {
            "GitHub Copilot is not logged in. Run: nanobot provider login github-copilot".to_string()
        })?;
        if github_token.access.is_empty() {
            return Err(
                "GitHub Copilot is not logged in. Run: nanobot provider login github-copilot"
                    .into(),
            );
        }

        let mut request = self.client.get(COPILOT_TOKEN_URL);
        for (k, v) in copilot_headers(&github_token.access) {
            request = request.header(k, v);
        }
        let response = request
            .send()
            .await
            .map_err(|e| format!("Copilot token exchange failed: {e}"))?;
        if !response.status().is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Copilot token exchange HTTP error: {text}"));
        }
        let body: Value = response
            .json()
            .await
            .map_err(|e| format!("Copilot token bad JSON: {e}"))?;
        let token = body
            .get("token")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Copilot token exchange returned no token.".to_string())?
            .to_string();
        let expires_at_s = body.get("expires_at").and_then(|v| v.as_i64());
        let refresh_in = body
            .get("refresh_in")
            .and_then(|v| v.as_i64())
            .unwrap_or(1500);
        let expires_at_ms = expires_at_s
            .map(|s| s * 1_000)
            .unwrap_or(now_ms + refresh_in * 1_000);

        let mut guard = self.cache.write().await;
        guard.token = Some(token.clone());
        guard.expires_at_ms = expires_at_ms;
        Ok(token)
    }

    fn build_underlying(&self, token: String) -> OpenAICompatProvider {
        let base = self
            .extra_base_url
            .clone()
            .unwrap_or_else(|| COPILOT_BASE_URL.to_string());
        let mut cfg = OpenAICompatConfig::new(self.default_model.clone())
            .with_api_key(token)
            .with_api_base(base)
            .with_extra_header("Editor-Version", EDITOR_VERSION)
            .with_extra_header("Editor-Plugin-Version", EDITOR_PLUGIN_VERSION)
            .with_extra_header("User-Agent", USER_AGENT);
        if let Some(spec) = self.spec {
            cfg = cfg.with_spec(spec);
        }
        OpenAICompatProvider::new(cfg).with_generation(self.generation.clone())
    }
}

#[async_trait]
impl LLMProvider for GitHubCopilotProvider {
    fn default_model(&self) -> String {
        self.default_model.clone()
    }

    fn generation(&self) -> GenerationSettings {
        self.generation.clone()
    }

    fn supports_progress_deltas(&self) -> bool {
        true
    }

    async fn chat(&self, req: ChatRequest) -> LLMResponse {
        let token = match self.get_copilot_access_token().await {
            Ok(t) => t,
            Err(e) => return LLMResponse::error(e),
        };
        let inner = self.build_underlying(token);
        inner.chat(req).await
    }

    async fn chat_stream(
        &self,
        req: ChatRequest,
        on_delta: Option<crate::base::StreamDeltaCallback>,
        on_tool_call_delta: Option<crate::responses::ToolCallDeltaCallback>,
    ) -> LLMResponse {
        let token = match self.get_copilot_access_token().await {
            Ok(t) => t,
            Err(e) => return LLMResponse::error(e),
        };
        let inner = self.build_underlying(token);
        inner.chat_stream(req, on_delta, on_tool_call_delta).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn static_token_short_circuits_exchange() {
        let provider = GitHubCopilotProvider::new("github-copilot/gpt-4.1")
            .unwrap()
            .with_static_token("stub")
            .with_base_url("http://127.0.0.1:1"); // unreachable, exchange skipped
        let tok = provider.get_copilot_access_token().await.unwrap();
        assert_eq!(tok, "stub");
    }

    #[test]
    fn copilot_headers_include_editor() {
        let hs = copilot_headers("abc");
        assert!(hs.iter().any(|(k, v)| *k == "Authorization" && v == "token abc"));
        assert!(hs.iter().any(|(k, _)| *k == "Editor-Version"));
    }
}
