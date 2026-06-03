//! Chat service — user chat tables only.
//!
//! Tables: chat_sessions_user + chat_messages_user
//!
//! Ownership semantics:
//!   - User session: user_id must match JWT user_id

use sqlx::MySqlPool;

use crate::models::{ChatMessageUser, ChatSessionUser};
use crate::services::chat::{SessionList, SessionListItem};
use crate::utils::error::AppError;

// --- User session CRUD ---

pub async fn create_user_session(
    pool: &MySqlPool,
    user_id: i64,
    title: Option<String>,
) -> Result<ChatSessionUser, AppError> {
    let user: Option<(String,)> = sqlx::query_as("SELECT phone FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_optional(pool)
        .await
        .map_err(|e| AppError::Internal(format!("user lookup: {e}")))?;
    let phone = user.map(|u| u.0).unwrap_or_default();

    let res = sqlx::query(
        r#"INSERT INTO chat_sessions_user
           (user_id, user_phone_snapshot, user_nickname_snapshot, title)
           VALUES (?, ?, '', ?)"#,
    )
    .bind(user_id)
    .bind(&phone)
    .bind(&title)
    .execute(pool)
    .await
    .map_err(|e| AppError::Internal(format!("user session insert: {e}")))?;

    fetch_session_user(pool, res.last_insert_id() as i64).await
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

    let count_sql = format!("{}{}", count_base, search_clause);
    let list_sql = format!(
        "{}{} ORDER BY updated_at DESC LIMIT ? OFFSET ?",
        list_base, search_clause
    );

    let total: (i64,) = if bind_search {
        sqlx::query_as(&count_sql)
            .bind(user_id)
            .bind(format!("%{}%", search.unwrap()))
            .fetch_one(pool)
            .await
    } else {
        sqlx::query_as(&count_sql)
            .bind(user_id)
            .fetch_one(pool)
            .await
    }
    .map_err(|e| AppError::Internal(format!("user session count: {e}")))?;

    let items: Vec<ChatSessionUser> = if bind_search {
        sqlx::query_as(&list_sql)
            .bind(user_id)
            .bind(format!("%{}%", search.unwrap()))
            .bind(limit)
            .bind(offset)
            .fetch_all(pool)
            .await
    } else {
        sqlx::query_as(&list_sql)
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
    sqlx::query_as::<_, ChatMessageUser>(
        "SELECT * FROM chat_messages_user WHERE session_id = ? ORDER BY id ASC",
    )
    .bind(session_id)
    .fetch_all(pool)
    .await
    .map_err(|e| AppError::Internal(format!("user messages list: {e}")))
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
) -> Result<(), AppError> {
    sqlx::query(
        r#"INSERT INTO chat_messages_user
           (session_id, user_id, role, content, elapsed_ms)
           VALUES (?, ?, 'assistant', ?, ?)"#,
    )
    .bind(session_id)
    .bind(user_id)
    .bind(content)
    .bind(elapsed_ms)
    .execute(pool)
    .await
    .map_err(|e| AppError::Internal(format!("user assistant insert: {e}")))?;
    Ok(())
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
