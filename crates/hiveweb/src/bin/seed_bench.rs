//! T101 / SC-005 disposable performance-dataset generator.
//!
//! This binary deletes existing non-Super administrator and login data. It is
//! independently feature-gated, accepts only databases whose parsed name ends
//! in `_test` or `_bench`, and requires the operator to repeat that exact
//! database name with `--confirm-destructive`.

use std::str::FromStr;

use clap::Parser;
use hiveweb::db::sql_safety::audit_sql;
use rand::RngCore;
use sqlx::mysql::MySqlConnectOptions;

const DEFAULT_ADMINS: usize = 100;
const DEFAULT_LOGINS_PER_ADMIN: usize = 100;
const MAX_ADMINS: usize = 1_000_000;
const MAX_LOGINS_PER_ADMIN: usize = 1_000_000;

#[derive(Debug, Parser)]
#[command(about = "Replace data in a disposable HiveWeb benchmark database")]
struct Cli {
    /// Number of generated administrator accounts.
    #[arg(value_name = "ADMINS", default_value_t = DEFAULT_ADMINS)]
    admins: usize,
    /// Number of generated login rows per administrator.
    #[arg(
        value_name = "LOGINS_PER_ADMIN",
        default_value_t = DEFAULT_LOGINS_PER_ADMIN
    )]
    logins_per_admin: usize,
    /// Repeat the exact parsed database name to authorize destructive cleanup.
    #[arg(long, value_name = "DATABASE_NAME")]
    confirm_destructive: String,
}

fn validated_disposable_database(
    database_url: &str,
    confirmation: &str,
) -> anyhow::Result<MySqlConnectOptions> {
    let options = MySqlConnectOptions::from_str(database_url)
        .map_err(|_| anyhow::anyhow!("DATABASE_URL must be a valid MySQL URL"))?;
    let database = options
        .get_database()
        .filter(|database| !database.is_empty())
        .ok_or_else(|| anyhow::anyhow!("DATABASE_URL must select a database"))?;
    anyhow::ensure!(
        database.ends_with("_test") || database.ends_with("_bench"),
        "seed-bench only accepts a database name ending in _test or _bench"
    );
    anyhow::ensure!(
        confirmation == database,
        "--confirm-destructive must exactly match the parsed database name"
    );
    Ok(options)
}

fn validate_dataset_size(admins: usize, logins_per_admin: usize) -> anyhow::Result<usize> {
    anyhow::ensure!(
        (1..=MAX_ADMINS).contains(&admins),
        "ADMINS must be between 1 and {MAX_ADMINS}"
    );
    anyhow::ensure!(
        logins_per_admin <= MAX_LOGINS_PER_ADMIN,
        "LOGINS_PER_ADMIN must not exceed {MAX_LOGINS_PER_ADMIN}"
    );
    admins
        .checked_mul(logins_per_admin)
        .ok_or_else(|| anyhow::anyhow!("requested benchmark dataset is too large"))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let cli = Cli::parse();
    let total_rows = validate_dataset_size(cli.admins, cli.logins_per_admin)?;

    let url =
        std::env::var("DATABASE_URL").map_err(|_| anyhow::anyhow!("DATABASE_URL must be set"))?;
    let options = validated_disposable_database(&url, &cli.confirm_destructive)?;
    let pool = sqlx::MySqlPool::connect_with(options)
        .await
        .map_err(|_| anyhow::anyhow!("failed to connect to the disposable benchmark database"))?;

    println!(
        "Seeding bench dataset: {} admins × {} login_records each ({} total rows)",
        cli.admins, cli.logins_per_admin, total_rows
    );

    // Clean non-super admins + their login records (FK SET NULL retains
    // historic rows; we delete them explicitly here for a clean baseline).
    sqlx::query("DELETE FROM login_records WHERE admin_id IS NULL OR admin_id IN (SELECT id FROM admins WHERE role <> 3)")
        .execute(&pool)
        .await
        .map_err(|_| anyhow::anyhow!("failed to delete disposable benchmark login records"))?;
    sqlx::query("DELETE FROM admins WHERE role <> 3")
        .execute(&pool)
        .await
        .map_err(|_| anyhow::anyhow!("failed to delete disposable benchmark administrators"))?;

    // Each run receives an unreported high-entropy password. There is no
    // committed or operator-visible credential for the generated accounts.
    let mut random_password = [0_u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut random_password);
    let password_hash = bcrypt::hash(random_password, bcrypt::DEFAULT_COST)
        .map_err(|_| anyhow::anyhow!("failed to hash benchmark account password"))?;
    random_password.fill(0);

    for i in 0..cli.admins {
        // 11-digit phone in 139xxxxxxxx range, padded with index.
        let phone = format!("139{:08}", 10_000_000 + i);
        let nickname = format!("bench-{i}");
        let role: i8 = if i % 10 == 0 { 2 } else { 1 };

        let result = sqlx::query(
            r#"
            INSERT INTO admins (phone, nickname, password_hash, role, status, created_at, updated_at, last_login_at)
            VALUES (?, ?, ?, ?, 1, NOW(), NOW(), NOW() - INTERVAL FLOOR(RAND()*48) HOUR)
            "#,
        )
        .bind(&phone)
        .bind(&nickname)
        .bind(&password_hash)
        .bind(role)
        .execute(&pool)
        .await
        .map_err(|_| anyhow::anyhow!("failed to insert a benchmark administrator"))?;
        let admin_id = result.last_insert_id() as i64;

        // Batch INSERT login records — split into chunks to stay under the
        // max_allowed_packet limit. 200 rows per statement is conservative.
        for chunk in (0..cli.logins_per_admin).collect::<Vec<_>>().chunks(200) {
            let mut sql = String::from(
                "INSERT INTO login_records \
                 (admin_id, admin_phone_snapshot, admin_nickname_snapshot, \
                  login_at, ip_address, success, failure_reason) VALUES ",
            );
            let mut first = true;
            for _ in chunk {
                if !first {
                    sql.push(',');
                }
                first = false;
                sql.push_str(
                    "(?, ?, ?, NOW() - INTERVAL FLOOR(RAND()*720) HOUR, '127.0.0.1', 1, NULL)",
                );
            }
            let audited_sql = audit_sql(sql);
            let mut query = sqlx::query(audited_sql);
            for _ in chunk {
                query = query.bind(admin_id).bind(&phone).bind(&nickname);
            }
            query
                .execute(&pool)
                .await
                .map_err(|_| anyhow::anyhow!("failed to insert benchmark login records"))?;
        }

        if (i + 1) % 25 == 0 {
            println!("  inserted {}/{} admins", i + 1, cli.admins);
        }
    }

    println!("Seed complete.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::{Cli, validate_dataset_size, validated_disposable_database};

    #[test]
    fn destructive_confirmation_is_required() {
        assert!(Cli::try_parse_from(["seed-bench"]).is_err());
        assert!(
            Cli::try_parse_from([
                "seed-bench",
                "100",
                "100",
                "--confirm-destructive",
                "hiveweb_bench",
            ])
            .is_ok()
        );
    }

    #[test]
    fn parsed_database_name_must_be_disposable_and_match_confirmation() {
        assert!(
            validated_disposable_database(
                "mysql://user:secret@127.0.0.1/hiveweb_test",
                "hiveweb_test",
            )
            .is_ok()
        );
        assert!(
            validated_disposable_database(
                "mysql://user:secret@127.0.0.1/hiveweb_bench?ssl-mode=disabled",
                "hiveweb_bench",
            )
            .is_ok()
        );
        assert!(
            validated_disposable_database("mysql://user:secret@127.0.0.1/hiveweb", "hiveweb",)
                .is_err()
        );
        assert!(
            validated_disposable_database(
                "mysql://user:secret@127.0.0.1/hiveweb_test",
                "another_test",
            )
            .is_err()
        );
        assert!(
            validated_disposable_database(
                "mysql://user:secret@127.0.0.1/hiveweb?database=hiveweb_test",
                "hiveweb_test",
            )
            .is_err()
        );
    }

    #[test]
    fn dataset_size_is_bounded_and_overflow_checked() {
        assert_eq!(validate_dataset_size(100, 100).unwrap(), 10_000);
        assert!(validate_dataset_size(0, 100).is_err());
        assert!(validate_dataset_size(usize::MAX, usize::MAX).is_err());
    }
}
