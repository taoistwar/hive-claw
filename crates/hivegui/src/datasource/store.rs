use anyhow::Result;
use chrono::{DateTime, Utc};
use sqlx::{
    Pool, Row, Sqlite,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;

use super::crypto::Crypto;
use super::models::DataSource;

const DB_FILENAME: &str = "datasources.db";
const KEY_SIZE: usize = 32;

struct StoreInner {
    pool: Pool<Sqlite>,
    crypto: Crypto,
}

#[derive(Clone)]
pub struct Store {
    inner: Arc<StoreInner>,
}

impl Store {
    pub async fn new(db_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(db_dir)?;

        let db_path = db_dir.join(DB_FILENAME);
        let conn_opts =
            SqliteConnectOptions::from_str(&db_path.to_string_lossy())?.create_if_missing(true);

        let pool = SqlitePoolOptions::new().connect_with(conn_opts).await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS data_sources (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL UNIQUE,
                host TEXT NOT NULL,
                port INTEGER NOT NULL DEFAULT 3306,
                username TEXT NOT NULL,
                encrypted_password BLOB NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&pool)
        .await?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_data_sources_name ON data_sources(name)")
            .execute(&pool)
            .await?;

        // global_configs table
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS global_configs (\
             id INTEGER PRIMARY KEY AUTOINCREMENT, \
             name TEXT NOT NULL, key TEXT NOT NULL UNIQUE, \
             type TEXT NOT NULL DEFAULT 'text', data TEXT NOT NULL, \
             created_at TEXT NOT NULL DEFAULT '', updated_at TEXT NOT NULL DEFAULT '')",
        )
        .execute(&pool)
        .await?;

        // FR-027: 运行数据库迁移（包括初始化实体表）
        super::entity_store::run_migrations(&pool).await?;
        super::entity_store::register_runtime_capabilities(&pool).await?;

        // 执行数据库完整性检查
        let integrity_result = sqlx::query_scalar::<_, String>("PRAGMA integrity_check")
            .fetch_one(&pool)
            .await;

        match integrity_result {
            Ok(result) if result == "ok" => {
                // 数据库完整性正常
            }
            Ok(result) => {
                tracing::error!("数据库完整性检查失败: {}", result);
                // 这里可以添加恢复逻辑，但目前仅记录错误
            }
            Err(e) => {
                tracing::error!("执行完整性检查时出错: {}", e);
            }
        }

        let key = Self::load_or_generate_key(db_dir)?;
        let crypto = Crypto::new(&key);

        Ok(Self {
            inner: Arc::new(StoreInner { pool, crypto }),
        })
    }

    fn load_or_generate_key(db_dir: &Path) -> Result<[u8; KEY_SIZE]> {
        let key_path = db_dir.join("encryption.key");
        if key_path.exists() {
            let data = std::fs::read(&key_path)?;
            if data.len() != KEY_SIZE {
                anyhow::bail!("Invalid key file size");
            }
            let mut key = [0u8; KEY_SIZE];
            key.copy_from_slice(&data);
            Ok(key)
        } else {
            let key = Crypto::generate_key();
            std::fs::write(&key_path, &key)?;
            Ok(key)
        }
    }

    pub async fn list(&self) -> Result<Vec<DataSource>> {
        let rows = sqlx::query(
            "SELECT id, name, host, port, username, encrypted_password, created_at, updated_at FROM data_sources ORDER BY name",
        )
        .fetch_all(&self.inner.pool)
        .await?;

        let mut sources = Vec::new();
        for row in rows {
            let created_at = row
                .try_get::<String, _>("created_at")?
                .parse::<DateTime<Utc>>()
                .unwrap_or_else(|_| Utc::now());
            let updated_at = row
                .try_get::<String, _>("updated_at")?
                .parse::<DateTime<Utc>>()
                .unwrap_or_else(|_| Utc::now());
            sources.push(DataSource {
                id: row.try_get("id")?,
                name: row.try_get("name")?,
                host: row.try_get("host")?,
                port: row.try_get("port")?,
                username: row.try_get("username")?,
                encrypted_password: row.try_get("encrypted_password")?,
                created_at,
                updated_at,
            });
        }
        Ok(sources)
    }

    pub async fn get(&self, id: i64) -> Result<Option<DataSource>> {
        let row = sqlx::query("SELECT id, name, host, port, username, encrypted_password, created_at, updated_at FROM data_sources WHERE id = ?")
            .bind(id).fetch_optional(&self.inner.pool).await?;
        match row {
            Some(row) => {
                let created_at = row
                    .try_get::<String, _>("created_at")?
                    .parse::<DateTime<Utc>>()
                    .unwrap_or_else(|_| Utc::now());
                let updated_at = row
                    .try_get::<String, _>("updated_at")?
                    .parse::<DateTime<Utc>>()
                    .unwrap_or_else(|_| Utc::now());
                Ok(Some(DataSource {
                    id: row.try_get("id")?,
                    name: row.try_get("name")?,
                    host: row.try_get("host")?,
                    port: row.try_get("port")?,
                    username: row.try_get("username")?,
                    encrypted_password: row.try_get("encrypted_password")?,
                    created_at,
                    updated_at,
                }))
            }
            None => Ok(None),
        }
    }

    pub async fn create(
        &self,
        name: &str,
        host: &str,
        port: u16,
        username: &str,
        password: &[u8],
    ) -> Result<DataSource> {
        let encrypted = self.inner.crypto.encrypt(password)?;
        let now = Utc::now();
        let result = sqlx::query("INSERT INTO data_sources (name, host, port, username, encrypted_password, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?)")
            .bind(name).bind(host).bind(port as i64).bind(username).bind(&encrypted).bind(now.to_rfc3339()).bind(now.to_rfc3339()).execute(&self.inner.pool).await?;
        self.get(result.last_insert_rowid())
            .await?
            .ok_or_else(|| anyhow::anyhow!("Failed to retrieve newly created data source"))
    }

    pub async fn update(
        &self,
        id: i64,
        name: &str,
        host: &str,
        port: u16,
        username: &str,
        password: Option<&[u8]>,
    ) -> Result<Option<DataSource>> {
        if self.get(id).await?.is_none() {
            return Ok(None);
        }
        let now = Utc::now();
        if let Some(pwd) = password {
            let encrypted = self.inner.crypto.encrypt(pwd)?;
            sqlx::query("UPDATE data_sources SET name = ?, host = ?, port = ?, username = ?, encrypted_password = ?, updated_at = ? WHERE id = ?")
                .bind(name).bind(host).bind(port as i64).bind(username).bind(&encrypted).bind(now.to_rfc3339()).bind(id).execute(&self.inner.pool).await?;
        } else {
            sqlx::query("UPDATE data_sources SET name = ?, host = ?, port = ?, username = ?, updated_at = ? WHERE id = ?")
                .bind(name).bind(host).bind(port as i64).bind(username).bind(now.to_rfc3339()).bind(id).execute(&self.inner.pool).await?;
        }
        self.get(id).await
    }

    pub async fn delete(&self, id: i64) -> Result<bool> {
        Ok(sqlx::query("DELETE FROM data_sources WHERE id = ?")
            .bind(id)
            .execute(&self.inner.pool)
            .await?
            .rows_affected()
            > 0)
    }

    pub fn decrypt_password(&self, encrypted: &[u8]) -> Result<Vec<u8>> {
        self.inner.crypto.decrypt(encrypted)
    }

    pub fn default_db_path() -> PathBuf {
        dirs::data_local_dir()
            .unwrap_or_else(|| dirs::home_dir().unwrap_or_default())
            .join("hivegui")
    }

    pub fn pool(&self) -> &Pool<Sqlite> {
        &self.inner.pool
    }
    pub fn crypto(&self) -> &Crypto {
        &self.inner.crypto
    }
}

// ── GlobalConfig CRUD ──
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct GlobalConfig {
    pub id: i64,
    pub name: String,
    pub key: String,
    #[sqlx(rename = "type")]
    pub config_type: String,
    pub data: String,
    pub created_at: String,
    pub updated_at: String,
}

impl Store {
    pub async fn create_global_config(
        &self,
        name: &str,
        key: &str,
        config_type: &str,
        data: &str,
    ) -> Result<GlobalConfig> {
        let now = Utc::now().to_rfc3339();
        sqlx::query("INSERT INTO global_configs (name, key, type, data, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?)")
            .bind(name).bind(key).bind(config_type).bind(data).bind(&now).bind(&now).execute(&self.inner.pool).await?;
        Ok(sqlx::query_as::<_, GlobalConfig>(
            "SELECT * FROM global_configs WHERE id = last_insert_rowid()",
        )
        .fetch_one(&self.inner.pool)
        .await?)
    }
    pub async fn list_global_configs(
        &self,
        search: &str,
        page: i64,
        page_size: i64,
    ) -> Result<(Vec<GlobalConfig>, i64)> {
        let pattern = format!("%{}%", search);
        let total: (i64,) = if search.is_empty() {
            sqlx::query_as("SELECT COUNT(*) FROM global_configs")
                .fetch_one(&self.inner.pool)
                .await?
        } else {
            sqlx::query_as("SELECT COUNT(*) FROM global_configs WHERE name LIKE ? OR key LIKE ?")
                .bind(&pattern)
                .bind(&pattern)
                .fetch_one(&self.inner.pool)
                .await?
        };
        let rows = if search.is_empty() {
            sqlx::query_as("SELECT * FROM global_configs ORDER BY id LIMIT ? OFFSET ?")
                .bind(page_size)
                .bind((page - 1) * page_size)
                .fetch_all(&self.inner.pool)
                .await?
        } else {
            sqlx::query_as("SELECT * FROM global_configs WHERE name LIKE ? OR key LIKE ? ORDER BY id LIMIT ? OFFSET ?").bind(&pattern).bind(&pattern).bind(page_size).bind((page - 1) * page_size).fetch_all(&self.inner.pool).await?
        };
        Ok((rows, total.0))
    }
    pub async fn update_global_config(
        &self,
        id: i64,
        name: &str,
        key: &str,
        config_type: &str,
        data: &str,
    ) -> Result<bool> {
        let now = Utc::now().to_rfc3339();
        Ok(sqlx::query(
            "UPDATE global_configs SET name=?, key=?, type=?, data=?, updated_at=? WHERE id=?",
        )
        .bind(name)
        .bind(key)
        .bind(config_type)
        .bind(data)
        .bind(&now)
        .bind(id)
        .execute(&self.inner.pool)
        .await?
        .rows_affected()
            > 0)
    }
    pub async fn delete_global_config(&self, id: i64) -> Result<bool> {
        Ok(sqlx::query("DELETE FROM global_configs WHERE id=?")
            .bind(id)
            .execute(&self.inner.pool)
            .await?
            .rows_affected()
            > 0)
    }
}
