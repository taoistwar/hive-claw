//! Web-search provider usage fetchers for `/status` (port of
//! `nanobot.utils.searchusage`).
//!
//! Network calls are delegated to a pluggable trait so this crate does not
//! pull in `reqwest` / `tokio-rt-multi-thread` directly. The `channels` or
//! `providers` crate can implement [`HttpClient`] with a real backend.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Structured usage info returned by a provider fetcher.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SearchUsageInfo {
    pub provider: String,
    #[serde(default)]
    pub supported: bool,
    #[serde(default)]
    pub error: Option<String>,

    #[serde(default)]
    pub used: Option<u64>,
    #[serde(default)]
    pub limit: Option<u64>,
    #[serde(default)]
    pub remaining: Option<u64>,
    #[serde(default)]
    pub reset_date: Option<String>,

    #[serde(default)]
    pub search_used: Option<u64>,
    #[serde(default)]
    pub extract_used: Option<u64>,
    #[serde(default)]
    pub crawl_used: Option<u64>,
}

impl SearchUsageInfo {
    /// Human-readable multi-line string for `/status` output.
    pub fn format(&self) -> String {
        let mut lines = vec![format!("\u{1f50d} Web Search: {}", self.provider)];
        if !self.supported {
            lines.push("   Usage tracking: not available for this provider".into());
            return lines.join("\n");
        }
        if let Some(err) = &self.error {
            lines.push(format!("   Usage: unavailable ({err})"));
            return lines.join("\n");
        }
        if let (Some(u), Some(l)) = (self.used, self.limit) {
            lines.push(format!("   Usage: {u} / {l} requests"));
        } else if let Some(u) = self.used {
            lines.push(format!("   Usage: {u} requests"));
        }
        let mut breakdown = Vec::new();
        if let Some(s) = self.search_used {
            breakdown.push(format!("Search: {s}"));
        }
        if let Some(e) = self.extract_used {
            breakdown.push(format!("Extract: {e}"));
        }
        if let Some(c) = self.crawl_used {
            breakdown.push(format!("Crawl: {c}"));
        }
        if !breakdown.is_empty() {
            lines.push(format!("   Breakdown: {}", breakdown.join(" | ")));
        }
        if let Some(r) = self.remaining {
            lines.push(format!("   Remaining: {r} requests"));
        }
        if let Some(d) = &self.reset_date {
            lines.push(format!("   Resets: {d}"));
        }
        lines.join("\n")
    }
}

/// Simple HTTP client abstraction so this crate doesn't depend on `reqwest`
/// directly. Returns the body on 2xx, a stringified HTTP-status error on
/// non-success, or transport errors as `Err`.
#[async_trait]
pub trait HttpClient: Send + Sync {
    async fn get_with_bearer(&self, url: &str, token: &str) -> Result<String, HttpError>;
}

#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    #[error("HTTP {status}")]
    Status { status: u16 },
    #[error("{0}")]
    Transport(String),
}

/// Fetch usage info for the configured web-search provider.
///
/// Only `tavily` talks to an upstream API; every other provider reports
/// `supported = false`.
pub async fn fetch_search_usage(
    client: Option<&dyn HttpClient>,
    provider: &str,
    api_key: Option<&str>,
) -> SearchUsageInfo {
    let p = if provider.is_empty() {
        "duckduckgo".to_string()
    } else {
        provider.trim().to_lowercase()
    };
    if p == "tavily" {
        return fetch_tavily_usage(client, api_key).await;
    }
    SearchUsageInfo {
        provider: p,
        supported: false,
        ..Default::default()
    }
}

async fn fetch_tavily_usage(
    client: Option<&dyn HttpClient>,
    api_key: Option<&str>,
) -> SearchUsageInfo {
    let key = api_key
        .map(str::to_string)
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("TAVILY_API_KEY").ok())
        .unwrap_or_default();
    if key.is_empty() {
        return SearchUsageInfo {
            provider: "tavily".into(),
            supported: true,
            error: Some("TAVILY_API_KEY not configured".into()),
            ..Default::default()
        };
    }
    let Some(client) = client else {
        return SearchUsageInfo {
            provider: "tavily".into(),
            supported: true,
            error: Some("no HTTP client configured".into()),
            ..Default::default()
        };
    };

    match client
        .get_with_bearer("https://api.tavily.com/usage", &key)
        .await
    {
        Ok(body) => match serde_json::from_str::<Value>(&body) {
            Ok(data) => parse_tavily_usage(&data),
            Err(e) => SearchUsageInfo {
                provider: "tavily".into(),
                supported: true,
                error: Some(short(&e.to_string())),
                ..Default::default()
            },
        },
        Err(HttpError::Status { status }) => SearchUsageInfo {
            provider: "tavily".into(),
            supported: true,
            error: Some(format!("HTTP {status}")),
            ..Default::default()
        },
        Err(HttpError::Transport(e)) => SearchUsageInfo {
            provider: "tavily".into(),
            supported: true,
            error: Some(short(&e)),
            ..Default::default()
        },
    }
}

fn short(s: &str) -> String {
    s.chars().take(80).collect()
}

fn parse_tavily_usage(data: &Value) -> SearchUsageInfo {
    let account = data.get("account").cloned().unwrap_or(Value::Null);
    let get_u64 = |key: &str| account.get(key).and_then(Value::as_u64);
    let used = get_u64("plan_usage");
    let limit = get_u64("plan_limit");
    let remaining = match (used, limit) {
        (Some(u), Some(l)) => Some(l.saturating_sub(u)),
        _ => None,
    };
    SearchUsageInfo {
        provider: "tavily".into(),
        supported: true,
        used,
        limit,
        remaining,
        search_used: get_u64("search_usage"),
        extract_used: get_u64("extract_usage"),
        crawl_used: get_u64("crawl_usage"),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_unsupported() {
        let info = SearchUsageInfo {
            provider: "brave".into(),
            supported: false,
            ..Default::default()
        };
        let text = info.format();
        assert!(text.contains("Usage tracking: not available"));
    }

    #[test]
    fn parses_tavily_account() {
        let data: Value = serde_json::from_str(
            r#"{"account":{"plan_usage":20,"plan_limit":1000,"search_usage":20}}"#,
        )
        .unwrap();
        let info = parse_tavily_usage(&data);
        assert_eq!(info.used, Some(20));
        assert_eq!(info.limit, Some(1000));
        assert_eq!(info.remaining, Some(980));
        assert_eq!(info.search_used, Some(20));
    }
}
