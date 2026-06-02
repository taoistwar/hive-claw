//! Chat service — admin and user tables are completely separate.
//!
//! Admin tables: chat_sessions_admin + chat_messages_admin
//! User tables: chat_sessions_user + chat_messages_user
//!
//! Ownership semantics:
//!   - Admin session: admin_id must match JWT admin_id (or Super)
//!   - User session: user_id must match JWT user_id

use sqlx::MySqlPool;

use crate::models::{ChatMessageAdmin,  ChatSessionAdmin};
use crate::services::chat::{SessionList, SessionListItem};
use crate::utils::error::AppError;

// --- Admin session CRUD ---

/// 创建 admin session
pub async fn create_session_admin(
    pool: &MySqlPool,
    admin_id: i64,
    title: Option<String>,
) -> Result<ChatSessionAdmin, AppError> {
    let admin: Option<(String, String)> =
        sqlx::query_as("SELECT phone, nickname FROM admins WHERE id = ?")
            .bind(admin_id)
            .fetch_optional(pool)
            .await
            .map_err(|e| AppError::Internal(format!("admin lookup: {e}")))?;
    let (phone, nickname) = admin.unwrap_or_else(|| ("".into(), "".into()));

    let res = sqlx::query(
        r#"INSERT INTO chat_sessions_admin
           (admin_id, admin_phone_snapshot, admin_nickname_snapshot, title)
           VALUES (?, ?, ?, ?)"#,
    )
    .bind(admin_id)
    .bind(&phone)
    .bind(&nickname)
    .bind(&title)
    .execute(pool)
    .await
    .map_err(|e| AppError::Internal(format!("session insert: {e}")))?;

    fetch_session_admin(pool, res.last_insert_id() as i64).await
}

pub async fn fetch_session_admin(pool: &MySqlPool, id: i64) -> Result<ChatSessionAdmin, AppError> {
    sqlx::query_as::<_, ChatSessionAdmin>("SELECT * FROM chat_sessions_admin WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(|e| AppError::Internal(format!("admin session fetch: {e}")))?
        .ok_or_else(|| AppError::NotFound(format!("admin session id={id} not found")))
}

/// 会话所有权校验 — admin 会话
pub fn check_ownership_admin(
    session: &ChatSessionAdmin,
    actor_admin_id: Option<i64>,
    actor_role: i8,
) -> Result<(), AppError> {
    if actor_role == 3 {
        return Ok(());
    }
    match actor_admin_id {
        Some(admin_id) if session.admin_id == admin_id => Ok(()),
        Some(_) => Err(AppError::InsufficientPermission("无权访问他人会话".into())),
        None => Err(AppError::InsufficientPermission(
            "无权访问管理员会话".into(),
        )),
    }
}

pub async fn list_sessions_admin(
    pool: &MySqlPool,
    actor_admin_id: Option<i64>,
    actor_role: i8,
    offset: i64,
    limit: i64,
    search: Option<&str>,
) -> Result<SessionList, AppError> {
    let (count_base, list_base, bind_filter) = if actor_role == 3 {
        (
            "SELECT COUNT(*) FROM chat_sessions_admin",
            "SELECT * FROM chat_sessions_admin",
            false,
        )
    } else if let Some(admin_id) = actor_admin_id {
        (
            "SELECT COUNT(*) FROM chat_sessions_admin WHERE admin_id = ?",
            "SELECT * FROM chat_sessions_admin WHERE admin_id = ?",
            true,
        )
    } else {
        return Err(AppError::Internal("No valid admin actor".into()));
    };

    let (search_clause, bind_search) = match search {
        Some(s) if !s.is_empty() => (" AND title LIKE ?", true),
        _ => ("", false),
    };

    let count_sql = format!("{}{}", count_base, search_clause);
    let list_sql = format!(
        "{}{} ORDER BY updated_at DESC LIMIT ? OFFSET ?",
        list_base, search_clause
    );

    let total: (i64,) = if bind_filter && bind_search {
        sqlx::query_as(&count_sql)
            .bind(actor_admin_id.unwrap())
            .bind(format!("%{}%", search.unwrap()))
            .fetch_one(pool)
            .await
    } else if bind_filter {
        sqlx::query_as(&count_sql)
            .bind(actor_admin_id.unwrap())
            .fetch_one(pool)
            .await
    } else if bind_search {
        sqlx::query_as(&count_sql)
            .bind(format!("%{}%", search.unwrap()))
            .fetch_one(pool)
            .await
    } else {
        sqlx::query_as(&count_sql).fetch_one(pool).await
    }
    .map_err(|e| AppError::Internal(format!("admin session count: {e}")))?;

    let items: Vec<ChatSessionAdmin> = if bind_filter && bind_search {
        sqlx::query_as(&list_sql)
            .bind(actor_admin_id.unwrap())
            .bind(format!("%{}%", search.unwrap()))
            .bind(limit)
            .bind(offset)
            .fetch_all(pool)
            .await
    } else if bind_filter {
        sqlx::query_as(&list_sql)
            .bind(actor_admin_id.unwrap())
            .bind(limit)
            .bind(offset)
            .fetch_all(pool)
            .await
    } else if bind_search {
        sqlx::query_as(&list_sql)
            .bind(format!("%{}%", search.unwrap()))
            .bind(limit)
            .bind(offset)
            .fetch_all(pool)
            .await
    } else {
        sqlx::query_as(&list_sql)
            .bind(limit)
            .bind(offset)
            .fetch_all(pool)
            .await
    }
    .map_err(|e| AppError::Internal(format!("admin session list: {e}")))?;

    Ok(SessionList {
        items: items.into_iter().map(SessionListItem::Admin).collect(),
        total: total.0,
    })
}

// --- Admin messages ---

pub async fn list_messages_admin(
    pool: &MySqlPool,
    session_id: i64,
) -> Result<Vec<ChatMessageAdmin>, AppError> {
    sqlx::query_as::<_, ChatMessageAdmin>(
        "SELECT * FROM chat_messages_admin WHERE session_id = ? ORDER BY id ASC",
    )
    .bind(session_id)
    .fetch_all(pool)
    .await
    .map_err(|e| AppError::Internal(format!("admin messages list: {e}")))
}

pub async fn append_user_message_admin(
    pool: &MySqlPool,
    session_id: i64,
    admin_id: i64,
    content: &str,
) -> Result<ChatMessageAdmin, AppError> {
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| AppError::Internal(format!("tx begin: {e}")))?;
    let res = sqlx::query(
        r#"INSERT INTO chat_messages_admin (session_id, admin_id, role, content) VALUES (?, ?, 'user', ?)"#,
    )
    .bind(session_id)
    .bind(admin_id)
    .bind(content)
    .execute(&mut *tx)
    .await
    .map_err(|e| AppError::Internal(format!("message insert: {e}")))?;

    sqlx::query("UPDATE chat_sessions_admin SET updated_at = NOW() WHERE id = ?")
        .bind(session_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| AppError::Internal(format!("session bump: {e}")))?;

    tx.commit()
        .await
        .map_err(|e| AppError::Internal(format!("tx commit: {e}")))?;

    let id = res.last_insert_id() as i64;
    sqlx::query_as::<_, ChatMessageAdmin>("SELECT * FROM chat_messages_admin WHERE id = ?")
        .bind(id)
        .fetch_one(pool)
        .await
        .map_err(|e| AppError::Internal(format!("message refetch: {e}")))
}

pub async fn append_assistant_message_admin(
    pool: &MySqlPool,
    session_id: i64,
    admin_id: i64,
    content: &str,
    routed_to: Option<i64>,
    elapsed_ms: Option<i32>,
) -> Result<(), AppError> {
    append_assistant_message_generic_admin(
        pool, session_id, admin_id, content, None, routed_to, elapsed_ms,
    )
    .await
}

pub async fn append_assistant_with_tool_calls_admin(
    pool: &MySqlPool,
    session_id: i64,
    admin_id: i64,
    content: &str,
    tool_calls_json: &str,
    elapsed_ms: Option<i32>,
) -> Result<(), AppError> {
    append_assistant_message_generic_admin(
        pool,
        session_id,
        admin_id,
        content,
        Some(tool_calls_json),
        None,
        elapsed_ms,
    )
    .await
}

async fn append_assistant_message_generic_admin(
    pool: &MySqlPool,
    session_id: i64,
    admin_id: i64,
    content: &str,
    tool_calls_json: Option<&str>,
    routed_to: Option<i64>,
    elapsed_ms: Option<i32>,
) -> Result<(), AppError> {
    if let Some(tc_json) = tool_calls_json {
        sqlx::query(
            r#"INSERT INTO chat_messages_admin
               (session_id, admin_id, role, content, tool_calls, routed_to_agent_id, elapsed_ms)
               VALUES (?, ?, 'assistant', ?, ?, ?, ?)"#,
        )
        .bind(session_id)
        .bind(admin_id)
        .bind(content)
        .bind(tc_json)
        .bind(routed_to)
        .bind(elapsed_ms)
        .execute(pool)
        .await
        .map_err(|e| AppError::Internal(format!("assistant insert: {e}")))?;
    } else {
        sqlx::query(
            r#"INSERT INTO chat_messages_admin
               (session_id, admin_id, role, content, routed_to_agent_id, elapsed_ms)
               VALUES (?, ?, 'assistant', ?, ?, ?)"#,
        )
        .bind(session_id)
        .bind(admin_id)
        .bind(content)
        .bind(routed_to)
        .bind(elapsed_ms)
        .execute(pool)
        .await
        .map_err(|e| AppError::Internal(format!("assistant insert: {e}")))?;
    }
    Ok(())
}

pub async fn append_tool_message_admin(
    pool: &MySqlPool,
    session_id: i64,
    admin_id: i64,
    tool_call_id: &str,
    name: &str,
    result_json: &str,
) -> Result<(), AppError> {
    let content = serde_json::to_string(&serde_json::json!({
        "tool_call_id": tool_call_id,
        "name": name,
        "result": result_json
    }))
    .unwrap_or_default();
    sqlx::query(
        r#"INSERT INTO chat_messages_admin
           (session_id, admin_id, role, content)
           VALUES (?, ?, 'tool', ?)"#,
    )
    .bind(session_id)
    .bind(admin_id)
    .bind(content)
    .execute(pool)
    .await
    .map_err(|e| AppError::Internal(format!("tool message insert: {e}")))?;
    Ok(())
}

// --- Delete ---

pub async fn delete_session_admin(pool: &MySqlPool, session_id: i64) -> Result<(), AppError> {
    sqlx::query("DELETE FROM chat_sessions_admin WHERE id = ?")
        .bind(session_id)
        .execute(pool)
        .await
        .map_err(|e| AppError::Internal(format!("admin session delete: {e}")))?;
    Ok(())
}
