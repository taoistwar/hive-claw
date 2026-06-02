//! Audit log retention cron (T145 / FR-022)
//!
//! 启动后立即扫描一次，然后按 cron 间隔重复（默认每 24 小时）。
//! 保留窗口由 `AUDIT_RETENTION_DAYS`（默认 90）控制，与 FR-022 对齐。
//!
//! 运行：`cargo run --bin audit_retention` 或作为独立 systemd/k8s cronjob。

use sqlx::MySqlPool;
use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv_override().ok();
    tracing_subscriber::fmt::init();

    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let pool = MySqlPool::connect(&url).await?;

    let retention_days: i64 = std::env::var("AUDIT_RETENTION_DAYS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(90);

    let interval_secs: u64 = std::env::var("AUDIT_RETENTION_INTERVAL_SEC")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(24 * 3600);

    tracing::info!(
        retention_days,
        interval_secs,
        "audit_retention cron starting"
    );

    loop {
        let t0 = std::time::Instant::now();
        let result = sqlx::query(
            "DELETE FROM runtime_audit_logs WHERE occurred_at < (NOW() - INTERVAL ? DAY)",
        )
        .bind(retention_days)
        .execute(&pool)
        .await;
        match result {
            Ok(r) => tracing::info!(
                rows_deleted = r.rows_affected(),
                elapsed_ms = t0.elapsed().as_millis(),
                "audit_retention pass completed"
            ),
            Err(e) => tracing::error!(error = %e, "audit_retention pass failed"),
        }
        tokio::time::sleep(Duration::from_secs(interval_secs)).await;
    }
}
