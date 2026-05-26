use anyhow::Result;
use sqlx::MySqlPool;

use crate::models::{Admin, Role};

pub async fn find_admin_by_phone(pool: &MySqlPool, phone: &str) -> Result<Option<Admin>> {
    let admin = sqlx::query_as::<_, Admin>(
        "SELECT * FROM admins WHERE phone = ?",
    )
    .bind(phone)
    .fetch_optional(pool)
    .await?;

    Ok(admin)
}

pub async fn update_last_login(pool: &MySqlPool, admin_id: i64) -> Result<()> {
    sqlx::query("UPDATE admins SET last_login_at = NOW() WHERE id = ?")
        .bind(admin_id)
        .execute(pool)
        .await?;

    Ok(())
}

pub async fn get_admin_by_id(pool: &MySqlPool, id: i64) -> Result<Option<Admin>> {
    let admin = sqlx::query_as::<_, Admin>("SELECT * FROM admins WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?;

    Ok(admin)
}

pub async fn list_admins(
    pool: &MySqlPool,
    offset: u32,
    limit: u32,
) -> Result<(Vec<Admin>, u64)> {
    let admins = sqlx::query_as::<_, Admin>(
        "SELECT * FROM admins ORDER BY created_at DESC LIMIT ? OFFSET ?",
    )
    .bind(limit as i64)
    .bind(offset as i64)
    .fetch_all(pool)
    .await?;

    let total: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM admins")
        .fetch_one(pool)
        .await?;

    Ok((admins, total.0 as u64))
}

pub async fn create_admin(
    pool: &MySqlPool,
    phone: &str,
    nickname: &str,
    password_hash: &str,
    role: i8,
) -> Result<Admin> {
    let existing = find_admin_by_phone(pool, phone).await?;
    if existing.is_some() {
        return Err(anyhow::anyhow!("Phone number already exists"));
    }

    let result = sqlx::query(
        r#"
        INSERT INTO admins (phone, nickname, password_hash, role, status, created_at, updated_at)
        VALUES (?, ?, ?, ?, 1, NOW(), NOW())
        "#,
    )
    .bind(phone)
    .bind(nickname)
    .bind(password_hash)
    .bind(role)
    .execute(pool)
    .await?;

    let id = result.last_insert_id() as i64;
    let admin = sqlx::query_as::<_, Admin>("SELECT * FROM admins WHERE id = ?")
        .bind(id)
        .fetch_one(pool)
        .await?;

    Ok(admin)
}

pub async fn update_admin(
    pool: &MySqlPool,
    id: i64,
    nickname: &str,
    role: i8,
) -> Result<Admin> {
    let result = sqlx::query(
        "UPDATE admins SET nickname = ?, role = ?, updated_at = NOW() WHERE id = ?",
    )
    .bind(nickname)
    .bind(role)
    .bind(id)
    .execute(pool)
    .await?;

    if result.rows_affected() == 0 {
        return Err(anyhow::anyhow!("Admin not found"));
    }

    get_admin_by_id(pool, id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Admin not found after update"))
}

pub async fn delete_admin(pool: &MySqlPool, id: i64) -> Result<()> {
    let admin = get_admin_by_id(pool, id).await?
        .ok_or_else(|| anyhow::anyhow!("Admin not found"))?;

    let role = Role::try_from(admin.role)
        .map_err(|_| anyhow::anyhow!("Invalid role"))?;

    if role == Role::Super {
        return Err(anyhow::anyhow!("Cannot delete super admin"));
    }

    sqlx::query("DELETE FROM admins WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;

    Ok(())
}

pub async fn toggle_admin_status(
    pool: &MySqlPool,
    id: i64,
    status: i8,
) -> Result<Admin> {
    let admin = get_admin_by_id(pool, id).await?
        .ok_or_else(|| anyhow::anyhow!("Admin not found"))?;

    let role = Role::try_from(admin.role)
        .map_err(|_| anyhow::anyhow!("Invalid role"))?;

    if role == Role::Super && status == 0 {
        let super_admin_count: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM admins WHERE role = 3 AND status = 1",
        )
        .fetch_one(pool)
        .await?;

        if super_admin_count.0 <= 1 {
            return Err(anyhow::anyhow!(
                "Cannot disable the last active super admin"
            ));
        }
    }

    sqlx::query("UPDATE admins SET status = ?, updated_at = NOW() WHERE id = ?")
        .bind(status)
        .bind(id)
        .execute(pool)
        .await?;

    get_admin_by_id(pool, id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Admin not found after status update"))
}
