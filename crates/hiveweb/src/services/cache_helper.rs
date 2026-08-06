//! Redis cache-aside helpers for external database queries.
//!
//! Provides a generic `cached_or_fetch` function that implements the
//! read-aside pattern: Redis GET → on miss, execute DB query → SETEX → return.
//! All cache operations are best-effort — failures fall back to the DB query.

use redis::AsyncCommands;
use serde::{Serialize, de::DeserializeOwned};

use crate::cache::redis::RedisClient;

/// Try to read a cached value from Redis, or fetch from DB and cache it.
///
/// - `redis`: Redis client
/// - `key`: cache key
/// - `ttl_secs`: TTL in seconds
/// - `fetch`: async closure that queries the database
///
/// Returns the value from cache or DB. Cache failures are logged and
/// silently fall back to the DB result.
pub async fn cached_or_fetch<T, F, Fut>(
    redis: &RedisClient,
    key: &str,
    ttl_secs: u64,
    fetch: F,
) -> Result<T, String>
where
    T: Serialize + DeserializeOwned,
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<T, String>>,
{
    // 1. Try Redis
    match cached_get::<T>(redis, key).await {
        Ok(Some(value)) => return Ok(value),
        Ok(None) => {} // cache miss
        Err(_) => {
            tracing::debug!(
                error_kind = "cache_read_failed",
                "cache read failed, falling back to DB"
            );
        }
    }

    // 2. Fetch from DB
    let value = fetch().await?;

    // 3. Write to cache (best-effort)
    if cached_set(redis, key, &value, ttl_secs).await.is_err() {
        tracing::debug!(error_kind = "cache_write_failed", "cache write failed");
    }

    Ok(value)
}

/// Read a value from Redis cache. Returns `None` on cache miss.
pub async fn cached_get<T: DeserializeOwned>(
    redis: &RedisClient,
    key: &str,
) -> Result<Option<T>, String> {
    let mut conn = redis
        .get_multiplexed_async_connection()
        .await
        .map_err(|e| format!("redis connect: {e}"))?;
    let raw: Option<String> = conn
        .get(key)
        .await
        .map_err(|e| format!("redis GET {key}: {e}"))?;
    match raw {
        Some(json) => {
            let value: T =
                serde_json::from_str(&json).map_err(|e| format!("deserialize {key}: {e}"))?;
            Ok(Some(value))
        }
        None => Ok(None),
    }
}

/// Write a value to Redis cache with TTL.
pub async fn cached_set<T: Serialize>(
    redis: &RedisClient,
    key: &str,
    value: &T,
    ttl_secs: u64,
) -> Result<(), String> {
    let mut conn = redis
        .get_multiplexed_async_connection()
        .await
        .map_err(|e| format!("redis connect: {e}"))?;
    let json = serde_json::to_string(value).map_err(|e| format!("serialize {key}: {e}"))?;
    let _: () = conn
        .set_ex(key, json, ttl_secs)
        .await
        .map_err(|e| format!("redis SETEX {key}: {e}"))?;
    Ok(())
}

/// Delete a key from Redis (for cache invalidation).
#[allow(dead_code)]
pub async fn cached_del(redis: &RedisClient, key: &str) -> Result<(), String> {
    let mut conn = redis
        .get_multiplexed_async_connection()
        .await
        .map_err(|e| format!("redis connect: {e}"))?;
    let _: () = conn
        .del(key)
        .await
        .map_err(|e| format!("redis DEL {key}: {e}"))?;
    Ok(())
}

// ── Cache key constants ──

/// VIP membership status → bool
pub const KEY_VIP_STATUS: &str = "vip:status";
/// cloud_user uid + nickname → (String, String)
pub const KEY_CLOUD_USER_INFO: &str = "cloud_user:info";
/// Membership balance → MembershipBalanceRow
pub const KEY_BALANCE: &str = "balance";
/// Membership subscriptions → Vec<MembershipSubscriptionRow>
pub const KEY_SUBSCRIPTIONS: &str = "subscriptions";
/// Duration cards → Vec<DurationCardRow>
pub const KEY_DURATION_CARDS: &str = "duration_cards";
/// External game list by (channel, client_type) → Vec<(u32, String, String)>
pub const KEY_GAME_LIST: &str = "game_list";
/// External game by cc_game ID → (i64, String, String)
pub const KEY_GAME_INFO: &str = "game_info";
/// External games simple list → Vec<ExternalGameOption>
pub const KEY_EXTERNAL_GAMES: &str = "external_games:list";
/// Global config cache prefix → i64 (config value)
pub const KEY_CONFIG_PREFIX: &str = "config:";
/// AI assistant chat limit config → AssistantChatLimitConfig (from cc_config)
pub const KEY_AI_ASSISTANT_CHAT_LIMIT_CONFIG: &str = "ai_assistant_limit_cfg";
/// Agent content cache prefix → AgentContent
pub const KEY_AGENT_CONTENT_PREFIX: &str = "agent:content";

// ── TTL constants (seconds) ──

pub const TTL_VIP_STATUS: u64 = 300; // 5 min
pub const TTL_CLOUD_USER_INFO: u64 = 900; // 15 min
pub const TTL_BALANCE: u64 = 60; // 1 min
pub const TTL_SUBSCRIPTIONS: u64 = 300; // 5 min
pub const TTL_DURATION_CARDS: u64 = 300; // 5 min
pub const TTL_GAME_LIST: u64 = 600; // 10 min
pub const TTL_GAME_INFO_FOUND: u64 = 86400; // 1 day — 查到游戏信息则长缓存
pub const TTL_GAME_INFO_NOT_FOUND: u64 = 300; // 5 min — 未查到则短缓存（可能新上架）
pub const TTL_EXTERNAL_GAMES: u64 = 1800; // 30 min
pub const TTL_CONFIG: u64 = 3600; // 1 hour
pub const TTL_AI_ASSISTANT_CHAT_LIMIT_CONFIG: u64 = 60; // 5 min
pub const TTL_AGENT_CONTENT_DEFAULT: u64 = 300; // 5 min (overridable via AGENT_CACHE_TTL_SECS env)

/// Agent content cache TTL — reads AGENT_CACHE_TTL_SECS env var, falls back to 300s.
pub fn agent_content_ttl_secs() -> u64 {
    std::env::var("AGENT_CACHE_TTL_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(TTL_AGENT_CONTENT_DEFAULT)
}
