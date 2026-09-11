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
    /// Reasoning effort forwarded to the provider request when the tier
    /// enables extended thinking (`low` / `medium` / `high` / `adaptive`).
    /// `None` keeps the provider default (no extended thinking).
    pub reasoning_effort: Option<String>,
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

/// 内置档位的规范定义：名称、描述、是否默认、`max_tokens`、
/// `temperature` 与 `reasoning_effort`。
///
/// 档位只表达「意图 / 成本偏好」——具体用哪个模型由用户把某个 Provider
/// 下的 Model 挂到该档位（同档内按 priority 形成 fallback 链）决定。
struct BuiltinTier {
    name: &'static str,
    description: &'static str,
    is_default: bool,
    max_tokens: i32,
    temperature: f64,
    reasoning_effort: Option<&'static str>,
}

/// 三个内置档位：快速 / 均衡 / 极致。
///
/// * `fast` — 低延迟低成本，适合简单问答与草稿；不请求扩展思考。
/// * `balanced` — 默认档，日常任务的延迟与质量折中；不请求扩展思考。
/// * `max` — 最强质量与深度推理，延迟与成本最高；默认请求
///   `reasoning_effort = high`（需要绑定的模型支持，用户可在界面改）。
const BUILTIN_TIERS: [BuiltinTier; 3] = [
    BuiltinTier {
        name: "fast",
        description: "快速：低延迟、低成本，适合简单问答与草稿",
        is_default: false,
        max_tokens: 2048,
        temperature: 0.7,
        reasoning_effort: None,
    },
    BuiltinTier {
        name: "balanced",
        description: "均衡：默认档，日常任务的延迟与质量折中",
        is_default: true,
        max_tokens: 8192,
        temperature: 0.3,
        reasoning_effort: None,
    },
    BuiltinTier {
        name: "max",
        description: "极致：最强质量与深度推理，延迟与成本最高（需模型支持 reasoning_effort）",
        is_default: false,
        max_tokens: 16384,
        temperature: 0.3,
        reasoning_effort: Some("high"),
    },
];

/// Look up a builtin tier definition by canonical name.
fn builtin_tier(name: &str) -> Option<&'static BuiltinTier> {
    BUILTIN_TIERS.iter().find(|tier| tier.name == name)
}

/// 归一 `reasoning_effort` 的存储形式：去空白 + 小写，空串等同于 `None`。
///
/// `None` 表示不向 provider 请求扩展思考（保持 provider 默认行为）；
/// 具体支持的值由 provider 适配层决定（`low` / `medium` / `high` /
/// `adaptive` / `none`）。
fn normalize_reasoning_effort(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase)
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
        // 兼容旧库：`reasoning_effort` 之前的版本写出的 llm_presets 没有该列，
        // 而 `LlmPreset` 解码走 `SELECT *`，缺列会直接报
        // "no column found for name: reasoning_effort"。
        self.ensure_presets_reasoning_effort().await?;
        self.seed_builtins().await?;
        // 老版本写出的内置档位（cheap-fast / code-expert）归一到三档命名。
        self.normalize_builtin_presets().await?;
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
                reasoning_effort TEXT,
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

    /// 若 llm_presets 表为空，插入三个内置档位（fast / balanced / max）。
    async fn seed_builtins(&self) -> Result<()> {
        let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM llm_presets")
            .fetch_one(&self.pool)
            .await?;
        if count.0 > 0 {
            return Ok(());
        }
        tracing::info!("seeding builtin LLM presets");
        for tier in BUILTIN_TIERS {
            self.create_preset(
                tier.name,
                tier.description,
                tier.is_default,
                tier.max_tokens,
                tier.temperature,
                tier.reasoning_effort,
            )
            .await?;
        }
        Ok(())
    }

    /// 确保 `llm_presets` 含 `reasoning_effort` 列。
    ///
    /// 该列之前的版本写出的库已被标记为 v4，`migrate_to_current` 对这类库
    /// 判定 `Unchanged` 并提前返回，因此补列必须发生在 LLM 表初始化路径上；
    /// 否则 `SELECT *` 解码 `LlmPreset` 会报
    /// "no column found for name: reasoning_effort"。
    /// 对新建库（CREATE TABLE 已含该列）是空操作。
    async fn ensure_presets_reasoning_effort(&self) -> Result<()> {
        let exists = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM pragma_table_info('llm_presets') WHERE name = 'reasoning_effort'",
        )
        .fetch_one(&self.pool)
        .await
        .unwrap_or(0)
            > 0;
        if !exists {
            sqlx::query("ALTER TABLE llm_presets ADD COLUMN reasoning_effort TEXT")
                .execute(&self.pool)
                .await?;
        }
        Ok(())
    }

    /// 把老版本写出的两个内置档位（`cheap-fast` / `code-expert`）归一到
    /// `fast` / `balanced` / `max` 三档。
    ///
    /// 保守且幂等：
    ///   * 仅在老内置名仍然存在时执行；用户已经把内置档位删掉的话不会再被复活。
    ///   * 目标名已被占用时**不合并、不改名**（保留原记录，绝不引入别名）。
    ///   * 只做重命名、改描述、补 `balanced` 与 `max.reasoning_effort`；
    ///     **不改**已有档位的 `max_tokens` / `temperature`（那是用户可能已调过的值）。
    ///   * 重命名时在同一事务内更新全部 `agents.model_preset`
    ///     （`contracts/llm-provider.md` 要求原子更新）。
    async fn normalize_builtin_presets(&self) -> Result<()> {
        const RENAMES: [(&str, &str); 2] = [("cheap-fast", "fast"), ("code-expert", "max")];
        let mut tx = self.pool.begin().await?;
        let mut renamed = false;
        for (legacy, canonical) in RENAMES {
            let legacy_exists =
                sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM llm_presets WHERE name = ?")
                    .bind(legacy)
                    .fetch_one(&mut *tx)
                    .await?
                    > 0;
            if !legacy_exists {
                continue;
            }
            let target_taken =
                sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM llm_presets WHERE name = ?")
                    .bind(canonical)
                    .fetch_one(&mut *tx)
                    .await?
                    > 0;
            if target_taken {
                tracing::warn!(
                    legacy,
                    canonical,
                    "builtin preset rename skipped: target name already taken"
                );
                continue;
            }
            let now = Utc::now().to_rfc3339();
            let description = builtin_tier(canonical)
                .map(|tier| tier.description)
                .unwrap_or("");
            sqlx::query(
                "UPDATE llm_presets SET name = ?, description = ?, updated_at = ? WHERE name = ?",
            )
            .bind(canonical)
            .bind(description)
            .bind(&now)
            .bind(legacy)
            .execute(&mut *tx)
            .await?;
            sqlx::query("UPDATE agents SET model_preset = ? WHERE model_preset = ?")
                .bind(canonical)
                .bind(legacy)
                .execute(&mut *tx)
                .await?;
            renamed = true;
        }
        if renamed && let Some(tier) = builtin_tier("max") {
            sqlx::query(
                "UPDATE llm_presets SET reasoning_effort = ? \
                 WHERE name = 'max' AND reasoning_effort IS NULL",
            )
            .bind(tier.reasoning_effort)
            .execute(&mut *tx)
            .await?;
        }
        let has_balanced = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM llm_presets WHERE name = 'balanced'",
        )
        .fetch_one(&mut *tx)
        .await?
            > 0;
        if renamed && !has_balanced {
            let balanced = builtin_tier("balanced").expect("balanced tier is defined");
            let current_default = sqlx::query_scalar::<_, String>(
                "SELECT name FROM llm_presets WHERE is_default = 1 ORDER BY id LIMIT 1",
            )
            .fetch_optional(&mut *tx)
            .await?;
            // 只有默认档正好是被重命名过来的 `fast`（或压根没有默认档）时，
            // 才让新的 `balanced` 接管默认；用户自选的默认档位保持不动。
            let make_default = matches!(current_default.as_deref(), None | Some("fast"));
            let def: i32 = make_default.into();
            if make_default {
                sqlx::query("UPDATE llm_presets SET is_default = 0")
                    .execute(&mut *tx)
                    .await?;
            }
            let now = Utc::now().to_rfc3339();
            sqlx::query(
                "INSERT INTO llm_presets
                    (name, description, is_default, max_tokens, temperature, reasoning_effort, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(balanced.name)
            .bind(balanced.description)
            .bind(def)
            .bind(balanced.max_tokens)
            .bind(balanced.temperature)
            .bind(balanced.reasoning_effort)
            .bind(&now)
            .bind(&now)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
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

    /// Resolve the default model name: the lowest-priority model under the
    /// single default preset (`is_default = 1`). Returns `None` when no
    /// default preset exists or the default preset has no models. This is
    /// the fallback used when a caller (e.g. `generate_answer_node`) does not
    /// specify an explicit model.
    pub async fn default_model_name(&self) -> Result<Option<String>> {
        let name: Option<String> = sqlx::query_scalar(
            "SELECT m.name FROM models m \
             JOIN llm_presets p ON p.id = m.preset_id \
             WHERE p.is_default = 1 \
             ORDER BY m.priority, m.id \
             LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(name)
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
        reasoning_effort: Option<&str>,
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
                (name, description, is_default, max_tokens, temperature, reasoning_effort, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(name)
        .bind(desc)
        .bind(def)
        .bind(max_tokens)
        .bind(temp)
        .bind(normalize_reasoning_effort(reasoning_effort))
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

    /// Resolve the unique default Preset name for runtime provider selection.
    pub async fn default_preset_name(&self) -> Result<Option<String>> {
        sqlx::query_scalar("SELECT name FROM llm_presets WHERE is_default = 1 ORDER BY id LIMIT 1")
            .fetch_optional(&self.pool)
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
        reasoning_effort: Option<&str>,
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
             SET name=?, description=?, is_default=?, max_tokens=?, temperature=?, reasoning_effort=?, updated_at=?
             WHERE id=?",
        )
        .bind(name)
        .bind(desc)
        .bind(def)
        .bind(max_tokens)
        .bind(temp)
        .bind(normalize_reasoning_effort(reasoning_effort))
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
