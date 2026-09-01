//! User sync service

use sqlx::MySqlPool;

/// Ensure a user row exists.
pub async fn ensure_user_exists(
    pool: &MySqlPool,
    user_id: i64,
    uid: Option<&str>,
    nickname: Option<&str>,
) -> Result<(), anyhow::Error> {
    let existing: Option<(i64, Option<String>, Option<String>)> =
        sqlx::query_as("SELECT id, uid, nickname FROM users WHERE id = ?")
            .bind(user_id)
            .fetch_optional(pool)
            .await?;
    match existing {
        Some(_) => {
            let new_uid = uid.unwrap_or("");
            let new_nick = nickname.unwrap_or("");
            if !new_uid.is_empty() || !new_nick.is_empty() {
                sqlx::query("UPDATE users SET uid = ?, nickname = ? WHERE id = ?")
                    .bind(new_uid)
                    .bind(new_nick)
                    .bind(user_id)
                    .execute(pool)
                    .await?;
            }
        }
        None => {
            sqlx::query("INSERT INTO users (id, uid, nickname) VALUES (?, ?, ?)")
                .bind(user_id)
                .bind(uid.unwrap_or(""))
                .bind(nickname.unwrap_or(""))
                .execute(pool)
                .await?;
        }
    }
    Ok(())
}
