use anyhow::Result;
use redis::AsyncCommands;
use sqlx::MySqlPool;

use crate::cache::redis::RedisClient;
use crate::models::LoginRecord;

const LOGIN_FAILED_KEY_PREFIX: &str = "login_failed:";
const MAX_LOGIN_ATTEMPTS: i64 = 5;
const LOCK_DURATION_SECONDS: usize = 900;

pub async fn create_login_record(
    pool: &MySqlPool,
    admin_id: i64,
    admin_nickname: &str,
    ip_address: &str,
    success: bool,
    failure_reason: Option<&str>,
) -> Result<LoginRecord> {
    let record = sqlx::query_as::<_, LoginRecord>(
        r#"
        INSERT INTO login_records (admin_id, admin_nickname, login_at, ip_address, success, failure_reason)
        VALUES (?, ?, NOW(), ?, ?, ?)
        "#,
    )
    .bind(admin_id)
    .bind(admin_nickname)
    .bind(ip_address)
    .bind(success)
    .bind(failure_reason)
    .fetch_one(pool)
    .await?;

    Ok(record)
}

pub async fn get_recent_logins(pool: &MySqlPool, limit: u32) -> Result<Vec<LoginRecord>> {
    let records = sqlx::query_as::<_, LoginRecord>(
        r#"
        SELECT lr.*, a.nickname as admin_nickname
        FROM login_records lr
        JOIN admins a ON a.id = lr.admin_id
        ORDER BY lr.login_at DESC
        LIMIT ?
        "#,
    )
    .bind(limit as i64)
    .fetch_all(pool)
    .await?;

    Ok(records)
}

pub async fn get_failed_attempts(redis: &RedisClient, phone: &str) -> Result<i64> {
    let key = format!("{}{}", LOGIN_FAILED_KEY_PREFIX, phone);
    let mut conn = redis.get_multiplexed_async_connection().await?;
    let count: Option<i64> = conn.get(&key).await?;
    Ok(count.unwrap_or(0))
}

pub async fn is_account_locked(redis: &RedisClient, phone: &str) -> Result<bool> {
    // Redis TTL semantics:
    //   -2 → key does not exist
    //   -1 → key exists but has no expiry (counter is still being accumulated below the lockout threshold)
    //   >0 → seconds remaining until expiry (account is locked)
    // Must use a signed type so the negative sentinels survive the wire decode.
    let key = format!("{}{}", LOGIN_FAILED_KEY_PREFIX, phone);
    let mut conn = redis.get_multiplexed_async_connection().await?;
    let ttl: i64 = conn.ttl(&key).await?;
    Ok(ttl > 0)
}

pub async fn increment_failed_attempts(redis: &RedisClient, phone: &str) -> Result<i64> {
    let key = format!("{}{}", LOGIN_FAILED_KEY_PREFIX, phone);
    let mut conn = redis.get_multiplexed_async_connection().await?;

    let count: i64 = conn.incr(&key, 1).await?;

    if count == MAX_LOGIN_ATTEMPTS {
        let _: () = conn.expire(&key, LOCK_DURATION_SECONDS as i64).await?;
    }

    Ok(count)
}

pub async fn reset_failed_attempts(redis: &RedisClient, phone: &str) -> Result<()> {
    let key = format!("{}{}", LOGIN_FAILED_KEY_PREFIX, phone);
    let mut conn = redis.get_multiplexed_async_connection().await?;
    let _: () = conn.del(&key).await?;
    Ok(())
}
