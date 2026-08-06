//! db.query / db.execute capabilities (T098 / US4 commit 4)
//!
//! 安全契约：**只接受预注册的 named query**；自由 SQL 永远拒绝（spec FR + TM-4）。
//! 命名查询从 `DB_NAMED_QUERIES_PATH`（默认 `./named_queries.toml`）启动期加载。
//!
//! 参数绑定：用 `:name` 占位符；运行时按 args 中的 key 顺序替换为 `?` 并 bind。
//! 这保证最终 SQL 仍然走 sqlx parameterized query — 无字符串拼接、无注入面。

use once_cell::sync::OnceCell;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use sqlx::{Column, MySqlPool, Row};
use std::collections::HashMap;

#[derive(Debug, Clone, Deserialize)]
struct NamedQueryRaw {
    name: String,
    sql: String,
    kind: String, // "select" | "execute"
    #[serde(default)]
    params: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct NamedQueriesFile {
    #[serde(default)]
    query: Vec<NamedQueryRaw>,
}

#[derive(Debug, Clone)]
pub struct NamedQuery {
    pub name: String,
    pub sql: String,
    pub kind: String,
    pub params: Vec<String>,
}

#[derive(Debug, Default)]
pub struct NamedQueryRegistry {
    queries: HashMap<String, NamedQuery>,
}

impl NamedQueryRegistry {
    pub fn load() -> Self {
        let path = std::env::var("DB_NAMED_QUERIES_PATH")
            .unwrap_or_else(|_| "./named_queries.toml".to_string());
        if !std::path::Path::new(&path).exists() {
            tracing::warn!(
                error_kind = "named_query_config_missing",
                "named_queries.toml not found; db.query/db.execute will reject all calls"
            );
            return Self::default();
        }
        let content = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(_) => {
                tracing::error!(
                    error_kind = "named_query_config_read_failed",
                    "failed to read named_queries.toml"
                );
                return Self::default();
            }
        };
        let file: NamedQueriesFile = match toml::from_str(&content) {
            Ok(f) => f,
            Err(_) => {
                tracing::error!(
                    error_kind = "named_query_config_parse_failed",
                    "named_queries.toml parse error"
                );
                return Self::default();
            }
        };
        let mut queries = HashMap::with_capacity(file.query.len());
        for raw in file.query {
            queries.insert(
                raw.name.clone(),
                NamedQuery {
                    name: raw.name,
                    sql: raw.sql,
                    kind: raw.kind,
                    params: raw.params,
                },
            );
        }
        tracing::info!(count = queries.len(), "named queries loaded");
        Self { queries }
    }

    pub fn lookup(&self, name: &str) -> Option<&NamedQuery> {
        self.queries.get(name)
    }
}

static REGISTRY: OnceCell<NamedQueryRegistry> = OnceCell::new();

pub fn registry() -> &'static NamedQueryRegistry {
    REGISTRY.get_or_init(NamedQueryRegistry::load)
}

#[derive(Debug, Deserialize)]
pub struct DbCallArgs {
    pub query: String,
    #[serde(default)]
    pub params: Map<String, Value>,
}

/// `:name` → `?` substitution. Returns (final_sql, ordered_values).
fn bind_params(q: &NamedQuery, args: &Map<String, Value>) -> Result<(String, Vec<Value>), String> {
    let mut sql = q.sql.clone();
    let mut ordered: Vec<Value> = Vec::with_capacity(q.params.len());
    for p in &q.params {
        let placeholder = format!(":{p}");
        if !sql.contains(&placeholder) {
            return Err(format!("query 定义中找不到占位符 {placeholder}"));
        }
        sql = sql.replace(&placeholder, "?");
        let v = args
            .get(p)
            .cloned()
            .ok_or_else(|| format!("missing param: {p}"))?;
        ordered.push(v);
    }
    Ok((sql, ordered))
}

fn bind_value<'q>(
    mut q: sqlx::query::Query<'q, sqlx::MySql, sqlx::mysql::MySqlArguments>,
    v: &'q Value,
) -> sqlx::query::Query<'q, sqlx::MySql, sqlx::mysql::MySqlArguments> {
    match v {
        Value::Null => q = q.bind(Option::<String>::None),
        Value::Bool(b) => q = q.bind(*b),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                q = q.bind(i);
            } else if let Some(f) = n.as_f64() {
                q = q.bind(f);
            }
        }
        Value::String(s) => q = q.bind(s.as_str()),
        other => q = q.bind(other.to_string()),
    }
    q
}

pub async fn db_query(pool: &MySqlPool, args: DbCallArgs) -> Result<Value, String> {
    let reg = registry();
    let q = reg
        .lookup(&args.query)
        .ok_or_else(|| format!("未注册的 named query: {}", args.query))?;
    if q.kind != "select" {
        return Err(format!(
            "query {} 不是 select 类型；db.query 仅允许 select",
            q.name
        ));
    }
    let (sql, ordered) = bind_params(q, &args.params)?;

    let mut built = sqlx::query(&sql);
    for v in &ordered {
        built = bind_value(built, v);
    }
    let rows = built
        .fetch_all(pool)
        .await
        .map_err(|e| format!("db.query exec: {e}"))?;

    // 把 MySqlRow → Vec<Map<String, Value>>
    let mut out: Vec<Value> = Vec::with_capacity(rows.len());
    for row in &rows {
        let mut obj = Map::new();
        for (idx, col) in row.columns().iter().enumerate() {
            let name = col.name().to_string();
            let v: Value = try_get_col(row, idx).unwrap_or(Value::Null);
            obj.insert(name, v);
        }
        out.push(Value::Object(obj));
    }
    Ok(json!({ "rows": out, "row_count": out.len() }))
}

fn try_get_col(row: &sqlx::mysql::MySqlRow, idx: usize) -> Option<Value> {
    // 尝试常见类型
    if let Ok(v) = row.try_get::<Option<i64>, _>(idx) {
        return Some(v.map(|i| json!(i)).unwrap_or(Value::Null));
    }
    if let Ok(v) = row.try_get::<Option<f64>, _>(idx) {
        return Some(v.map(|f| json!(f)).unwrap_or(Value::Null));
    }
    if let Ok(v) = row.try_get::<Option<String>, _>(idx) {
        return Some(v.map(Value::String).unwrap_or(Value::Null));
    }
    if let Ok(v) = row.try_get::<Option<bool>, _>(idx) {
        return Some(v.map(Value::Bool).unwrap_or(Value::Null));
    }
    // fallback：未识别类型，记日志
    tracing::debug!(
        error_kind = "unsupported_column_type",
        "db.query unknown column type — returning null"
    );
    Some(Value::Null)
}

pub async fn db_execute(pool: &MySqlPool, args: DbCallArgs) -> Result<Value, String> {
    let reg = registry();
    let q = reg
        .lookup(&args.query)
        .ok_or_else(|| format!("未注册的 named query: {}", args.query))?;
    if q.kind != "execute" {
        return Err(format!(
            "query {} 不是 execute 类型；db.execute 仅允许 DML",
            q.name
        ));
    }
    let (sql, ordered) = bind_params(q, &args.params)?;
    let mut built = sqlx::query(&sql);
    for v in &ordered {
        built = bind_value(built, v);
    }
    let res = built
        .execute(pool)
        .await
        .map_err(|e| format!("db.execute: {e}"))?;
    Ok(json!({
        "rows_affected": res.rows_affected(),
        "last_insert_id": res.last_insert_id(),
    }))
}
