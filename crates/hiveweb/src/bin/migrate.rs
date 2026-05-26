//! Forward-only schema migrations for the admin center.
//!
//! Migration files live in `crates/hiveweb/migrations/` and follow the
//! Flyway-style naming convention `V###__description.sql` documented in
//! `specs/003-admin-center/data-model.md §Migrations`.
//!
//! Each migration is embedded at compile time via `include_str!` and applied
//! exactly once; applied versions are recorded in `schema_migrations`.

use sqlx::MySqlPool;
use std::env;

struct Migration {
    version: &'static str,
    sql: &'static str,
}

const MIGRATIONS: &[Migration] = &[
    Migration {
        version: "V001__create_admins_table",
        sql: include_str!("../../migrations/V001__create_admins_table.sql"),
    },
    Migration {
        version: "V002__create_login_records_table",
        sql: include_str!("../../migrations/V002__create_login_records_table.sql"),
    },
    Migration {
        version: "V003__seed_super_admin",
        sql: include_str!("../../migrations/V003__seed_super_admin.sql"),
    },
    Migration {
        version: "V004__login_records_set_null_and_snapshots",
        sql: include_str!("../../migrations/V004__login_records_set_null_and_snapshots.sql"),
    },
    Migration {
        version: "V005__login_records_idx_login_at_desc",
        sql: include_str!("../../migrations/V005__login_records_idx_login_at_desc.sql"),
    },
    Migration {
        version: "V006__audit_logs",
        sql: include_str!("../../migrations/V006__audit_logs.sql"),
    },
    Migration {
        version: "V007__admins_idx_created_at",
        sql: include_str!("../../migrations/V007__admins_idx_created_at.sql"),
    },
];

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv_override().ok();

    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL must be set");

    println!("Connecting to database...");
    let pool = MySqlPool::connect(&database_url).await?;

    println!("Ensuring schema_migrations tracking table...");
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS schema_migrations (
            version VARCHAR(128) PRIMARY KEY,
            applied_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
        "#,
    )
    .execute(&pool)
    .await?;

    for migration in MIGRATIONS {
        let already_applied: Option<(String,)> =
            sqlx::query_as("SELECT version FROM schema_migrations WHERE version = ?")
                .bind(migration.version)
                .fetch_optional(&pool)
                .await?;
        if already_applied.is_some() {
            println!("  ✓ {} (skipped — already applied)", migration.version);
            continue;
        }

        println!("  → Applying {}", migration.version);
        for stmt in split_sql_statements(migration.sql) {
            sqlx::query(&stmt).execute(&pool).await?;
        }
        sqlx::query("INSERT INTO schema_migrations (version) VALUES (?)")
            .bind(migration.version)
            .execute(&pool)
            .await?;
        println!("    applied");
    }

    println!("All migrations applied.");
    Ok(())
}

/// Strip line comments (`-- ...`) and split a multi-statement SQL file on
/// statement-terminating semicolons. Empty / whitespace-only statements are
/// dropped. Works for our migration files; do not use for arbitrary SQL that
/// contains semicolons inside string literals.
fn split_sql_statements(sql: &str) -> Vec<String> {
    let stripped: String = sql
        .lines()
        .map(|line| {
            let trimmed = line.trim_start();
            if trimmed.starts_with("--") {
                ""
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n");

    stripped
        .split(';')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}
