//! Shared chat utilities for admin and user chat APIs.
//!
//! Contains SSE concurrency control, query structs, common helpers, and MD5 sign verification.

use axum::{
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
};
use futures::stream::Stream;
use serde::Deserialize;
use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::sync::Mutex;

// --- MD5 Sign Verification ---

/// Verify MD5 signature: `MD5(SECRET + "{path}?body={body}")`
pub fn verify_sign(secret: &str, path: &str, body: &str, expected_sign: &str) -> bool {
    let sign_string = format!("{}{}?body={}", secret, path, body);
    let digest = format!("{:x}", md5::compute(sign_string.as_bytes()));
    digest == expected_sign
}

// --- Shared ASSISTANT_SECRET loader ---

/// 预共享密钥，从环境变量 ASSISTANT_SECRET 懒加载
static ASSISTANT_SECRET: OnceLock<String> = OnceLock::new();

/// 懒加载 ASSISTANT_SECRET；返回 `&'static str`，未设置时为空串。
pub fn get_assistant_secret() -> &'static str {
    ASSISTANT_SECRET
        .get_or_init(|| std::env::var("ASSISTANT_SECRET").unwrap_or_default())
        .as_str()
}

// --- SSE Concurrency Control ---

pub struct SseSlotConfig {
    pub env_var: &'static str,
    pub default_cap: usize,
}

impl SseSlotConfig {
    pub const USER: Self = Self {
        env_var: "CHAT_SSE_MAX_CONCURRENT_PER_USER",
        default_cap: 1,
    };
}

/// Per-user SSE counter registry.
static SSE_COUNTERS: once_cell::sync::Lazy<Arc<Mutex<HashMap<i64, usize>>>> =
    once_cell::sync::Lazy::new(|| Arc::new(Mutex::new(HashMap::new())));

/// Guard that decrements the SSE counter on drop.
pub struct SseConcurrencyGuard {
    pub(crate) actor_id: i64,
}

impl Drop for SseConcurrencyGuard {
    fn drop(&mut self) {
        let actor_id = self.actor_id;
        let counter = SSE_COUNTERS.clone();
        tokio::spawn(async move {
            let mut map = counter.lock().await;
            if let Some(c) = map.get_mut(&actor_id) {
                *c = c.saturating_sub(1);
                if *c == 0 {
                    map.remove(&actor_id);
                }
            }
        });
    }
}

/// Try to acquire an SSE slot for the given user.
pub async fn try_acquire_slot(actor_id: i64, config: SseSlotConfig) -> bool {
    let mut map = SSE_COUNTERS.lock().await;
    let cap: usize = std::env::var(config.env_var)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(config.default_cap);
    let entry = map.entry(actor_id).or_insert(0);
    if *entry >= cap {
        return false;
    }
    *entry += 1;
    true
}

// --- Shared Query Structs ---

#[derive(Debug, Deserialize)]
pub struct ListSessionsQuery {
    pub offset: Option<i64>,
    pub limit: Option<i64>,
    pub search: Option<String>,
}

// --- SSE Response Builder ---

/// Build a standard SSE response with common headers.
pub fn sse_response<S>(stream: S) -> Response
where
    S: Stream<Item = Result<Event, Infallible>> + Send + 'static,
{
    let sse = Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text(": ping"),
    );

    let mut headers = HeaderMap::new();
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-cache, no-transform"),
    );
    headers.insert(header::CONNECTION, HeaderValue::from_static("keep-alive"));
    headers.insert("x-accel-buffering", HeaderValue::from_static("no"));

    (StatusCode::OK, headers, sse).into_response()
}
