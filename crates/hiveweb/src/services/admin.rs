use anyhow::Result;
use sqlx::{MySqlPool, Row};

use crate::models::{Admin, Role};

pub struct AdminFilter {
    pub search: Option<String>,
    pub status: Option<i8>,
    pub role: Option<i8>,
    pub created_at_start: Option<String>,
    pub created_at_end: Option<String>,
    pub last_login_start: Option<String>,
    pub last_login_end: Option<String>,
}

impl AdminFilter {
    pub fn is_empty(&self) -> bool {
        self.search.is_none()
            && self.status.is_none()
            && self.role.is_none()
            && self.created_at_start.is_none()
            && self.created_at_end.is_none()
            && self.last_login_start.is_none()
            && self.last_login_end.is_none()
    }

    pub fn build_where(&self) -> String {
        let mut conditions = Vec::new();

        if self.search.is_some() {
            conditions.push("(phone LIKE ? OR nickname LIKE ? OR CAST(id AS CHAR) LIKE ?)");
        }
        if self.status.is_some() {
            conditions.push("status = ?");
        }
        if self.role.is_some() {
            conditions.push("role = ?");
        }
        if self.created_at_start.is_some() {
            conditions.push("created_at >= ?");
        }
        if self.created_at_end.is_some() {
            conditions.push("created_at < ?");
        }
        if self.last_login_start.is_some() {
            conditions.push("last_login_at >= ?");
        }
        if self.last_login_end.is_some() {
            conditions.push("last_login_at < ?");
        }

        if conditions.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", conditions.join(" AND "))
        }
    }
}

pub async fn find_admin_by_phone(pool: &MySqlPool, phone: &str) -> Result<Option<Admin>> {
    let admin = sqlx::query_as::<_, Admin>("SELECT * FROM admins WHERE phone = ?")
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
    filter: &AdminFilter,
) -> Result<(Vec<Admin>, u64)> {
    let where_clause = filter.build_where();

    // Build and execute the list query
    let list_sql = format!(
        "SELECT * FROM admins{} ORDER BY created_at DESC LIMIT ? OFFSET ?",
        where_clause
    );

    let count_sql = format!("SELECT COUNT(*) FROM admins{}", where_clause);

    // Use sqlx::query with manual row mapping to avoid type complexity
    let admins = build_admin_list_query(&list_sql, filter, pool, offset, limit).await?;

    let total = build_count_query(&count_sql, filter, pool).await?;

    Ok((admins, total))
}

async fn build_admin_list_query(
    sql: &str,
    filter: &AdminFilter,
    pool: &MySqlPool,
    offset: u32,
    limit: u32,
) -> Result<Vec<Admin>> {
    let mut query = sqlx::query(sql);
    query = bind_params(query, filter);
    let query = query.bind(limit as i64).bind(offset as i64);
    let rows = query.fetch_all(pool).await?;
    let admins: Vec<Admin> = rows
        .into_iter()
        .map(|row| sqlx::FromRow::from_row(&row).unwrap())
        .collect();
    Ok(admins)
}

async fn build_count_query(sql: &str, filter: &AdminFilter, pool: &MySqlPool) -> Result<u64> {
    let mut query = sqlx::query(sql);
    query = bind_params(query, filter);
    let row = query.fetch_one(pool).await?;
    let count: i64 = row.get(0);
    Ok(count as u64)
}

fn bind_params<'a>(
    mut q: sqlx::query::Query<'a, sqlx::MySql, sqlx::mysql::MySqlArguments>,
    filter: &'a AdminFilter,
) -> sqlx::query::Query<'a, sqlx::MySql, sqlx::mysql::MySqlArguments> {
    if let Some(ref s) = filter.search {
        let p = format!("%{}%", s);
        q = q.bind(p.clone()).bind(p.clone()).bind(p);
    }
    if let Some(s) = filter.status {
        q = q.bind(s);
    }
    if let Some(r) = filter.role {
        q = q.bind(r);
    }
    if let Some(ref start) = filter.created_at_start {
        q = q.bind(start);
    }
    if let Some(ref end) = filter.created_at_end {
        q = q.bind(end);
    }
    if let Some(ref start) = filter.last_login_start {
        q = q.bind(start);
    }
    if let Some(ref end) = filter.last_login_end {
        q = q.bind(end);
    }
    q
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

pub async fn update_admin(pool: &MySqlPool, id: i64, nickname: &str, role: i8) -> Result<Admin> {
    let result =
        sqlx::query("UPDATE admins SET nickname = ?, role = ?, updated_at = NOW() WHERE id = ?")
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
    let admin = get_admin_by_id(pool, id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Admin not found"))?;

    let role = Role::try_from(admin.role).map_err(|_| anyhow::anyhow!("Invalid role"))?;

    if role == Role::Super {
        return Err(anyhow::anyhow!("Cannot delete super admin"));
    }

    sqlx::query("DELETE FROM admins WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;

    Ok(())
}

pub async fn toggle_admin_status(pool: &MySqlPool, id: i64, status: i8) -> Result<Admin> {
    let admin = get_admin_by_id(pool, id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Admin not found"))?;

    let role = Role::try_from(admin.role).map_err(|_| anyhow::anyhow!("Invalid role"))?;

    if role == Role::Super && status == 0 {
        let super_admin_count: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM admins WHERE role = 3 AND status = 1")
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

pub async fn update_admin_password(
    pool: &MySqlPool,
    id: i64,
    new_password_hash: &str,
) -> Result<()> {
    let result =
        sqlx::query("UPDATE admins SET password_hash = ?, updated_at = NOW() WHERE id = ?")
            .bind(new_password_hash)
            .bind(id)
            .execute(pool)
            .await?;

    if result.rows_affected() == 0 {
        return Err(anyhow::anyhow!("Admin not found"));
    }

    Ok(())
}
