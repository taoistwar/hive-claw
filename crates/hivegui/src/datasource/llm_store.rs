//! LLM configuration store — Model, Preset, Provider CRUD.
use anyhow::Result;
use chrono::Utc;
use sqlx::{Row, SqlitePool};

use super::crypto::Crypto;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct LlmModel {
    pub id: i64,
    pub name: String,
    pub preset_id: Option<i64>,
    pub provider_id: Option<i64>,
    pub priority: i32,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct LlmPreset {
    pub id: i64,
    pub name: String,
    pub description: String,
    pub is_default: i32,
    pub max_tokens: i32,
    pub temperature: f64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct LlmProvider {
    pub id: i64,
    pub name: String,
    pub category: String,
    pub base_url: String,
    pub token_encrypted: Option<Vec<u8>>,
    pub token_env: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone)]
pub struct LlmStore {
    pool: SqlitePool,
    crypto: Crypto,
}

impl LlmStore {
    pub fn new(pool: SqlitePool, crypto: Crypto) -> Self {
        LlmStore { pool, crypto }
    }

    pub fn crypto(&self) -> &Crypto {
        &self.crypto
    }

    pub async fn migrate(&self) -> Result<()> {
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&self.pool)
            .await?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS llm_presets (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL UNIQUE,
                description TEXT NOT NULL DEFAULT '',
                is_default INTEGER NOT NULL DEFAULT 0,
                max_tokens INTEGER NOT NULL DEFAULT 2048,
                temperature REAL NOT NULL DEFAULT 0.7,
                created_at TEXT NOT NULL DEFAULT '',
                updated_at TEXT NOT NULL DEFAULT ''
            )",
        )
        .execute(&self.pool)
        .await?;

        let models_exist = self.table_exists("models").await?;
        let providers_exist = self.table_exists("llm_providers").await?;
        if providers_exist {
            self.ensure_legacy_provider_columns().await?;
            self.ensure_provider_name_column().await?;
        }
        if !models_exist && !providers_exist {
            self.create_current_tables().await?;
        } else if models_exist
            && providers_exist
            && (self.table_has_column("llm_providers", "preset_id").await?
                || !self.table_has_column("models", "provider_id").await?)
        {
            self.migrate_legacy_relationships().await?;
        } else {
            self.create_current_tables().await?;
        }

        self.seed_builtins().await?;
        Ok(())
    }

    async fn table_exists(&self, table: &str) -> Result<bool> {
        let exists: i64 = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?)",
        )
        .bind(table)
        .fetch_one(&self.pool)
        .await?;
        Ok(exists != 0)
    }

    async fn table_has_column(&self, table: &str, column: &str) -> Result<bool> {
        // `table` is the verified internal table name from the LLM store, not
        // user input, so it is safe to interpolate into the PRAGMA query.
        let rows = sqlx::query(sqlx::AssertSqlSafe(format!("PRAGMA table_info({table})")))
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.iter().any(|row| {
            row.try_get::<String, _>("name")
                .is_ok_and(|name| name == column)
        }))
    }

    async fn create_current_tables(&self) -> Result<()> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS llm_providers (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL UNIQUE,
                category TEXT NOT NULL DEFAULT 'openai',
                base_url TEXT NOT NULL DEFAULT '',
                token_encrypted BLOB,
                token_env TEXT NOT NULL DEFAULT '',
                created_at TEXT NOT NULL DEFAULT '',
                updated_at TEXT NOT NULL DEFAULT ''
            )",
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS models (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL,
                preset_id INTEGER NOT NULL REFERENCES llm_presets(id) ON DELETE CASCADE,
                provider_id INTEGER NOT NULL REFERENCES llm_providers(id) ON DELETE RESTRICT,
                priority INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL DEFAULT '',
                updated_at TEXT NOT NULL DEFAULT ''
            )",
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn ensure_legacy_provider_columns(&self) -> Result<()> {
        for statement in [
            "ALTER TABLE llm_providers ADD COLUMN category TEXT NOT NULL DEFAULT 'openai'",
            "ALTER TABLE llm_providers ADD COLUMN base_url TEXT NOT NULL DEFAULT ''",
            "ALTER TABLE llm_providers ADD COLUMN token_encrypted BLOB",
            "ALTER TABLE llm_providers ADD COLUMN token_env TEXT NOT NULL DEFAULT ''",
        ] {
            let _ = sqlx::query(statement).execute(&self.pool).await;
        }
        if self
            .table_has_column("llm_providers", "api_key_encrypted")
            .await?
        {
            sqlx::query(
                "UPDATE llm_providers
                 SET token_encrypted = api_key_encrypted
                 WHERE token_encrypted IS NULL AND api_key_encrypted IS NOT NULL",
            )
            .execute(&self.pool)
            .await?;
        }
        if self.table_has_column("llm_providers", "kind").await? {
            sqlx::query(
                "UPDATE llm_providers SET category = kind
                 WHERE kind IS NOT NULL AND kind != ''",
            )
            .execute(&self.pool)
            .await?;
        }
        if self
            .table_has_column("llm_providers", "api_key_env")
            .await?
        {
            sqlx::query(
                "UPDATE llm_providers SET token_env = api_key_env
                 WHERE token_env = '' AND api_key_env IS NOT NULL",
            )
            .execute(&self.pool)
            .await?;
        }
        Ok(())
    }

    /// 为已有 llm_providers 表添加 name 列（UNIQUE），并用 category 填充默认值。
    async fn ensure_provider_name_column(&self) -> Result<()> {
        if !self.table_has_column("llm_providers", "name").await? {
            sqlx::query("ALTER TABLE llm_providers ADD COLUMN name TEXT NOT NULL DEFAULT ''")
                .execute(&self.pool)
                .await?;
            // 用 category + id 生成唯一 name，避免冲突
            sqlx::query(
                "UPDATE llm_providers SET name = category || '_' || CAST(id AS TEXT)
                 WHERE name = ''",
            )
            .execute(&self.pool)
            .await?;
        }
        Ok(())
    }

    async fn migrate_legacy_relationships(&self) -> Result<()> {
        tracing::info!("migrating LLM relationships from Provider-owned to Model-owned");

        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "CREATE TABLE llm_providers_v2 (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL UNIQUE,
                category TEXT NOT NULL DEFAULT 'openai',
                base_url TEXT NOT NULL DEFAULT '',
                token_encrypted BLOB,
                token_env TEXT NOT NULL DEFAULT '',
                created_at TEXT NOT NULL DEFAULT '',
                updated_at TEXT NOT NULL DEFAULT ''
            )",
        )
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO llm_providers_v2
                (id, name, category, base_url, token_encrypted, token_env, created_at, updated_at)
             SELECT id, category || '_' || CAST(id AS TEXT), category, base_url, token_encrypted, token_env, created_at, updated_at
             FROM llm_providers",
        )
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            "CREATE TABLE models_v2 (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL,
                preset_id INTEGER REFERENCES llm_presets(id) ON DELETE CASCADE,
                provider_id INTEGER REFERENCES llm_providers_v2(id) ON DELETE RESTRICT,
                priority INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL DEFAULT '',
                updated_at TEXT NOT NULL DEFAULT ''
            )",
        )
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO models_v2
                (id, name, preset_id, provider_id, priority, created_at, updated_at)
             SELECT
                m.id,
                m.name,
                (SELECT p.preset_id FROM llm_providers p
                 WHERE p.model_id = m.id ORDER BY p.priority, p.id LIMIT 1),
                (SELECT p.id FROM llm_providers p
                 WHERE p.model_id = m.id ORDER BY p.priority, p.id LIMIT 1),
                COALESCE((SELECT p.priority FROM llm_providers p
                 WHERE p.model_id = m.id ORDER BY p.priority, p.id LIMIT 1), 0),
                m.created_at,
                m.updated_at
             FROM models m",
        )
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO models_v2
                (name, preset_id, provider_id, priority, created_at, updated_at)
             SELECT m.name, p.preset_id, p.id, p.priority, m.created_at, m.updated_at
             FROM models m
             JOIN llm_providers p ON p.model_id = m.id
             WHERE p.id != (
                 SELECT p2.id FROM llm_providers p2
                 WHERE p2.model_id = m.id ORDER BY p2.priority, p2.id LIMIT 1
             )",
        )
        .execute(&mut *tx)
        .await?;

        sqlx::query("DROP TABLE llm_providers")
            .execute(&mut *tx)
            .await?;
        sqlx::query("DROP TABLE models").execute(&mut *tx).await?;
        sqlx::query("ALTER TABLE llm_providers_v2 RENAME TO llm_providers")
            .execute(&mut *tx)
            .await?;
        sqlx::query("ALTER TABLE models_v2 RENAME TO models")
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// 若 llm_presets 表为空，插入两个内置 preset（cheap-fast / code-expert）。
    async fn seed_builtins(&self) -> Result<()> {
        let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM llm_presets")
            .fetch_one(&self.pool)
            .await?;
        if count.0 > 0 {
            return Ok(());
        }
        tracing::info!("seeding builtin LLM presets");
        self.create_preset(
            "cheap-fast",
            "GPT-4o-mini primary, Claude Haiku fallback",
            true,
            2048,
            0.7,
        )
        .await?;
        self.create_preset(
            "code-expert",
            "Claude Opus primary, GPT-4o fallback",
            false,
            4096,
            0.3,
        )
        .await?;
        Ok(())
    }

    // ── Model ──
    pub async fn create_model(
        &self,
        name: &str,
        preset_id: i64,
        provider_id: i64,
        priority: i32,
    ) -> Result<LlmModel> {
        let now = Utc::now().to_rfc3339();
        let result = sqlx::query(
            "INSERT INTO models
                (name, preset_id, provider_id, priority, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(name)
        .bind(preset_id)
        .bind(provider_id)
        .bind(priority)
        .bind(&now)
        .bind(&now)
        .execute(&self.pool)
        .await?;
        let id = result.last_insert_rowid();
        Ok(
            sqlx::query_as::<_, LlmModel>("SELECT * FROM models WHERE id = ?")
                .bind(id)
                .fetch_one(&self.pool)
                .await?,
        )
    }

    pub async fn list_models(&self) -> Result<Vec<LlmModel>> {
        sqlx::query_as::<_, LlmModel>("SELECT * FROM models ORDER BY preset_id, priority, id")
            .fetch_all(&self.pool)
            .await
            .map_err(Into::into)
    }

    pub async fn update_model(
        &self,
        id: i64,
        name: &str,
        preset_id: i64,
        provider_id: i64,
        priority: i32,
    ) -> Result<bool> {
        let now = Utc::now().to_rfc3339();
        Ok(sqlx::query(
            "UPDATE models
             SET name=?, preset_id=?, provider_id=?, priority=?, updated_at=?
             WHERE id=?",
        )
        .bind(name)
        .bind(preset_id)
        .bind(provider_id)
        .bind(priority)
        .bind(&now)
        .bind(id)
        .execute(&self.pool)
        .await?
        .rows_affected()
            > 0)
    }

    pub async fn delete_model(&self, id: i64) -> Result<bool> {
        Ok(sqlx::query("DELETE FROM models WHERE id=?")
            .bind(id)
            .execute(&self.pool)
            .await?
            .rows_affected()
            > 0)
    }

    // ── Preset ──
    pub async fn create_preset(
        &self,
        name: &str,
        desc: &str,
        is_default: bool,
        max_tokens: i32,
        temp: f64,
    ) -> Result<LlmPreset> {
        let now = Utc::now().to_rfc3339();
        let def: i32 = is_default.into();
        if is_default {
            sqlx::query("UPDATE llm_presets SET is_default=0")
                .execute(&self.pool)
                .await?;
        }
        let result = sqlx::query(
            "INSERT INTO llm_presets
                (name, description, is_default, max_tokens, temperature, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(name)
        .bind(desc)
        .bind(def)
        .bind(max_tokens)
        .bind(temp)
        .bind(&now)
        .bind(&now)
        .execute(&self.pool)
        .await?;
        let id = result.last_insert_rowid();
        Ok(
            sqlx::query_as::<_, LlmPreset>("SELECT * FROM llm_presets WHERE id = ?")
                .bind(id)
                .fetch_one(&self.pool)
                .await?,
        )
    }

    pub async fn list_presets(&self) -> Result<Vec<LlmPreset>> {
        sqlx::query_as::<_, LlmPreset>("SELECT * FROM llm_presets ORDER BY id")
            .fetch_all(&self.pool)
            .await
            .map_err(Into::into)
    }

    pub async fn update_preset(
        &self,
        id: i64,
        name: &str,
        desc: &str,
        is_default: bool,
        max_tokens: i32,
        temp: f64,
    ) -> Result<bool> {
        let now = Utc::now().to_rfc3339();
        let def: i32 = is_default.into();
        if is_default {
            sqlx::query("UPDATE llm_presets SET is_default=0 WHERE id!=?")
                .bind(id)
                .execute(&self.pool)
                .await?;
        }
        Ok(sqlx::query(
            "UPDATE llm_presets
             SET name=?, description=?, is_default=?, max_tokens=?, temperature=?, updated_at=?
             WHERE id=?",
        )
        .bind(name)
        .bind(desc)
        .bind(def)
        .bind(max_tokens)
        .bind(temp)
        .bind(&now)
        .bind(id)
        .execute(&self.pool)
        .await?
        .rows_affected()
            > 0)
    }

    pub async fn delete_preset(&self, id: i64) -> Result<bool> {
        Ok(sqlx::query("DELETE FROM llm_presets WHERE id=?")
            .bind(id)
            .execute(&self.pool)
            .await?
            .rows_affected()
            > 0)
    }

    // ── Provider ──
    pub async fn create_provider(
        &self,
        name: &str,
        category: &str,
        base_url: &str,
        token: &str,
        token_env: &str,
    ) -> Result<LlmProvider> {
        let now = Utc::now().to_rfc3339();
        let encrypted = if token.is_empty() {
            None
        } else {
            Some(self.crypto.encrypt(token.as_bytes())?)
        };
        let result = sqlx::query(
            "INSERT INTO llm_providers
                (name, category, base_url, token_encrypted, token_env, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(name)
        .bind(category)
        .bind(base_url)
        .bind(&encrypted)
        .bind(token_env)
        .bind(&now)
        .bind(&now)
        .execute(&self.pool)
        .await?;
        let id = result.last_insert_rowid();
        Ok(
            sqlx::query_as::<_, LlmProvider>("SELECT * FROM llm_providers WHERE id = ?")
                .bind(id)
                .fetch_one(&self.pool)
                .await?,
        )
    }

    pub async fn list_providers(&self) -> Result<Vec<LlmProvider>> {
        sqlx::query_as::<_, LlmProvider>("SELECT * FROM llm_providers ORDER BY id")
            .fetch_all(&self.pool)
            .await
            .map_err(Into::into)
    }

    pub async fn update_provider(
        &self,
        id: i64,
        name: &str,
        category: &str,
        base_url: &str,
        token: &str,
        token_env: &str,
    ) -> Result<bool> {
        let now = Utc::now().to_rfc3339();
        let result = if token.is_empty() {
            sqlx::query(
                "UPDATE llm_providers
                 SET name=?, category=?, base_url=?, token_env=?, updated_at=?
                 WHERE id=?",
            )
            .bind(name)
            .bind(category)
            .bind(base_url)
            .bind(token_env)
            .bind(&now)
            .bind(id)
            .execute(&self.pool)
            .await?
        } else {
            let encrypted = self.crypto.encrypt(token.as_bytes())?;
            sqlx::query(
                "UPDATE llm_providers
                 SET name=?, category=?, base_url=?, token_encrypted=?, token_env=?, updated_at=?
                 WHERE id=?",
            )
            .bind(name)
            .bind(category)
            .bind(base_url)
            .bind(encrypted)
            .bind(token_env)
            .bind(&now)
            .bind(id)
            .execute(&self.pool)
            .await?
        };
        Ok(result.rows_affected() > 0)
    }

    pub async fn delete_provider(&self, id: i64) -> Result<bool> {
        let refs: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM models WHERE provider_id=?")
            .bind(id)
            .fetch_one(&self.pool)
            .await?;
        if refs.0 > 0 {
            anyhow::bail!("该 Provider 被 {} 个 Model 引用，无法删除", refs.0);
        }
        Ok(sqlx::query("DELETE FROM llm_providers WHERE id=?")
            .bind(id)
            .execute(&self.pool)
            .await?
            .rows_affected()
            > 0)
    }
}

#[cfg(test)]
mod tests {
    use sqlx::sqlite::SqlitePoolOptions;

    use super::{Crypto, LlmStore};

    async fn test_store() -> LlmStore {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("in-memory sqlite pool");
        let store = LlmStore::new(pool, Crypto::new(&[7; 32]));
        store.migrate().await.expect("migrate llm tables");
        store
    }

    #[tokio::test]
    async fn provider_is_independent_and_model_references_preset_and_provider() {
        let store = test_store().await;
        let preset = store.list_presets().await.unwrap().remove(0);
        let provider = store
            .create_provider(
                "deepseek-1",
                "deepseek",
                "https://api.deepseek.com",
                "secret",
                "",
            )
            .await
            .unwrap();
        let model = store
            .create_model("deepseek-chat", preset.id, provider.id, 10)
            .await
            .unwrap();

        assert_eq!(model.preset_id, Some(preset.id));
        assert_eq!(model.provider_id, Some(provider.id));
        assert_eq!(model.priority, 10);
        assert_eq!(store.list_providers().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn referenced_provider_cannot_be_deleted() {
        let store = test_store().await;
        let preset = store.list_presets().await.unwrap().remove(0);
        let provider = store
            .create_provider(
                "openai-1",
                "openai",
                "https://api.openai.com/v1",
                "",
                "OPENAI_API_KEY",
            )
            .await
            .unwrap();
        store
            .create_model("gpt-4.1", preset.id, provider.id, 0)
            .await
            .unwrap();

        let error = store.delete_provider(provider.id).await.unwrap_err();

        assert!(error.to_string().contains("Model 引用"));
    }

    #[tokio::test]
    async fn legacy_provider_relationships_are_moved_to_models() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query("CREATE TABLE models (id INTEGER PRIMARY KEY, name TEXT NOT NULL, created_at TEXT NOT NULL DEFAULT '', updated_at TEXT NOT NULL DEFAULT '')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("CREATE TABLE llm_presets (id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE, description TEXT NOT NULL DEFAULT '', is_default INTEGER NOT NULL DEFAULT 0, max_tokens INTEGER NOT NULL DEFAULT 2048, temperature REAL NOT NULL DEFAULT 0.7, created_at TEXT NOT NULL DEFAULT '', updated_at TEXT NOT NULL DEFAULT '')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("CREATE TABLE llm_providers (id INTEGER PRIMARY KEY, preset_id INTEGER NOT NULL, model_id INTEGER, priority INTEGER NOT NULL DEFAULT 0, category TEXT NOT NULL DEFAULT 'openai', base_url TEXT NOT NULL DEFAULT '', token_encrypted BLOB, token_env TEXT NOT NULL DEFAULT '', created_at TEXT NOT NULL DEFAULT '', updated_at TEXT NOT NULL DEFAULT '')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO llm_presets (id, name) VALUES (1, 'legacy')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO models (id, name) VALUES (2, 'legacy-model')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO llm_providers (id, preset_id, model_id, priority, category) VALUES (3, 1, 2, 7, 'deepseek')")
            .execute(&pool)
            .await
            .unwrap();

        let store = LlmStore::new(pool, Crypto::new(&[9; 32]));
        store.migrate().await.unwrap();

        let model = store.list_models().await.unwrap().remove(0);
        assert_eq!(model.preset_id, Some(1));
        assert_eq!(model.provider_id, Some(3));
        assert_eq!(model.priority, 7);
        assert_eq!(
            store.list_providers().await.unwrap()[0].category,
            "deepseek"
        );
    }
}
