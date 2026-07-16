//! External database queries for VIP membership and cloud user validation.
//!
//! These functions query the external (non-hive) database for user membership
//! status and user existence checks.

use sqlx::MySqlPool;

#[derive(Debug, Clone, sqlx::FromRow)]
struct CcUserMembership {
    id: i64,
    membership_level: Option<String>,
    effective_end_time: Option<chrono::NaiveDateTime>,
}

/// Check if a user has an active VIP membership in the external database.
pub async fn check_vip_membership(pool: &MySqlPool, user_id: i64) -> Result<bool, sqlx::Error> {
    sqlx::query_as::<_, CcUserMembership>(
        "SELECT id, membership_level, effective_end_time FROM cc_user_membership WHERE id = ? LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .map(|row| {
        row.map_or(false, |m| {
            m.effective_end_time
                .map_or(true, |end| end >= chrono::Utc::now().naive_utc())
        })
    })
}

/// Check if a user exists in the external cloud_user table.
pub async fn user_exists_in_cloud(pool: &MySqlPool, user_id: i64) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar::<_, i64>("SELECT COUNT(1) FROM cloud_user WHERE ID = ?")
        .bind(user_id)
        .fetch_one(pool)
        .await
        .map(|count| count > 0)
}

/// Get cloud_user uid and nickname for a given user ID.
pub async fn get_cloud_user_info(
    pool: &MySqlPool,
    user_id: i64,
) -> Result<Option<(String, String)>, sqlx::Error> {
    sqlx::query_as::<_, (String, String)>("SELECT uid, nickname FROM cloud_user WHERE ID = ?")
        .bind(user_id)
        .fetch_optional(pool)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "requires external DB"]
    async fn check_vip_membership_active() {
        // Requires seeded cc_user_membership with future effective_end_time
    }

    #[tokio::test]
    #[ignore = "requires external DB"]
    async fn user_exists_in_cloud_returns_true() {
        // Requires seeded cloud_user
    }
}
