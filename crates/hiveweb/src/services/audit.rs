//! Admin operation audit log writer (T093 / spec FR-022).
//!
//! Persists one row per administrative action. Snapshot columns retain the
//! human-readable identity of both operator and target so the trail survives
//! deletion of either admin (90-day retention requirement).

use anyhow::Result;
use serde_json::Value;
use sqlx::MySqlPool;

/// Convert an optional JSON value to a `Option<String>` payload bindable by
/// sqlx-mysql without the `json` feature. MySQL parses the string into the
/// JSON column on insert.
fn encode_detail(detail: Option<Value>) -> Option<String> {
    detail.map(|v| v.to_string())
}

#[derive(Debug, Clone, Copy)]
pub enum Operation {
    Create,
    Update,
    Delete,
    Enable,
    Disable,
    GameAliasCreate,
    GameAliasUpdate,
    GameAliasDelete,
}

impl Operation {
    pub fn as_str(&self) -> &'static str {
        match self {
            Operation::Create => "create",
            Operation::Update => "update",
            Operation::Delete => "delete",
            Operation::Enable => "enable",
            Operation::Disable => "disable",
            Operation::GameAliasCreate => "game_alias_create",
            Operation::GameAliasUpdate => "game_alias_update",
            Operation::GameAliasDelete => "game_alias_delete",
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn record(
    pool: &MySqlPool,
    operator_id: i64,
    operator_phone: &str,
    target_admin_id: Option<i64>,
    target_phone: &str,
    op: Operation,
    detail: Option<Value>,
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO admin_audit_logs
            (operator_id, operator_phone_snapshot,
             target_admin_id, target_phone_snapshot,
             operation, detail, occurred_at)
        VALUES (?, ?, ?, ?, ?, ?, NOW())
        "#,
    )
    .bind(operator_id)
    .bind(operator_phone)
    .bind(target_admin_id)
    .bind(target_phone)
    .bind(op.as_str())
    .bind(encode_detail(detail))
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn record_game_alias(
    pool: &MySqlPool,
    operator_id: i64,
    operator_phone: &str,
    game_id: i64,
    game_name: &str,
    op: Operation,
    detail: Option<Value>,
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO admin_audit_logs
            (operator_id, operator_phone_snapshot,
             target_admin_id, target_phone_snapshot,
             operation, detail, occurred_at)
        VALUES (?, ?, NULL, ?, ?, ?, NOW())
        "#,
    )
    .bind(operator_id)
    .bind(operator_phone)
    .bind(game_name)
    .bind(op.as_str())
    .bind(encode_detail(detail))
    .execute(pool)
    .await?;
    Ok(())
}
