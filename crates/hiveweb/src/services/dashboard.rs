use anyhow::Result;
use sqlx::MySqlPool;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct DashboardStats {
    pub total_admins: i64,
    pub active_admins: i64,
    pub disabled_admins: i64,
    pub today_logins: i64,
}

pub async fn get_stats(pool: &MySqlPool) -> Result<DashboardStats> {
    let row = sqlx::query_as::<_, (i64, i64, i64, i64)>(
        r#"
        SELECT
            (SELECT COUNT(*) FROM admins) as total_admins,
            (SELECT COUNT(*) FROM admins WHERE status = 1) as active_admins,
            (SELECT COUNT(*) FROM admins WHERE status = 0) as disabled_admins,
            (SELECT COUNT(*) FROM login_records WHERE DATE(login_at) = CURDATE()) as today_logins
        "#,
    )
    .fetch_one(pool)
    .await?;

    Ok(DashboardStats {
        total_admins: row.0,
        active_admins: row.1,
        disabled_admins: row.2,
        today_logins: row.3,
    })
}
