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
    // ---------- 003 Admin Center ----------
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
        version: "V004__audit_logs",
        sql: include_str!("../../migrations/V004__audit_logs.sql"),
    },
    // ---------- 004 Agent Runtime ----------
    Migration {
        version: "V005__categories",
        sql: include_str!("../../migrations/V005__categories.sql"),
    },
    Migration {
        version: "V006__capabilities",
        sql: include_str!("../../migrations/V006__capabilities.sql"),
    },
    Migration {
        version: "V007__tags",
        sql: include_str!("../../migrations/V007__tags.sql"),
    },
    Migration {
        version: "V008__plugins",
        sql: include_str!("../../migrations/V008__plugins.sql"),
    },
    Migration {
        version: "V009__functions",
        sql: include_str!("../../migrations/V009__functions.sql"),
    },
    Migration {
        version: "V010__workflows",
        sql: include_str!("../../migrations/V010__workflows.sql"),
    },
    Migration {
        version: "V011__tools_skills",
        sql: include_str!("../../migrations/V011__tools_skills.sql"),
    },
    Migration {
        version: "V012__agents",
        sql: include_str!("../../migrations/V012__agents.sql"),
    },
    Migration {
        version: "V013__runtime_audit_logs",
        sql: include_str!("../../migrations/V013__runtime_audit_logs.sql"),
    },
    Migration {
        version: "V014__seed",
        sql: include_str!("../../migrations/V014__seed.sql"),
    },
    Migration {
        version: "V015__recommended_games",
        sql: include_str!("../../migrations/V015__recommended_games.sql"),
    },
    Migration {
        version: "V016__seed_capability_categories",
        sql: include_str!("../../migrations/V016__seed_capability_categories.sql"),
    },
    Migration {
        version: "V017__users_table",
        sql: include_str!("../../migrations/V017__users_table.sql"),
    },
    Migration {
        version: "V018__split_chat_tables",
        sql: include_str!("../../migrations/V018__split_chat_tables.sql"),
    },
    Migration {
        version: "V019__create_global_configs",
        sql: include_str!("../../migrations/V019__create_global_configs.sql"),
    },
    // ---------- 006 Game Alias Management ----------
    Migration {
        version: "V020__create_games_table",
        sql: include_str!("../../migrations/V020__create_games_table.sql"),
    },
    Migration {
        version: "V021__create_game_alias_entries_table",
        sql: include_str!("../../migrations/V021__create_game_alias_entries_table.sql"),
    },
    // ---------- 008 Agent Hook ----------
    Migration {
        version: "V022__create_agent_hooks_table",
        sql: include_str!("../../migrations/V022__create_agent_hooks_table.sql"),
    },
    Migration {
        version: "V023__create_hook_executions_table",
        sql: include_str!("../../migrations/V023__create_hook_executions_table.sql"),
    },
    Migration {
        version: "V024__add_extensions_to_chat_messages_user",
        sql: include_str!("../../migrations/V024__add_extensions_to_chat_messages_user.sql"),
    },
    // ---------- 010 Sensitive Word Filter ----------
    Migration {
        version: "V025__create_sensitive_words",
        sql: include_str!("../../migrations/V025__create_sensitive_words.sql"),
    },
    Migration {
        version: "V026__seed_sensitive_words",
        sql: include_str!("../../migrations/V026__seed_sensitive_words.sql"),
    },
    Migration {
        version: "V027__add_uid_nickname_to_users",
        sql: include_str!("../../migrations/V027__add_uid_nickname_to_users.sql"),
    },
    Migration {
        version: "V028__drop_phone_password_status_from_users",
        sql: include_str!("../../migrations/V028__drop_phone_password_status_from_users.sql"),
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
            if trimmed.starts_with("--") { "" } else { line }
        })
        .collect::<Vec<_>>()
        .join("\n");

    stripped
        .split(';')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}
