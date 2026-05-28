use anyhow::Result;
use sqlx::{MySqlPool, Row};

use crate::models::LoginRecord;

pub struct LoginRecordFilter {
    pub admin_id: Option<i64>,
    pub search: Option<String>,
    pub success: Option<bool>,
    pub failure_reason: Option<String>,
    pub ip_address: Option<String>,
    pub login_at_start: Option<String>,
    pub login_at_end: Option<String>,
}

impl LoginRecordFilter {
    pub fn build_where(&self) -> (String, Vec<String>) {
        let mut conditions = Vec::new();
        let mut params = Vec::new();

        if let Some(id) = self.admin_id {
            conditions.push("admin_id = ?");
            params.push(id.to_string());
        }

        if let Some(ref s) = self.search {
            conditions.push("(admin_phone_snapshot LIKE ? OR admin_nickname_snapshot LIKE ? OR ip_address LIKE ?)");
            let p = format!("%{}%", s);
            params.push(p.clone());
            params.push(p.clone());
            params.push(p);
        }

        if let Some(success) = self.success {
            conditions.push("success = ?");
            params.push(if success { "1".to_string() } else { "0".to_string() });
        }

        if let Some(ref reason) = self.failure_reason {
            conditions.push("failure_reason = ?");
            params.push(reason.clone());
        }

        if let Some(ref ip) = self.ip_address {
            conditions.push("ip_address = ?");
            params.push(ip.clone());
        }

        if let Some(ref start) = self.login_at_start {
            conditions.push("login_at >= ?");
            params.push(start.clone());
        }

        if let Some(ref end) = self.login_at_end {
            conditions.push("login_at < ?");
            params.push(end.clone());
        }

        if conditions.is_empty() {
            (String::new(), params)
        } else {
            (format!(" WHERE {}", conditions.join(" AND ")), params)
        }
    }
}

pub async fn list_login_records(
    pool: &MySqlPool,
    offset: u32,
    limit: u32,
    filter: &LoginRecordFilter,
) -> Result<(Vec<LoginRecord>, u64)> {
    let (where_clause, params) = filter.build_where();

    let list_sql = format!(
        "SELECT * FROM login_records{} ORDER BY login_at DESC LIMIT ? OFFSET ?",
        where_clause
    );

    let count_sql = format!("SELECT COUNT(*) FROM login_records{}", where_clause);

    let records = build_record_list_query(&list_sql, params.clone(), pool, offset, limit).await?;

    let total = build_count_query(&count_sql, params, pool).await?;

    Ok((records, total))
}

async fn build_record_list_query(
    sql: &str,
    params: Vec<String>,
    pool: &MySqlPool,
    offset: u32,
    limit: u32,
) -> Result<Vec<LoginRecord>> {
    let mut query = sqlx::query(sql);
    for p in params {
        query = query.bind(p);
    }
    let query = query.bind(limit as i64).bind(offset as i64);
    let rows = query.fetch_all(pool).await?;
    let records: Vec<LoginRecord> = rows
        .into_iter()
        .map(|row| sqlx::FromRow::from_row(&row).unwrap())
        .collect();
    Ok(records)
}

async fn build_count_query(
    sql: &str,
    params: Vec<String>,
    pool: &MySqlPool,
) -> Result<u64> {
    let mut query = sqlx::query(sql);
    for p in params {
        query = query.bind(p);
    }
    let row = query.fetch_one(pool).await?;
    let count: i64 = row.get(0);
    Ok(count as u64)
}

pub async fn get_login_record_by_id(pool: &MySqlPool, id: i64) -> Result<Option<LoginRecord>> {
    let record = sqlx::query_as::<_, LoginRecord>(
        "SELECT * FROM login_records WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;

    Ok(record)
}
