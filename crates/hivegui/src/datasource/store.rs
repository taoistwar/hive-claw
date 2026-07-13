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
        let row = sqlx::query(
            "SELECT id, name, host, port, username, encrypted_password, created_at, updated_at FROM data_sources WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.inner.pool)
        .await?;

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

        let result = sqlx::query(
            "INSERT INTO data_sources (name, host, port, username, encrypted_password, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(name)
        .bind(host)
        .bind(port as i64)
        .bind(username)
        .bind(&encrypted)
        .bind(now.to_rfc3339())
        .bind(now.to_rfc3339())
        .execute(&self.inner.pool)
        .await?;

        let id = result.last_insert_rowid();

        self.get(id)
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
            sqlx::query(
                "UPDATE data_sources SET name = ?, host = ?, port = ?, username = ?, encrypted_password = ?, updated_at = ? WHERE id = ?",
            )
            .bind(name)
            .bind(host)
            .bind(port as i64)
            .bind(username)
            .bind(&encrypted)
            .bind(now.to_rfc3339())
            .bind(id)
            .execute(&self.inner.pool)
            .await?;
        } else {
            sqlx::query(
                "UPDATE data_sources SET name = ?, host = ?, port = ?, username = ?, updated_at = ? WHERE id = ?",
            )
            .bind(name)
            .bind(host)
            .bind(port as i64)
            .bind(username)
            .bind(now.to_rfc3339())
            .bind(id)
            .execute(&self.inner.pool)
            .await?;
        }

        self.get(id).await
    }

    pub async fn delete(&self, id: i64) -> Result<bool> {
        let result = sqlx::query("DELETE FROM data_sources WHERE id = ?")
            .bind(id)
            .execute(&self.inner.pool)
            .await?;

        Ok(result.rows_affected() > 0)
    }

    pub fn decrypt_password(&self, encrypted: &[u8]) -> Result<Vec<u8>> {
        self.inner.crypto.decrypt(encrypted)
    }

    pub fn default_db_path() -> PathBuf {
        dirs::data_local_dir()
            .unwrap_or_else(|| dirs::home_dir().unwrap_or_default())
            .join("hivegui")
    }
}
