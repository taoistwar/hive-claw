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
    // ---------- 004 Agent Runtime ----------
    Migration {
        version: "V008__capabilities",
        sql: include_str!("../../migrations/V008__capabilities.sql"),
    },
    Migration {
        version: "V009__categories",
        sql: include_str!("../../migrations/V009__categories.sql"),
    },
    Migration {
        version: "V010__tags",
        sql: include_str!("../../migrations/V010__tags.sql"),
    },
    Migration {
        version: "V011__plugins",
        sql: include_str!("../../migrations/V011__plugins.sql"),
    },
    Migration {
        version: "V012__functions",
        sql: include_str!("../../migrations/V012__functions.sql"),
    },
    Migration {
        version: "V013__workflows",
        sql: include_str!("../../migrations/V013__workflows.sql"),
    },
    Migration {
        version: "V014__tools_skills",
        sql: include_str!("../../migrations/V014__tools_skills.sql"),
    },
    Migration {
        version: "V015__agents",
        sql: include_str!("../../migrations/V015__agents.sql"),
    },
    Migration {
        version: "V016__chat",
        sql: include_str!("../../migrations/V016__chat.sql"),
    },
    Migration {
        version: "V017__runtime_audit_logs",
        sql: include_str!("../../migrations/V017__runtime_audit_logs.sql"),
    },
    Migration {
        version: "V018__seed",
        sql: include_str!("../../migrations/V018__seed.sql"),
    },
    Migration {
        version: "V019__tools_source",
        sql: include_str!("../../migrations/V019__tools_source.sql"),
    },
    Migration {
        version: "V020__tools_is_always",
        sql: include_str!("../../migrations/V020__tools_is_always.sql"),
    },
    Migration {
        version: "V021__skills_is_always",
        sql: include_str!("../../migrations/V021__skills_is_always.sql"),
    },
    Migration {
        version: "V022__recommended_games",
        sql: include_str!("../../migrations/V022__recommended_games.sql"),
    },
    Migration {
        version: "V023__recommended_games_add_card_content",
        sql: include_str!("../../migrations/V023__recommended_games_add_card_content.sql"),
    },
    Migration {
        version: "V024__recommended_games_add_sort_value",
        sql: include_str!("../../migrations/V024__recommended_games_add_sort_value.sql"),
    },
    Migration {
        version: "V025__recommended_games_add_fields",
        sql: include_str!("../../migrations/V025__recommended_games_add_fields.sql"),
    },
    Migration {
        version: "V026__tools_category",
        sql: include_str!("../../migrations/V026__tools_category.sql"),
    },
    Migration {
        version: "V027__skills_category",
        sql: include_str!("../../migrations/V027__skills_category.sql"),
    },
    Migration {
        version: "V028__rename_audit_logs_to_admin_audit_logs",
        sql: include_str!("../../migrations/V028__rename_audit_logs_to_admin_audit_logs.sql"),
    },
    Migration {
        version: "V029__function_tool_capabilities",
        sql: include_str!("../../migrations/V029__function_tool_capabilities.sql"),
    },
    Migration {
        version: "V030__workflow_required_capabilities",
        sql: include_str!("../../migrations/V030__workflow_required_capabilities.sql"),
    },
    Migration {
        version: "V031__skill_required_capabilities",
        sql: include_str!("../../migrations/V031__skill_required_capabilities.sql"),
    },
    Migration {
        version: "V032__workflow_category",
        sql: include_str!("../../migrations/V032__workflow_category.sql"),
    },
    Migration {
        version: "V033__workflow_input_schema",
        sql: include_str!("../../migrations/V033__workflow_input_schema.sql"),
    },
    Migration {
        version: "V034__workflow_node_type",
        sql: include_str!("../../migrations/V034__workflow_node_type.sql"),
    },
    Migration {
        version: "V035__workflow_output_schema",
        sql: include_str!("../../migrations/V035__workflow_output_schema.sql"),
    },
    Migration {
        version: "V036__workflow_answer_node",
        sql: include_str!("../../migrations/V036__workflow_answer_node.sql"),
    },
    Migration {
        version: "V037__capabilities_category",
        sql: include_str!("../../migrations/V037__capabilities_category.sql"),
    },
    Migration {
        version: "V038__seed_capability_categories",
        sql: include_str!("../../migrations/V038__seed_capability_categories.sql"),
    },
    Migration {
        version: "V039__users_table",
        sql: include_str!("../../migrations/V039__users_table.sql"),
    },
    Migration {
        version: "V040__chat_sessions_user_id",
        sql: include_str!("../../migrations/V040__chat_sessions_user_id.sql"),
    },
    Migration {
        version: "V041__users_status",
        sql: include_str!("../../migrations/V041__users_status.sql"),
    },
    Migration {
        version: "V042__split_chat_tables",
        sql: include_str!("../../migrations/V042__split_chat_tables.sql"),
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
