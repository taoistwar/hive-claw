//! Transient-error classification and retry-after parsing.
//!
//! Mirrors the static tables in `nanobot.providers.base` so the Rust port
//! makes the same accept/reject decisions.

use once_cell::sync::Lazy;
use regex::Regex;

use crate::types::LLMResponse;

const TRANSIENT_ERROR_MARKERS: &[&str] = &[
    "429",
    "rate limit",
    "500",
    "502",
    "503",
    "504",
    "overloaded",
    "timeout",
    "timed out",
    "connection",
    "server error",
    "temporarily unavailable",
    "速率限制",
];

const NON_RETRYABLE_429_TEXT: &[&str] = &[
    "insufficient_quota",
    "insufficient quota",
    "quota exceeded",
    "quota exhausted",
    "billing hard limit",
    "billing_hard_limit_reached",
    "billing not active",
    "insufficient balance",
    "insufficient_balance",
    "credit balance too low",
    "payment required",
    "out of credits",
    "out of quota",
    "exceeded your current quota",
];

const RETRYABLE_429_TEXT: &[&str] = &[
    "rate limit",
    "rate_limit",
    "too many requests",
    "retry after",
    "try again in",
    "temporarily unavailable",
    "overloaded",
    "concurrency limit",
    "速率限制",
];

const NON_RETRYABLE_429_TOKENS: &[&str] = &[
    "insufficient_quota",
    "quota_exceeded",
    "quota_exhausted",
    "billing_hard_limit_reached",
    "insufficient_balance",
    "credit_balance_too_low",
    "billing_not_active",
    "payment_required",
];

const RETRYABLE_429_TOKENS: &[&str] = &[
    "rate_limit_exceeded",
    "rate_limit_error",
    "too_many_requests",
    "request_limit_exceeded",
    "requests_limit_exceeded",
    "overloaded_error",
];

const RETRYABLE_STATUS_CODES: &[i32] = &[408, 409, 429];
const TRANSIENT_ERROR_KINDS: &[&str] = &["timeout", "connection"];

pub fn is_transient_text(content: Option<&str>) -> bool {
    let Some(s) = content else {
        return false;
    };
    let lower = s.to_ascii_lowercase();
    TRANSIENT_ERROR_MARKERS
        .iter()
        .any(|m| lower.contains(m))
}

pub fn is_transient_response(resp: &LLMResponse) -> bool {
    if let Some(flag) = resp.error_should_retry {
        return flag;
    }

    if let Some(status) = resp.error_status_code {
        if status == 429 {
            return is_retryable_429(resp);
        }
        if RETRYABLE_STATUS_CODES.contains(&status) || status >= 500 {
            return true;
        }
    }

    if let Some(kind) = resp.error_kind.as_deref() {
        let k = kind.trim().to_ascii_lowercase();
        if TRANSIENT_ERROR_KINDS.contains(&k.as_str()) {
            return true;
        }
    }

    is_transient_text(resp.content.as_deref())
}

fn normalize_token(v: Option<&str>) -> Option<String> {
    v.map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
}

fn is_retryable_429(resp: &LLMResponse) -> bool {
    let ty = normalize_token(resp.error_type.as_deref());
    let code = normalize_token(resp.error_code.as_deref());
    let tokens: Vec<&str> = [ty.as_deref(), code.as_deref()]
        .into_iter()
        .flatten()
        .collect();

    if tokens
        .iter()
        .any(|t| NON_RETRYABLE_429_TOKENS.contains(t))
    {
        return false;
    }

    let content_lower = resp
        .content
        .as_deref()
        .unwrap_or("")
        .to_ascii_lowercase();
    if NON_RETRYABLE_429_TEXT
        .iter()
        .any(|m| content_lower.contains(m))
    {
        return false;
    }

    if tokens.iter().any(|t| RETRYABLE_429_TOKENS.contains(t)) {
        return true;
    }
    if RETRYABLE_429_TEXT
        .iter()
        .any(|m| content_lower.contains(m))
    {
        return true;
    }
    // Unknown 429 => retry.
    true
}

static RETRY_AFTER_RES: Lazy<[Regex; 4]> = Lazy::new(|| {
    [
        Regex::new(r"retry after\s+(\d+(?:\.\d+)?)\s*(ms|milliseconds|s|sec|secs|seconds|m|min|minutes)?").unwrap(),
        Regex::new(r"try again in\s+(\d+(?:\.\d+)?)\s*(ms|milliseconds|s|sec|secs|seconds|m|min|minutes)").unwrap(),
        Regex::new(r"wait\s+(\d+(?:\.\d+)?)\s*(ms|milliseconds|s|sec|secs|seconds|m|min|minutes)\s*before retry").unwrap(),
        Regex::new(r#"retry[_-]?after["'\s:=]+(\d+(?:\.\d+)?)"#).unwrap(),
    ]
});

pub fn extract_retry_after_from_text(content: Option<&str>) -> Option<f64> {
    let text = content?.to_ascii_lowercase();
    for (idx, re) in RETRY_AFTER_RES.iter().enumerate() {
        if let Some(cap) = re.captures(&text) {
            let value: f64 = cap.get(1)?.as_str().parse().ok()?;
            let unit = if idx < 3 {
                cap.get(2).map(|m| m.as_str().to_string())
            } else {
                Some("s".into())
            };
            return Some(to_retry_seconds(value, unit.as_deref()));
        }
    }
    None
}

fn to_retry_seconds(value: f64, unit: Option<&str>) -> f64 {
    let u = unit.unwrap_or("s").to_ascii_lowercase();
    match u.as_str() {
        "ms" | "milliseconds" => (value / 1000.0).max(0.1),
        "m" | "min" | "minutes" => (value * 60.0).max(0.1),
        _ => value.max(0.1),
    }
}

/// Preferred delay source: explicit `error_retry_after_s` > `retry_after` > text.
pub fn pick_delay(resp: &LLMResponse) -> Option<f64> {
    if let Some(v) = resp.error_retry_after_s {
        if v > 0.0 {
            return Some(v);
        }
    }
    if let Some(v) = resp.retry_after {
        if v > 0.0 {
            return Some(v);
        }
    }
    extract_retry_after_from_text(resp.content.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_transient_text() {
        assert!(is_transient_text(Some("Got 429 rate limit")));
        assert!(is_transient_text(Some("Timed out")));
        assert!(!is_transient_text(Some("Invalid API key")));
    }

    #[test]
    fn picks_retry_after_seconds() {
        assert_eq!(
            extract_retry_after_from_text(Some("please retry after 3 seconds")),
            Some(3.0)
        );
        assert_eq!(
            extract_retry_after_from_text(Some("try again in 1500 ms")),
            Some(1.5)
        );
    }

    #[test]
    fn status_429_with_quota_not_retryable() {
        let resp = LLMResponse {
            error_status_code: Some(429),
            error_type: Some("insufficient_quota".into()),
            ..Default::default()
        };
        assert!(!is_transient_response(&resp));
    }
}
