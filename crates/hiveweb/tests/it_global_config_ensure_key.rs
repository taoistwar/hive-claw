//! Integration tests for `global_config::ensure_key` (INSERT IGNORE semantics).

mod common;

use hiveweb::services::global_config::{ensure_key, fetch_by_key};

#[tokio::test]
async fn ensure_key_inserts_missing_row() -> anyhow::Result<()> {
    let pool = common::test_pool().await?;
    let key = format!("test_ensure_key_{}", uuid_suffix());

    ensure_key(
        &pool,
        "Test Ensure Key",
        &key,
        "string",
        &serde_json::json!({ "value": "hello" }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("ensure_key: {e}"))?;

    let cfg = fetch_by_key(&pool, &key)
        .await
        .map_err(|e| anyhow::anyhow!("fetch_by_key: {e}"))?;
    assert_eq!(cfg.key, key);
    assert_eq!(cfg.name, "Test Ensure Key");
    assert_eq!(cfg.config_type, "string");
    assert_eq!(cfg.data, serde_json::json!({ "value": "hello" }));

    cleanup(&pool, &key).await;
    Ok(())
}

#[tokio::test]
async fn ensure_key_is_idempotent() -> anyhow::Result<()> {
    let pool = common::test_pool().await?;
    let key = format!("test_ensure_key_{}", uuid_suffix());

    ensure_key(
        &pool,
        "Test Idempotent",
        &key,
        "number",
        &serde_json::json!({ "value": 1 }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("ensure_key: {e}"))?;
    // Second call must be a no-op (INSERT IGNORE), not an error or a duplicate.
    ensure_key(
        &pool,
        "Test Idempotent",
        &key,
        "number",
        &serde_json::json!({ "value": 999 }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("ensure_key dup: {e}"))?;

    let cfg = fetch_by_key(&pool, &key)
        .await
        .map_err(|e| anyhow::anyhow!("fetch_by_key: {e}"))?;
    // The original value must be preserved, not overwritten by the duplicate call.
    assert_eq!(
        cfg.data,
        serde_json::json!({ "value": 1 }),
        "ensure_key must not overwrite an existing row"
    );

    cleanup(&pool, &key).await;
    Ok(())
}

async fn cleanup(pool: &sqlx::MySqlPool, key: &str) {
    let _ = sqlx::query("DELETE FROM global_configs WHERE `key` = ?")
        .bind(key)
        .execute(pool)
        .await;
}

fn uuid_suffix() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{n}")
}
