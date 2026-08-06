//! Runtime audit retention worker.
//!
//! Runs one cleanup pass immediately and then repeats at the configured
//! interval. The retention window defaults to 36500 days (100 years) and can be
//! changed with `AUDIT_RETENTION_DAYS`.

use sqlx::MySqlPool;
use std::time::Duration;

const RETENTION_DATABASE_URL_ENV: &str = "AUDIT_RETENTION_DATABASE_URL";
const DEFAULT_RETENTION_DAYS: u64 = 36_500;
const DEFAULT_INTERVAL_SECS: u64 = 24 * 60 * 60;

fn parse_positive(value: Option<&str>, default: u64, name: &str) -> anyhow::Result<u64> {
    match value {
        None => Ok(default),
        Some(value) => {
            let parsed = value
                .parse::<u64>()
                .map_err(|_| anyhow::anyhow!("{name} must be a positive integer"))?;
            if parsed == 0 {
                anyhow::bail!("{name} must be greater than zero");
            }
            Ok(parsed)
        }
    }
}

fn required_retention_database_url(value: Option<String>) -> anyhow::Result<String> {
    match value {
        Some(value) if !value.trim().is_empty() => Ok(value),
        _ => anyhow::bail!(
            "{RETENTION_DATABASE_URL_ENV} must be set to the dedicated retention database account"
        ),
    }
}

async fn cleanup_once(pool: &MySqlPool, retention_days: u64) -> anyhow::Result<u64> {
    let retention_days = i64::try_from(retention_days).unwrap_or(i64::MAX);
    let result = sqlx::query(
        "DELETE FROM runtime_audit_logs \
         WHERE occurred_at < DATE_SUB(UTC_TIMESTAMP(6), INTERVAL ? DAY)",
    )
    .bind(retention_days)
    .execute(pool)
    .await
    .map_err(|_| anyhow::anyhow!("runtime audit retention DELETE failed"))?;
    Ok(result.rows_affected())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv_override().ok();
    tracing_subscriber::fmt::init();

    let database_url =
        required_retention_database_url(std::env::var(RETENTION_DATABASE_URL_ENV).ok())?;
    let raw_retention = std::env::var("AUDIT_RETENTION_DAYS").ok();
    let raw_interval = std::env::var("AUDIT_RETENTION_INTERVAL_SEC").ok();
    let retention_days = parse_positive(
        raw_retention.as_deref(),
        DEFAULT_RETENTION_DAYS,
        "AUDIT_RETENTION_DAYS",
    )?;
    let interval_secs = parse_positive(
        raw_interval.as_deref(),
        DEFAULT_INTERVAL_SECS,
        "AUDIT_RETENTION_INTERVAL_SEC",
    )?;
    let pool = match MySqlPool::connect(&database_url).await {
        Ok(pool) => pool,
        Err(_) => {
            tracing::error!(
                error_kind = "runtime_audit_retention_failed",
                "runtime audit retention database connection failed; exiting"
            );
            anyhow::bail!("failed to connect to the audit retention database");
        }
    };

    tracing::info!(
        retention_days,
        interval_secs,
        "runtime audit retention worker starting"
    );
    loop {
        let started = std::time::Instant::now();
        match cleanup_once(&pool, retention_days).await {
            Ok(rows_deleted) => tracing::info!(
                rows_deleted,
                elapsed_ms = started.elapsed().as_millis(),
                "runtime audit retention pass completed"
            ),
            Err(_) => {
                tracing::error!(
                    error_kind = "runtime_audit_retention_failed",
                    "runtime audit retention cleanup failed; exiting"
                );
                anyhow::bail!("runtime audit retention cleanup failed; exiting");
            }
        }
        tokio::time::sleep(Duration::from_secs(interval_secs)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_INTERVAL_SECS, DEFAULT_RETENTION_DAYS, parse_positive,
        required_retention_database_url,
    };

    #[test]
    fn defaults_to_one_hundred_year_retention() {
        assert_eq!(
            parse_positive(None, DEFAULT_RETENTION_DAYS, "days").unwrap(),
            36_500
        );
        assert_eq!(
            parse_positive(None, DEFAULT_INTERVAL_SECS, "interval").unwrap(),
            86_400
        );
    }

    #[test]
    fn accepts_overrides_and_rejects_invalid_values() {
        assert_eq!(
            parse_positive(Some("730"), DEFAULT_RETENTION_DAYS, "days").unwrap(),
            730
        );
        assert!(parse_positive(Some("0"), DEFAULT_RETENTION_DAYS, "days").is_err());
        assert!(parse_positive(Some("abc"), DEFAULT_RETENTION_DAYS, "days").is_err());
    }

    #[test]
    fn retention_database_url_is_required_without_generic_fallback() {
        let error = required_retention_database_url(None).unwrap_err();
        assert!(error.to_string().contains("AUDIT_RETENTION_DATABASE_URL"));
        assert_ne!(error.to_string(), "DATABASE_URL must be set");
        assert!(required_retention_database_url(Some("   ".into())).is_err());
        assert_eq!(
            required_retention_database_url(Some("mysql://retention@127.0.0.1/hiveweb".into()))
                .unwrap(),
            "mysql://retention@127.0.0.1/hiveweb"
        );
    }

    #[tokio::test]
    #[ignore = "pending disposable MySQL CI users with table-level grants"]
    async fn mysql_accounts_enforce_append_only_writer_and_delete_only_retention() {
        let writer_url = std::env::var("AUDIT_WRITER_TEST_DATABASE_URL")
            .expect("AUDIT_WRITER_TEST_DATABASE_URL must name the INSERT+SELECT test account");
        let retention_url = std::env::var("AUDIT_RETENTION_TEST_DATABASE_URL")
            .expect("AUDIT_RETENTION_TEST_DATABASE_URL must name the DELETE-only test account");
        let writer = sqlx::MySqlPool::connect(&writer_url).await.unwrap();
        let retention = sqlx::MySqlPool::connect(&retention_url).await.unwrap();
        let request_id = format!("audit-permissions-{}", uuid::Uuid::new_v4().simple());

        sqlx::query(
            "INSERT INTO runtime_audit_logs \
             (request_id, event_type, outcome, occurred_at) \
             VALUES (?, 'runtime_operation', 'success', UTC_TIMESTAMP(6))",
        )
        .bind(&request_id)
        .execute(&writer)
        .await
        .expect("writer account must be able to INSERT");
        let (id,): (i64,) =
            sqlx::query_as("SELECT id FROM runtime_audit_logs WHERE request_id = ?")
                .bind(&request_id)
                .fetch_one(&writer)
                .await
                .expect("writer account must be able to SELECT");

        assert!(
            sqlx::query("UPDATE runtime_audit_logs SET outcome = 'error' WHERE id = ?")
                .bind(id)
                .execute(&writer)
                .await
                .is_err(),
            "writer account unexpectedly has UPDATE"
        );
        assert!(
            sqlx::query("DELETE FROM runtime_audit_logs WHERE id = ?")
                .bind(id)
                .execute(&writer)
                .await
                .is_err(),
            "writer account unexpectedly has DELETE"
        );
        assert!(
            sqlx::query(
                "INSERT INTO runtime_audit_logs (event_type, outcome) \
                 VALUES ('runtime_operation', 'success')"
            )
            .execute(&retention)
            .await
            .is_err(),
            "retention account unexpectedly has INSERT"
        );
        assert!(
            sqlx::query("SELECT id FROM runtime_audit_logs LIMIT 1")
                .execute(&retention)
                .await
                .is_err(),
            "retention account unexpectedly has SELECT"
        );

        let result = sqlx::query("DELETE FROM runtime_audit_logs WHERE id = ?")
            .bind(id)
            .execute(&retention)
            .await
            .expect("retention account must be able to DELETE");
        assert_eq!(result.rows_affected(), 1);
    }
}
