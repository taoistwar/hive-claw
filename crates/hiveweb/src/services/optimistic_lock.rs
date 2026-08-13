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
//!
//! 仅允许下列静态表名，避免任意字符串拼接。

use chrono::{DateTime, Utc};
use sqlx::MySqlPool;

use crate::db::sql_safety::audit_sql;
use crate::utils::error::AppError;

/// Optimistic lock 覆盖表的封闭集合（供测试和调用方共用）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OptimisticLockTable {
    /// `agents` 表。
    Agents,
    /// `agent_hooks` 表。
    AgentHooks,
    /// `categories` 表。
    Categories,
    /// `functions` 表。
    Functions,
    /// `plugins` 表。
    Plugins,
    /// `recommended_games` 表。
    RecommendedGames,
    /// `skills` 表。
    Skills,
    /// `tags` 表。
    Tags,
    /// `tools` 表。
    Tools,
    /// `workflows` 表。
    Workflows,
}

impl OptimisticLockTable {
    /// 允许的全部表名（用于边界一致性校验）。
    pub const ALL: [Self; 10] = [
        Self::Agents,
        Self::AgentHooks,
        Self::Categories,
        Self::Functions,
        Self::Plugins,
        Self::RecommendedGames,
        Self::Skills,
        Self::Tags,
        Self::Tools,
        Self::Workflows,
    ];

    /// 表名对应的生产 SQL 片段。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Agents => "agents",
            Self::AgentHooks => "agent_hooks",
            Self::Categories => "categories",
            Self::Functions => "functions",
            Self::Plugins => "plugins",
            Self::RecommendedGames => "recommended_games",
            Self::Skills => "skills",
            Self::Tags => "tags",
            Self::Tools => "tools",
            Self::Workflows => "workflows",
        }
    }
}

const fn table_query_sql(table: OptimisticLockTable) -> &'static str {
    match table {
        OptimisticLockTable::Agents => "SELECT updated_at FROM agents WHERE id = ?",
        OptimisticLockTable::AgentHooks => "SELECT updated_at FROM agent_hooks WHERE id = ?",
        OptimisticLockTable::Categories => "SELECT updated_at FROM categories WHERE id = ?",
        OptimisticLockTable::Functions => "SELECT updated_at FROM functions WHERE id = ?",
        OptimisticLockTable::Plugins => "SELECT updated_at FROM plugins WHERE id = ?",
        OptimisticLockTable::RecommendedGames => {
            "SELECT updated_at FROM recommended_games WHERE id = ?"
        }
        OptimisticLockTable::Skills => "SELECT updated_at FROM skills WHERE id = ?",
        OptimisticLockTable::Tags => "SELECT updated_at FROM tags WHERE id = ?",
        OptimisticLockTable::Tools => "SELECT updated_at FROM tools WHERE id = ?",
        OptimisticLockTable::Workflows => "SELECT updated_at FROM workflows WHERE id = ?",
    }
}

/// 校验 `table.updated_at` 与 client 期望一致。一致 → Ok；不一致 → 4094。
///
/// `table` 必须来自 `OptimisticLockTable`，不接受用户输入；
/// sqlx 不支持表名 bind，故通过有限枚举选择固定 SQL 常量。
pub async fn check_and_bump(
    pool: &MySqlPool,
    table: OptimisticLockTable,
    id: i64,
    client_updated_at: DateTime<Utc>,
) -> Result<(), AppError> {
    let sql = table_query_sql(table);
    let row: Option<(DateTime<Utc>,)> = sqlx::query_as(audit_sql(sql.to_string()))
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(|e| AppError::Internal(format!("optimistic_lock fetch failed: {e}")))?;

    let Some((db_updated_at,)) = row else {
        return Err(AppError::NotFound(format!("{} id={id} not found", table.as_str())));
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

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{table_query_sql, OptimisticLockTable};

    #[test]
    fn optimistic_lock_table_list_is_complete_and_unique() {
        let names: Vec<_> = OptimisticLockTable::ALL.iter().map(|t| t.as_str()).collect();

        assert_eq!(names.len(), 10, "enum 扩展时需要同步更新 ALL");
        assert_eq!(
            names.len(),
            names.iter().collect::<HashSet<_>>().len(),
            "table name 映射不能重复",
        );
        assert!(names.contains(&"agents"));
        assert!(names.contains(&"agent_hooks"));
        assert!(names.contains(&"categories"));
        assert!(names.contains(&"functions"));
        assert!(names.contains(&"plugins"));
        assert!(names.contains(&"recommended_games"));
        assert!(names.contains(&"skills"));
        assert!(names.contains(&"tags"));
        assert!(names.contains(&"tools"));
        assert!(names.contains(&"workflows"));
    }

    #[test]
    fn optimistic_lock_table_sql_is_static_and_table_specific() {
        assert_eq!(
            table_query_sql(OptimisticLockTable::Agents),
            "SELECT updated_at FROM agents WHERE id = ?"
        );
        assert_eq!(
            table_query_sql(OptimisticLockTable::AgentHooks),
            "SELECT updated_at FROM agent_hooks WHERE id = ?"
        );
        assert_eq!(
            table_query_sql(OptimisticLockTable::Categories),
            "SELECT updated_at FROM categories WHERE id = ?"
        );
        assert_eq!(
            table_query_sql(OptimisticLockTable::Functions),
            "SELECT updated_at FROM functions WHERE id = ?"
        );
        assert_eq!(
            table_query_sql(OptimisticLockTable::Plugins),
            "SELECT updated_at FROM plugins WHERE id = ?"
        );
        assert_eq!(
            table_query_sql(OptimisticLockTable::RecommendedGames),
            "SELECT updated_at FROM recommended_games WHERE id = ?"
        );
        assert_eq!(
            table_query_sql(OptimisticLockTable::Skills),
            "SELECT updated_at FROM skills WHERE id = ?"
        );
        assert_eq!(
            table_query_sql(OptimisticLockTable::Tags),
            "SELECT updated_at FROM tags WHERE id = ?"
        );
        assert_eq!(
            table_query_sql(OptimisticLockTable::Tools),
            "SELECT updated_at FROM tools WHERE id = ?"
        );
        assert_eq!(
            table_query_sql(OptimisticLockTable::Workflows),
            "SELECT updated_at FROM workflows WHERE id = ?"
        );

        for table in OptimisticLockTable::ALL {
            let sql = table_query_sql(table);
            assert!(
                sql.starts_with("SELECT updated_at FROM ") && sql.ends_with(" WHERE id = ?"),
                "sql for {} must be static and parameterized",
                table.as_str()
            );
            assert!(
                !sql.contains('{') && !sql.contains('}'),
                "sql for {} must not contain dynamic formatting placeholders",
                table.as_str()
            );
        }
    }
}
