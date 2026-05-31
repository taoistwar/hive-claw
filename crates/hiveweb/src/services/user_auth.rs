//! User authentication service

use sqlx::MySqlPool;

use crate::models::User;
use crate::utils::error::AppError;
use crate::utils::password::{hash_password, verify_password};

pub async fn find_user_by_phone(pool: &MySqlPool, phone: &str) -> Result<Option<User>, AppError> {
    sqlx::query_as::<_, User>(
        "SELECT id, phone, password_hash, status, created_at, updated_at, last_login_at FROM users WHERE phone = ?",
    )
        .bind(phone)
        .fetch_optional(pool)
        .await
        .map_err(|e| AppError::Internal(format!("user lookup: {e}")))
}

pub async fn register_user(
    pool: &MySqlPool,
    phone: &str,
    password: &str,
) -> Result<User, AppError> {
    // Check if phone already registered
    if let Some(_existing) = find_user_by_phone(pool, phone).await? {
        return Err(AppError::Conflict("Phone already registered".to_string()));
    }

    let password_hash = hash_password(password)
        .map_err(|e| AppError::Internal(format!("password hash: {e}")))?;

    let res = sqlx::query(
        "INSERT INTO users (phone, password_hash, status) VALUES (?, ?, 1)",
    )
    .bind(phone)
    .bind(&password_hash)
    .execute(pool)
    .await
    .map_err(|e| AppError::Internal(format!("user insert: {e}")))?;

    let id = res.last_insert_id() as i64;
    sqlx::query_as::<_, User>(
        "SELECT id, phone, password_hash, status, created_at, updated_at, last_login_at FROM users WHERE id = ?",
    )
        .bind(id)
        .fetch_one(pool)
        .await
        .map_err(|e| AppError::Internal(format!("user refetch: {e}")))
}

pub async fn login_user(
    pool: &MySqlPool,
    phone: &str,
    password: &str,
) -> Result<User, AppError> {
    let user = find_user_by_phone(pool, phone)
        .await?
        .ok_or_else(|| AppError::WrongPassword("Invalid phone or password".to_string()))?;

    if user.status != 1 {
        return Err(AppError::AccountDisabled("Account has been disabled".to_string()));
    }

    let valid = verify_password(password, &user.password_hash)
        .map_err(|e| AppError::Internal(format!("password verify: {e}")))?;

    if !valid {
        return Err(AppError::WrongPassword("Invalid phone or password".to_string()));
    }

    Ok(user)
}

pub async fn update_last_login(pool: &MySqlPool, user_id: i64) -> Result<(), AppError> {
    sqlx::query("UPDATE users SET last_login_at = NOW() WHERE id = ?")
        .bind(user_id)
        .execute(pool)
        .await
        .map_err(|e| AppError::Internal(format!("last_login update: {e}")))?;
    Ok(())
}
