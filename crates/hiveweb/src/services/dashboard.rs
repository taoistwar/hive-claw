use anyhow::Result;
use chrono::NaiveDateTime;
use serde::Serialize;
use sqlx::{FromRow, MySqlPool};

/// Stats payload per spec FR-014 (revised 2026-05-26):
/// `total_admins` / `online_admins` / `today_logins`.
#[derive(Debug, Serialize)]
pub struct DashboardStats {
    /// All rows in `admins`.
    pub total_admins: i64,
    /// `status = 1` AND `last_login_at` within the last 24h.
    pub online_admins: i64,
    /// Successful login_records on the server's current local date.
    pub today_logins: i64,
}

pub async fn get_stats(pool: &MySqlPool) -> Result<DashboardStats> {
    let row = sqlx::query_as::<_, (i64, i64, i64)>(
        r#"
        SELECT
            (SELECT COUNT(*) FROM admins) AS total_admins,
            (SELECT COUNT(*) FROM admins
              WHERE status = 1
                AND last_login_at IS NOT NULL
                AND last_login_at >= DATE_SUB(NOW(), INTERVAL 24 HOUR)) AS online_admins,
            (SELECT COUNT(*) FROM login_records
              WHERE success = 1
                AND login_at >= CURDATE()
                AND login_at <  CURDATE() + INTERVAL 1 DAY) AS today_logins
        "#,
    )
    .fetch_one(pool)
    .await?;

    Ok(DashboardStats {
        total_admins: row.0,
        online_admins: row.1,
        today_logins: row.2,
    })
}

/// Row shape returned by `/api/dashboard/recent-logins` per spec FR-015.
/// Uses the audit-snapshot columns added by migration V004 so deleted
/// admins still display their phone / nickname at the time of the login.
#[derive(Debug, Serialize, FromRow)]
pub struct RecentLoginRow {
    pub id: i64,
    pub admin_id: Option<i64>,
    pub admin_nickname: String,
    pub login_at: NaiveDateTime,
    pub ip_address: String,
    pub success: bool,
}

pub async fn get_recent_logins(pool: &MySqlPool, limit: u32) -> Result<Vec<RecentLoginRow>> {
    let limit = limit.clamp(1, 100) as i64;
    let rows = sqlx::query_as::<_, RecentLoginRow>(
        r#"
        SELECT
            id,
            admin_id,
            admin_nickname_snapshot AS admin_nickname,
            login_at,
            ip_address,
            success
        FROM login_records
        ORDER BY login_at DESC
        LIMIT ?
        "#,
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows)
}
