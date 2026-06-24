//! User sync service

use sqlx::MySqlPool;

/// Ensure a user row exists.
pub async fn ensure_user_exists(
    pool: &MySqlPool,
    user_id: i64,
    uid: Option<&str>,
    nickname: Option<&str>,
) -> Result<(), anyhow::Error> {
    sqlx::query("INSERT IGNORE INTO users (id, uid, nickname) VALUES (?, ?, ?)")
        .bind(user_id)
        .bind(uid.unwrap_or(""))
        .bind(nickname.unwrap_or(""))
        .execute(pool)
        .await?;
    Ok(())
}
