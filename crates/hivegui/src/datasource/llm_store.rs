//! LLM configuration store — Model, Preset, Provider CRUD.
use anyhow::Result;
use chrono::Utc;
use sqlx::SqlitePool;

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
        self.create_current_tables().await?;

        self.seed_builtins().await?;
        Ok(())
    }

    async fn create_current_tables(&self) -> Result<()> {
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
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_models_preset_id_priority_id ON models (preset_id, priority, id)",
        )
        .execute(&self.pool)
        .await?;
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_models_provider_id ON models (provider_id)")
            .execute(&self.pool)
            .await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_llm_presets_is_default ON llm_presets (is_default)",
        )
        .execute(&self.pool)
        .await?;
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
        let mut tx = self.pool.begin().await?;
        if is_default {
            sqlx::query("UPDATE llm_presets SET is_default=0")
                .execute(&mut *tx)
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
        .execute(&mut *tx)
        .await?;
        let id = result.last_insert_rowid();
        let preset = sqlx::query_as::<_, LlmPreset>("SELECT * FROM llm_presets WHERE id = ?")
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(preset)
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
        let mut tx = self.pool.begin().await?;
        let old_name = sqlx::query_scalar::<_, String>("SELECT name FROM llm_presets WHERE id = ?")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
        let old_name = match old_name {
            Some(old_name) => old_name,
            None => return Ok(false),
        };
        if is_default {
            sqlx::query("UPDATE llm_presets SET is_default=0 WHERE id!=?")
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        if old_name != name {
            sqlx::query("UPDATE agents SET model_preset = ? WHERE model_preset = ?")
                .bind(name)
                .bind(&old_name)
                .execute(&mut *tx)
                .await?;
        }
        let rows_affected = sqlx::query(
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
        .execute(&mut *tx)
        .await?
        .rows_affected()
            > 0;
        tx.commit().await?;
        Ok(rows_affected)
    }

    pub async fn delete_preset(&self, id: i64) -> Result<bool> {
        let preset_name =
            sqlx::query_scalar::<_, String>("SELECT name FROM llm_presets WHERE id = ?")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?;
        let preset_name = match preset_name {
            Some(name) => name,
            None => return Ok(false),
        };
        let references = sqlx::query_as::<_, (String,)>(
            "SELECT identifier FROM agents WHERE model_preset = ? ORDER BY identifier ASC",
        )
        .bind(&preset_name)
        .fetch_all(&self.pool)
        .await?;
        if !references.is_empty() {
            let references = references
                .into_iter()
                .map(|(identifier,)| identifier)
                .collect::<Vec<_>>();
            anyhow::bail!(
                "Conflict {{ field: \"name\", reason: \"referenced_by_agent\", references: {:?} }}",
                references
            );
        }
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
        let encrypted = if token_env.is_empty() {
            if token.is_empty() {
                None
            } else {
                Some(self.crypto.encrypt(token.as_bytes())?)
            }
        } else {
            None
        };
        let stored_env = token_env;
        let result = sqlx::query(
            "INSERT INTO llm_providers
                (name, category, base_url, token_encrypted, token_env, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(name)
        .bind(category)
        .bind(base_url)
        .bind(&encrypted)
        .bind(stored_env)
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
        let result = if token_env.is_empty() {
            if token.is_empty() {
                sqlx::query(
                    "UPDATE llm_providers
                     SET name=?, category=?, base_url=?, updated_at=?
                     WHERE id=?",
                )
                .bind(name)
                .bind(category)
                .bind(base_url)
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
            }
        } else {
            sqlx::query(
                "UPDATE llm_providers
                 SET name=?, category=?, base_url=?, token_encrypted=?, token_env=?, updated_at=?
                 WHERE id=?",
            )
            .bind(name)
            .bind(category)
            .bind(base_url)
            .bind(Option::<Vec<u8>>::None)
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
}
