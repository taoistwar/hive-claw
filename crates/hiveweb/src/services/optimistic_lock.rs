//! 乐观锁 helper（T150；data-model 不变量 #1..#10 之外的并发编辑保护）
//!
//! 调用约定：
//! 1. client 在 PUT 请求体中携带从 GET 读到的 `updated_at`
//! 2. service 层在 UPDATE 之前调 `check_and_bump`：
//!    - 比对 DB 中当前 `updated_at` 与 client 提供值
//!    - 不一致 → 返 [`AppError::OptimisticLockConflict`]（业务码 4094 / HTTP 409）
//!    - 一致 → 继续走 UPDATE（UPDATE 会触发 `ON UPDATE CURRENT_TIMESTAMP` 自动 bump）
//!
//! 适用表（contracts/api.md PUT 端点）：plugins / workflows / tools / skills / agents
//! categories / tags / functions（updated_at 列存在的全部）。

use chrono::{DateTime, Utc};
use sqlx::MySqlPool;

use crate::utils::error::AppError;

/// 校验 `table.updated_at` 与 client 期望一致。一致 → Ok；不一致 → 4094。
///
/// `table` 必须是受信任的静态字符串（来自代码常量），**不可**接受用户输入；
/// 否则有 SQL 注入风险。当前调用方都传字面量（"plugins" 等），sqlx 不支持
/// 表名 bind，故采用静态拼接。
pub async fn check_and_bump(
    pool: &MySqlPool,
    table: &'static str,
    id: i64,
    client_updated_at: DateTime<Utc>,
) -> Result<(), AppError> {
    let sql = format!("SELECT updated_at FROM {table} WHERE id = ?");
    let row: Option<(DateTime<Utc>,)> = sqlx::query_as(&sql)
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(|e| AppError::Internal(format!("optimistic_lock fetch failed: {e}")))?;

    let Some((db_updated_at,)) = row else {
        return Err(AppError::NotFound(format!(
            "{table} id={id} not found"
        )));
    };

    // MySQL DATETIME 仅秒级精度；比较时强制对齐到秒，避免亚秒级差异误判冲突。
    let db_secs = db_updated_at.timestamp();
    let client_secs = client_updated_at.timestamp();
    if db_secs != client_secs {
        return Err(AppError::OptimisticLockConflict(
            "内容已被他人修改，请刷新后重试".into(),
        ));
    }
    Ok(())
}
