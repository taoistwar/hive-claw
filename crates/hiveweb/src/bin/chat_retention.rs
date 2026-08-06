//! Chat retention cron (T132 / FR-027 v7 / CHAT_RETENTION_DAYS)
//!
//! 按 `CHAT_RETENTION_DAYS`（默认 30）清理超期 chat_sessions_user。
//! chat_messages_user 因 FK ON DELETE CASCADE 一并清除。

use sqlx::MySqlPool;
use std::time::Duration;

const DEFAULT_RETENTION_DAYS: u64 = 30;
const DEFAULT_INTERVAL_SECS: u64 = 24 * 60 * 60;

fn parse_positive(value: Option<&str>, default: u64, name: &str) -> anyhow::Result<u64> {
    match value {
        None => Ok(default),
        Some(value) => {
            let parsed = value
                .parse::<u64>()
                .map_err(|_| anyhow::anyhow!("{name} must be a positive integer"))?;
            anyhow::ensure!(parsed > 0, "{name} must be greater than zero");
            Ok(parsed)
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv_override().ok();
    tracing_subscriber::fmt::init();

    let raw_retention = std::env::var("CHAT_RETENTION_DAYS").ok();
    let raw_interval = std::env::var("CHAT_RETENTION_INTERVAL_SEC").ok();
    let retention_days = parse_positive(
        raw_retention.as_deref(),
        DEFAULT_RETENTION_DAYS,
        "CHAT_RETENTION_DAYS",
    )?;
    let interval_secs = parse_positive(
        raw_interval.as_deref(),
        DEFAULT_INTERVAL_SECS,
        "CHAT_RETENTION_INTERVAL_SEC",
    )?;
    let retention_days = i64::try_from(retention_days)
        .map_err(|_| anyhow::anyhow!("CHAT_RETENTION_DAYS is too large"))?;

    let url =
        std::env::var("DATABASE_URL").map_err(|_| anyhow::anyhow!("DATABASE_URL must be set"))?;
    let pool = match MySqlPool::connect(&url).await {
        Ok(pool) => pool,
        Err(_) => {
            tracing::error!(
                error_kind = "chat_retention_failed",
                "chat retention database connection failed; exiting"
            );
            anyhow::bail!("failed to connect to the chat retention database");
        }
    };

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
            Err(_) => {
                tracing::error!(
                    error_kind = "chat_retention_failed",
                    "chat retention cleanup failed; exiting"
                );
                anyhow::bail!("chat retention cleanup failed; exiting");
            }
        }
        tokio::time::sleep(Duration::from_secs(interval_secs)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::parse_positive;

    #[test]
    fn retention_days_and_interval_must_be_strictly_positive() {
        assert_eq!(parse_positive(None, 30, "DAYS").unwrap(), 30);
        assert_eq!(parse_positive(Some("7"), 30, "DAYS").unwrap(), 7);
        for invalid in ["0", "-1", "garbage", " 1", "1.0"] {
            assert!(
                parse_positive(Some(invalid), 30, "DAYS").is_err(),
                "{invalid:?} must fail closed"
            );
        }
    }
}
