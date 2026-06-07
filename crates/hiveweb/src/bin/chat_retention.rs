//! Chat retention cron (T132 / FR-027 v7 / CHAT_RETENTION_DAYS)
//!
//! 按 `CHAT_RETENTION_DAYS`（默认 30）清理超期 chat_sessions_user。
//! chat_messages_user 因 FK ON DELETE CASCADE 一并清除。

use sqlx::MySqlPool;
use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv_override().ok();
    tracing_subscriber::fmt::init();

    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let pool = MySqlPool::connect(&url).await?;

    let retention_days: i64 = std::env::var("CHAT_RETENTION_DAYS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30);

    let interval_secs: u64 = std::env::var("CHAT_RETENTION_INTERVAL_SEC")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(24 * 3600);

    tracing::info!(
        retention_days,
        interval_secs,
        "chat_retention cron starting"
    );

    loop {
        let t0 = std::time::Instant::now();
        let result = sqlx::query(
            "DELETE FROM chat_sessions_user WHERE updated_at < (NOW() - INTERVAL ? DAY)",
        )
        .bind(retention_days)
        .execute(&pool)
        .await;
        match result {
            Ok(r) => tracing::info!(
                rows_deleted = r.rows_affected(),
                elapsed_ms = t0.elapsed().as_millis(),
                "chat_retention pass completed"
            ),
            Err(e) => tracing::error!(error = %e, "chat_retention pass failed"),
        }
        tokio::time::sleep(Duration::from_secs(interval_secs)).await;
    }
}
