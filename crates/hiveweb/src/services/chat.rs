//! Chat service (T126 / US6)
//!
//! session CRUD + message persist + history。
//! 所有权语义（FR-027 v7 / data-model 不变量 #12）：
//!   - admin 会话：admin 存活 → 必须 session.admin_id == JWT.admin_id（或 Super）
//!   - admin 已删除 (admin_id IS NULL) → 仅 Super 可访问
//!   - user 会话：必须 session.user_id == JWT.user_id
//! admin_phone_snapshot / admin_nickname_snapshot 由 service 在创建时写入，
//! admin 删后保留追溯。user_phone_snapshot / user_nickname_snapshot 同理。

use serde::{Deserialize, Serialize};
use sqlx::MySqlPool;

use crate::models::{ChatMessage, ChatSession};
use crate::utils::error::AppError;

#[derive(Debug, Deserialize)]
pub struct CreateSession {
    pub title: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SessionList {
    pub items: Vec<ChatSession>,
    pub total: i64,
}

/// 创建 admin session — 写入 admin snapshot
pub async fn create_session(
    pool: &MySqlPool,
    admin_id: i64,
    title: Option<String>,
) -> Result<ChatSession, AppError> {
    // 取 admin 信息作为 snapshot
    let admin: Option<(String, String)> =
        sqlx::query_as("SELECT phone, nickname FROM admins WHERE id = ?")
            .bind(admin_id)
            .fetch_optional(pool)
            .await
            .map_err(|e| AppError::Internal(format!("admin lookup: {e}")))?;
    let (phone, nickname) = admin.unwrap_or_else(|| ("".into(), "".into()));

    let res = sqlx::query(
        r#"INSERT INTO chat_sessions
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

    fetch_session(pool, res.last_insert_id() as i64).await
}

/// 创建 user session — 写入 user snapshot
pub async fn create_user_session(
    pool: &MySqlPool,
    user_id: i64,
    title: Option<String>,
) -> Result<ChatSession, AppError> {
    // 取 user 信息作为 snapshot
    let user: Option<(String,)> =
        sqlx::query_as("SELECT phone FROM users WHERE id = ?")
            .bind(user_id)
            .fetch_optional(pool)
            .await
            .map_err(|e| AppError::Internal(format!("user lookup: {e}")))?;
    let phone = user.map(|u| u.0).unwrap_or_default();

    let res = sqlx::query(
        r#"INSERT INTO chat_sessions
           (user_id, user_phone_snapshot, user_nickname_snapshot, title)
           VALUES (?, ?, '', ?)"#,
    )
    .bind(user_id)
    .bind(&phone)
    .bind(&title)
    .execute(pool)
    .await
    .map_err(|e| AppError::Internal(format!("user session insert: {e}")))?;

    fetch_session(pool, res.last_insert_id() as i64).await
}

pub async fn fetch_session(pool: &MySqlPool, id: i64) -> Result<ChatSession, AppError> {
    sqlx::query_as::<_, ChatSession>("SELECT * FROM chat_sessions WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(|e| AppError::Internal(format!("session fetch: {e}")))?
        .ok_or_else(|| AppError::NotFound(format!("session id={id} not found")))
}

/// 会话所有权校验（service 层强制 — FR-027 v7）：
///   - actor_role=3 (Super) → 全部放行
///   - admin session：admin_id IS NULL → 非 Super 拒绝；admin_id != actor → 拒绝
///   - user session：user_id != actor_user_id → 拒绝
pub fn check_ownership(
    session: &ChatSession,
    actor_admin_id: Option<i64>,
    actor_user_id: Option<i64>,
    actor_role: i8,
) -> Result<(), AppError> {
    if actor_role == 3 {
        return Ok(());
    }

    // Admin session
    if session.admin_id.is_some() {
        match (actor_admin_id, session.admin_id) {
            (Some(admin_id), Some(owner_id)) if admin_id == owner_id => Ok(()),
            (Some(_), Some(_)) => Err(AppError::InsufficientPermission(
                "无权访问他人会话".into(),
            )),
            (Some(_), None) => Err(AppError::InsufficientPermission(
                "操作者已被删除，仅 Super 可继续访问此会话".into(),
            )),
            (None, _) => Err(AppError::InsufficientPermission(
                "无权访问管理员会话".into(),
            )),
        }
    } else if session.user_id.is_some() {
        // User session
        match actor_user_id {
            Some(user_id) if session.user_id == Some(user_id) => Ok(()),
            Some(_) => Err(AppError::InsufficientPermission(
                "无权访问他人会话".into(),
            )),
            None => Err(AppError::InsufficientPermission(
                "无权访问用户会话".into(),
            )),
        }
    } else {
        Err(AppError::InsufficientPermission("无效会话".into()))
    }
}

pub async fn list_sessions(
    pool: &MySqlPool,
    actor_admin_id: Option<i64>,
    actor_user_id: Option<i64>,
    actor_role: i8,
    offset: i64,
    limit: i64,
    search: Option<&str>,
) -> Result<SessionList, AppError> {
    let (count_base, list_base, bind_filter) = if actor_role == 3 {
        // Super can see all sessions
        (
            "SELECT COUNT(*) FROM chat_sessions",
            "SELECT * FROM chat_sessions",
            false,
        )
    } else if actor_admin_id.is_some() {
        // Admin: see own sessions (both admin and user-created under this admin)
        (
            "SELECT COUNT(*) FROM chat_sessions WHERE admin_id = ?",
            "SELECT * FROM chat_sessions WHERE admin_id = ?",
            true,
        )
    } else if actor_user_id.is_some() {
        // User: see own sessions
        (
            "SELECT COUNT(*) FROM chat_sessions WHERE user_id = ?",
            "SELECT * FROM chat_sessions WHERE user_id = ?",
            true,
        )
    } else {
        return Err(AppError::Internal("No valid actor".into()));
    };

    let (search_clause, bind_search) = match search {
        Some(s) if !s.is_empty() => (" AND title LIKE ?", true),
        _ => ("", false),
    };

    let count_sql = format!("{}{}", count_base, search_clause);
    let list_sql = format!("{}{} ORDER BY updated_at DESC LIMIT ? OFFSET ?", list_base, search_clause);

    let total: (i64,) = if bind_filter && bind_search {
        let id = actor_admin_id.or(actor_user_id).unwrap();
        sqlx::query_as(&count_sql)
            .bind(id)
            .bind(format!("%{}%", search.unwrap()))
            .fetch_one(pool)
            .await
    } else if bind_filter {
        let id = actor_admin_id.or(actor_user_id).unwrap();
        sqlx::query_as(&count_sql)
            .bind(id)
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
    .map_err(|e| AppError::Internal(format!("session count: {e}")))?;

    let items: Vec<ChatSession> = if bind_filter && bind_search {
        let id = actor_admin_id.or(actor_user_id).unwrap();
        sqlx::query_as(&list_sql)
            .bind(id)
            .bind(format!("%{}%", search.unwrap()))
            .bind(limit)
            .bind(offset)
            .fetch_all(pool)
            .await
    } else if bind_filter {
        let id = actor_admin_id.or(actor_user_id).unwrap();
        sqlx::query_as(&list_sql)
            .bind(id)
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
    .map_err(|e| AppError::Internal(format!("session list: {e}")))?;

    Ok(SessionList { items, total: total.0 })
}

pub async fn list_messages(
    pool: &MySqlPool,
    session_id: i64,
) -> Result<Vec<ChatMessage>, AppError> {
    sqlx::query_as::<_, ChatMessage>(
        "SELECT * FROM chat_messages WHERE session_id = ? ORDER BY seq ASC",
    )
    .bind(session_id)
    .fetch_all(pool)
    .await
    .map_err(|e| AppError::Internal(format!("messages list: {e}")))
}

#[derive(Debug, Deserialize)]
pub struct PostMessage {
    pub content: String,
}

/// 追加 user 消息（在 SSE 开始流之前 persist；中断时 assistant 部分不入库）
pub async fn append_user_message(
    pool: &MySqlPool,
    session_id: i64,
    content: &str,
) -> Result<ChatMessage, AppError> {
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| AppError::Internal(format!("tx begin: {e}")))?;
    let next_seq: (Option<i32>,) = sqlx::query_as(
        "SELECT MAX(seq) FROM chat_messages WHERE session_id = ?",
    )
    .bind(session_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| AppError::Internal(format!("seq fetch: {e}")))?;
    let seq = next_seq.0.unwrap_or(0) + 1;
    let res = sqlx::query(
        r#"INSERT INTO chat_messages (session_id, seq, role, content) VALUES (?, ?, 'user', ?)"#,
    )
    .bind(session_id)
    .bind(seq)
    .bind(content)
    .execute(&mut *tx)
    .await
    .map_err(|e| AppError::Internal(format!("message insert: {e}")))?;
    // bump session.updated_at
    sqlx::query("UPDATE chat_sessions SET updated_at = NOW() WHERE id = ?")
        .bind(session_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| AppError::Internal(format!("session bump: {e}")))?;
    tx.commit()
        .await
        .map_err(|e| AppError::Internal(format!("tx commit: {e}")))?;

    let id = res.last_insert_id() as i64;
    sqlx::query_as::<_, ChatMessage>("SELECT * FROM chat_messages WHERE id = ?")
        .bind(id)
        .fetch_one(pool)
        .await
        .map_err(|e| AppError::Internal(format!("message refetch: {e}")))
}

pub async fn append_assistant_message(
    pool: &MySqlPool,
    session_id: i64,
    content: &str,
    routed_to: Option<i64>,
    elapsed_ms: Option<i32>,
) -> Result<(), AppError> {
    let next_seq: (Option<i32>,) =
        sqlx::query_as("SELECT MAX(seq) FROM chat_messages WHERE session_id = ?")
            .bind(session_id)
            .fetch_one(pool)
            .await
            .map_err(|e| AppError::Internal(format!("seq fetch: {e}")))?;
    let seq = next_seq.0.unwrap_or(0) + 1;
    sqlx::query(
        r#"INSERT INTO chat_messages
           (session_id, seq, role, content, routed_to_agent_id, elapsed_ms)
           VALUES (?, ?, 'assistant', ?, ?, ?)"#,
    )
    .bind(session_id)
    .bind(seq)
    .bind(content)
    .bind(routed_to)
    .bind(elapsed_ms)
    .execute(pool)
    .await
    .map_err(|e| AppError::Internal(format!("assistant insert: {e}")))?;
    Ok(())
}

pub async fn delete_session(
    pool: &MySqlPool,
    session_id: i64,
) -> Result<(), AppError> {
    sqlx::query("DELETE FROM chat_sessions WHERE id = ?")
        .bind(session_id)
        .execute(pool)
        .await
        .map_err(|e| AppError::Internal(format!("session delete: {e}")))?;
    Ok(())
}
