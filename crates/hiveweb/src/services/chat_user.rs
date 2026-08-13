//! Chat service — user chat tables only.
//!
//! Tables: chat_sessions_user + chat_messages_user
//!
//! Ownership semantics:
//!   - User session: user_id must match JWT user_id

use sqlx::MySqlPool;

use crate::db::sql_safety::audit_sql;
use crate::models::{ChatMessageUser, ChatSessionUser};
use crate::services::chat::{SessionList, SessionListItem};
use crate::utils::error::AppError;

// --- User session CRUD ---

pub async fn create_user_session(
    pool: &MySqlPool,
    user_id: i64,
    title: Option<String>,
) -> Result<ChatSessionUser, AppError> {
    let user: Option<(String,)> =
        sqlx::query_as("SELECT COALESCE(uid, '') FROM users WHERE id = ?")
            .bind(user_id)
            .fetch_optional(pool)
            .await
            .map_err(|e| AppError::Internal(format!("user lookup: {e}")))?;
    let uid = user.map(|u| u.0).unwrap_or_default();

    let res = sqlx::query(
        r#"INSERT INTO chat_sessions_user
           (user_id, user_phone_snapshot, user_nickname_snapshot, title)
           VALUES (?, ?, '', ?)"#,
    )
    .bind(user_id)
    .bind(&uid)
    .bind(&title)
    .execute(pool)
    .await
    .map_err(|e| AppError::Internal(format!("user session insert: {e}")))?;

    fetch_session_user(pool, res.last_insert_id() as i64).await
}

/// 事务性清除历史会话/聊天记录 + 创建新会话。
/// 要么全部成功，要么全部回滚。
///
/// 任一 `?` 提前返回时，`tx` 被 drop 自动触发 ROLLBACK；
/// 只有到达 `tx.commit()` 才真正提交。
pub async fn clear_and_create_session(
    pool: &MySqlPool,
    user_id: i64,
    title: Option<String>,
) -> Result<ChatSessionUser, AppError> {
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| AppError::Internal(format!("tx begin: {e}")))?;

    // 1. 删除该用户的所有聊天记录
    sqlx::query(
        "DELETE FROM chat_messages_user WHERE session_id IN (SELECT id FROM chat_sessions_user WHERE user_id = ?)",
    )
    .bind(user_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| AppError::Internal(format!("user messages cleanup: {e}")))?;

    // 2. 删除该用户的所有会话
    sqlx::query("DELETE FROM chat_sessions_user WHERE user_id = ?")
        .bind(user_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| AppError::Internal(format!("user sessions cleanup: {e}")))?;

    // 3. 查询用户 uid
    let user: Option<(String,)> =
        sqlx::query_as("SELECT COALESCE(uid, '') FROM users WHERE id = ?")
            .bind(user_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|e| AppError::Internal(format!("user lookup: {e}")))?;
    let uid = user.map(|u| u.0).unwrap_or_default();

    // 4. 创建新会话
    let res = sqlx::query(
        r#"INSERT INTO chat_sessions_user
           (user_id, user_phone_snapshot, user_nickname_snapshot, title)
           VALUES (?, ?, '', ?)"#,
    )
    .bind(user_id)
    .bind(&uid)
    .bind(&title)
    .execute(&mut *tx)
    .await
    .map_err(|e| AppError::Internal(format!("user session insert: {e}")))?;

    let new_id = res.last_insert_id() as i64;

    // 5. 提交事务
    tx.commit()
        .await
        .map_err(|e| AppError::Internal(format!("tx commit: {e}")))?;

    // 6. 查询并返回新会话
    fetch_session_user(pool, new_id).await
}

pub async fn fetch_session_user(pool: &MySqlPool, id: i64) -> Result<ChatSessionUser, AppError> {
    sqlx::query_as::<_, ChatSessionUser>("SELECT * FROM chat_sessions_user WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(|e| AppError::Internal(format!("user session fetch: {e}")))?
        .ok_or_else(|| AppError::NotFound(format!("user session id={id} not found")))
}

pub fn check_ownership_user(
    session: &ChatSessionUser,
    actor_user_id: Option<i64>,
) -> Result<(), AppError> {
    match actor_user_id {
        Some(user_id) if session.user_id == user_id => Ok(()),
        Some(_) => Err(AppError::InsufficientPermission("无权访问他人会话".into())),
        None => Err(AppError::InsufficientPermission("无权访问用户会话".into())),
    }
}

pub async fn list_sessions_user(
    pool: &MySqlPool,
    user_id: i64,
    offset: i64,
    limit: i64,
    search: Option<&str>,
) -> Result<SessionList, AppError> {
    let count_base = "SELECT COUNT(*) FROM chat_sessions_user WHERE user_id = ?";
    let list_base = "SELECT * FROM chat_sessions_user WHERE user_id = ?";

    let (search_clause, bind_search) = match search {
        Some(s) if !s.is_empty() => (" AND title LIKE ?", true),
        _ => ("", false),
    };

    let count_sql = audit_sql(format!("{}{}", count_base, search_clause));
    let list_sql = audit_sql(format!(
        "{}{} ORDER BY updated_at DESC LIMIT ? OFFSET ?",
        list_base, search_clause
    ));

        let total: (i64,) = if bind_search {
            sqlx::query_as(count_sql)
                .bind(user_id)
                .bind(format!("%{}%", search.unwrap()))
                .fetch_one(pool)
                .await
        } else {
            sqlx::query_as(count_sql)
                .bind(user_id)
                .fetch_one(pool)
                .await
        }
    .map_err(|e| AppError::Internal(format!("user session count: {e}")))?;

        let items: Vec<ChatSessionUser> = if bind_search {
        sqlx::query_as(list_sql)
            .bind(user_id)
            .bind(format!("%{}%", search.unwrap()))
            .bind(limit)
            .bind(offset)
            .fetch_all(pool)
            .await
    } else {
        sqlx::query_as(list_sql)
            .bind(user_id)
            .bind(limit)
            .bind(offset)
            .fetch_all(pool)
            .await
    }
    .map_err(|e| AppError::Internal(format!("user session list: {e}")))?;

    Ok(SessionList {
        items: items.into_iter().map(SessionListItem::User).collect(),
        total: total.0,
    })
}

// --- User messages ---

pub async fn list_messages_user(
    pool: &MySqlPool,
    session_id: i64,
) -> Result<Vec<ChatMessageUser>, AppError> {
    let mut res = sqlx::query_as::<_, ChatMessageUser>(
        "SELECT * FROM chat_messages_user WHERE session_id = ? ORDER BY created_at DESC, id DESC limit 5",
    )
    .bind(session_id)
    .fetch_all(pool)
    .await
    .map_err(|e| AppError::Internal(format!("user messages list: {e}")))?;
    res.reverse();
    Ok(res)
}

/// Query messages for a user before a given cutoff datetime (UTC).
/// Returns up to 10 most recent messages.
pub async fn list_messages_before(
    pool: &MySqlPool,
    user_id: i64,
    cutoff: chrono::NaiveDateTime,
) -> Result<Vec<ChatMessageUser>, AppError> {
    let sql = "SELECT id, session_id, user_id, role, content, elapsed_ms, extensions, created_at \
               FROM chat_messages_user \
               WHERE user_id = ? AND UNIX_TIMESTAMP(created_at) < ? \
               ORDER BY created_at DESC, id DESC \
               LIMIT 10";
    let timestamp = cutoff.and_utc().timestamp();
    tracing::debug!(%user_id, %cutoff, %timestamp, sql, "list_messages_before");

    let res = sqlx::query_as::<_, ChatMessageUser>(sql)
        .bind(user_id)
        .bind(timestamp)
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Internal(format!("user messages before: {e}")))?;
    Ok(res)
}

pub async fn append_user_message_user(
    pool: &MySqlPool,
    session_id: i64,
    user_id: i64,
    content: &str,
) -> Result<(), AppError> {
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| AppError::Internal(format!("tx begin: {e}")))?;
    let _res = sqlx::query(
        r#"INSERT INTO chat_messages_user (session_id, user_id, role, content) VALUES (?, ?, 'user', ?)"#,
    )
    .bind(session_id)
    .bind(user_id)
    .bind(content)
    .execute(&mut *tx)
    .await
    .map_err(|e| AppError::Internal(format!("user message insert: {e}")))?;

    sqlx::query("UPDATE chat_sessions_user SET updated_at = NOW() WHERE id = ?")
        .bind(session_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| AppError::Internal(format!("user session bump: {e}")))?;

    tx.commit()
        .await
        .map_err(|e| AppError::Internal(format!("tx commit: {e}")))?;

    Ok(())
}

pub async fn append_assistant_message_user(
    pool: &MySqlPool,
    session_id: i64,
    user_id: i64,
    content: &str,
    elapsed_ms: Option<i32>,
    extensions: Option<serde_json::Value>,
) -> Result<ChatMessageUser, AppError> {
    let res = sqlx::query(
        r#"INSERT INTO chat_messages_user
           (session_id, user_id, role, content, elapsed_ms, extensions)
           VALUES (?, ?, 'assistant', ?, ?, ?)"#,
    )
    .bind(session_id)
    .bind(user_id)
    .bind(content)
    .bind(elapsed_ms)
    .bind(extensions)
    .execute(pool)
    .await
    .map_err(|e| AppError::Internal(format!("user assistant insert: {e}")))?;
    fetch_message_user(pool, res.last_insert_id() as i64).await
}

pub async fn fetch_message_user(pool: &MySqlPool, id: i64) -> Result<ChatMessageUser, AppError> {
    sqlx::query_as::<_, ChatMessageUser>("SELECT * FROM chat_messages_user WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(|e| AppError::Internal(format!("user message fetch: {e}")))?
        .ok_or_else(|| AppError::NotFound(format!("user message id={id} not found")))
}

// --- Delete ---

pub async fn delete_session_user(pool: &MySqlPool, session_id: i64) -> Result<(), AppError> {
    sqlx::query("DELETE FROM chat_sessions_user WHERE id = ?")
        .bind(session_id)
        .execute(pool)
        .await
        .map_err(|e| AppError::Internal(format!("user session delete: {e}")))?;
    Ok(())
}

/// Get the latest session for a user, or create a new one if none exists.
pub async fn get_or_create_session_user(
    pool: &MySqlPool,
    user_id: i64,
) -> Result<ChatSessionUser, AppError> {
    let existing: Option<ChatSessionUser> = sqlx::query_as(
        "SELECT * FROM chat_sessions_user WHERE user_id = ? ORDER BY updated_at DESC LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| AppError::Internal(format!("session lookup: {e}")))?;

    if let Some(session) = existing {
        return Ok(session);
    }

    create_user_session(pool, user_id, None).await
}
