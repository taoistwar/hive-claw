//! T047 [P] [US4] LLM provider store contract + T016F
//! `LlmProviderToken` canary activation.
//!
//! Source of truth: `specs/011-hivegui-standalone-mode/tasks.md` §T047
//! ("在 `crates/hivegui/tests/llm_config_store.rs` 编写 Provider/Preset/Model
//! CRUD、Provider name/Category 唯一、Token 仅加密存储、Token env 优先级
//! 设备密钥 Token、Preset 引用保护、重命名失败保留表单、搜索分页、
//! Mask token 测试").
//!
//! Red public boundaries (T051 will add these):
//!   - `hivegui::datasource::llm_provider_store::LlmProviderStore`
//!   - `hivegui::datasource::llm_provider_store::LlmProviderInput`
//!   - `hivegui::datasource::llm_provider_store::LlmProviderRecord`
//!   - `hivegui::datasource::llm_provider_store::LlmPresetRecord`
//!   - `hivegui::datasource::llm_provider_store::LlmModelRecord`
//!   - `hivegui::datasource::llm_provider_store::MaskedToken`
//!
//! T050 reviewer signs §T047.11; T051 implementation then makes
//! these tests Green; T054 reruns to record the Green evidence.
//!
//! The pool used by sqlx needs a tokio runtime, so the assertions
//! below use `#[tokio::test(flavor = "current_thread")]` rather
//! than `#[gpui::test]`. The `LlmProviderStore` public boundary
//! itself is async; callers do not need to be on a tokio runtime
//! to drive it.

mod support;

use hivegui::datasource::Crypto;
use hivegui::datasource::llm_provider_store::{
    LlmProviderInput, LlmProviderStore, LlmProviderTokenInput, MaskedToken,
};
use hivegui::datasource::llm_store::LlmStore;
use hivegui::datasource::migrations::{MigrationOptions, migrate_to_current};
use sqlx::Row;
use support::TestWorkspace;
use support::sensitive_canary::{
    SensitiveField, place_canary_for_test, scan_all_mediums_for_test, unique_canary_payload,
};

#[tokio::test(flavor = "current_thread")]
async fn create_provider_persists_encrypted_token() {
    let workspace = TestWorkspace::new().expect("test workspace");
    workspace
        .ensure_fixture_device_key()
        .expect("device key fixture");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    let store = LlmProviderStore::new(pool, fixture_key(&workspace))
        .await
        .expect("store");

    let input = LlmProviderInput::new(
        "openai-prod",
        "openai",
        LlmProviderTokenInput::Literal("sk-1234567890abcdef".into()),
        "https://api.openai.com/v1",
    )
    .expect("validated");
    let created = store.create(input).await.expect("create succeeds");
    assert_eq!(created.name(), "openai-prod");
    assert!(created.token_ciphertext().is_some());
    let masked: MaskedToken = created.token_masked();
    assert!(masked.as_str().contains("…"));
    assert!(!masked.as_str().contains("sk-1234567890abcdef"));
}

#[tokio::test(flavor = "current_thread")]
async fn duplicate_provider_name_returns_conflict() {
    let workspace = TestWorkspace::new().expect("test workspace");
    workspace
        .ensure_fixture_device_key()
        .expect("device key fixture");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    let store = LlmProviderStore::new(pool, fixture_key(&workspace))
        .await
        .expect("store");

    store
        .create(
            LlmProviderInput::new(
                "dup",
                "openai",
                LlmProviderTokenInput::Literal("sk-aaaa".into()),
                "https://api.openai.com/v1",
            )
            .unwrap(),
        )
        .await
        .expect("first create");
    let conflict = store
        .create(
            LlmProviderInput::new(
                "dup",
                "anthropic",
                LlmProviderTokenInput::Literal("sk-bbbb".into()),
                "https://api.anthropic.com/v1",
            )
            .unwrap(),
        )
        .await
        .expect_err("second create must conflict");
    assert_eq!(conflict.field(), "name");
}

#[tokio::test(flavor = "current_thread")]
async fn token_canary_leaves_zero_residue_across_all_mediums() {
    let workspace = TestWorkspace::new().expect("test workspace");
    workspace
        .ensure_fixture_device_key()
        .expect("device key fixture");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    let store = LlmProviderStore::new(pool, fixture_key(&workspace))
        .await
        .expect("store");

    let payload = unique_canary_payload("T047-canary-token");
    let canary =
        place_canary_for_test(SensitiveField::LlmProviderToken, &payload).expect("canary placed");
    store
        .create(
            LlmProviderInput::new(
                "canary-llm",
                "openai",
                LlmProviderTokenInput::Literal(payload.clone()),
                "https://api.openai.com/v1",
            )
            .unwrap(),
        )
        .await
        .expect("create");

    let scan = scan_all_mediums_for_test(workspace.root(), &canary).expect("canary scan");
    assert!(
        scan.hits.is_empty(),
        "LlmProviderToken canary {payload:?} must not appear in any medium; hits: {:?}",
        scan.hits
    );
}

#[tokio::test(flavor = "current_thread")]
async fn env_token_stores_only_env_name_and_no_ciphertext() {
    let workspace = TestWorkspace::new().expect("test workspace");
    workspace
        .ensure_fixture_device_key()
        .expect("device key fixture");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    let store = LlmProviderStore::new(pool, fixture_key(&workspace))
        .await
        .expect("store");

    let input = LlmProviderInput::new(
        "openai-env",
        "openai",
        LlmProviderTokenInput::Env("OPENAI_API_KEY".into()),
        "https://api.openai.com/v1",
    )
    .expect("validated");
    let created = store.create(input).await.expect("create succeeds");

    // Env 优先级：只存 env 变量名，绝无 device-key 加密的密文。
    assert_eq!(created.token_env(), "OPENAI_API_KEY");
    assert!(
        created.token_ciphertext().is_none(),
        "env token must not produce ciphertext"
    );
    let masked = created.token_masked();
    assert!(masked.as_str().contains("env"));
    assert!(!masked.as_str().contains("OPENAI_API_KEY"));
}

#[tokio::test(flavor = "current_thread")]
async fn literal_token_roundtrips_through_device_key_ciphertext() {
    let workspace = TestWorkspace::new().expect("test workspace");
    workspace
        .ensure_fixture_device_key()
        .expect("device key fixture");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    let store = LlmProviderStore::new(pool, fixture_key(&workspace))
        .await
        .expect("store");

    let input = LlmProviderInput::new(
        "literal-llm",
        "anthropic",
        LlmProviderTokenInput::Literal("sk-ant-secret".into()),
        "https://api.anthropic.com/v1",
    )
    .expect("validated");
    let created = store.create(input).await.expect("create succeeds");

    assert_eq!(created.token_env(), "");
    let ciphertext = created
        .token_ciphertext()
        .expect("literal token produces ciphertext");
    assert!(!ciphertext.is_empty());
    assert!(!ciphertext.windows(13).any(|w| w == b"sk-ant-secret"));
}

#[tokio::test(flavor = "current_thread")]
async fn empty_provider_fields_are_rejected_with_typed_validation() {
    // name / category / base_url 三项空值必须在本地校验阶段拒绝，
    // 且错误封套携带对应 field，供 UI 高亮与安全值回填。
    let name = LlmProviderInput::new(
        "",
        "openai",
        LlmProviderTokenInput::Env("X".into()),
        "https://api.openai.com/v1",
    )
    .expect_err("empty name must be rejected");
    assert_eq!(name.field(), "name");

    let category = LlmProviderInput::new(
        "p",
        "",
        LlmProviderTokenInput::Env("X".into()),
        "https://api.openai.com/v1",
    )
    .expect_err("empty category must be rejected");
    assert_eq!(category.field(), "category");

    let base_url = LlmProviderInput::new("p", "openai", LlmProviderTokenInput::Env("X".into()), "")
        .expect_err("empty base_url must be rejected");
    assert_eq!(base_url.field(), "base_url");
}

#[tokio::test(flavor = "current_thread")]
async fn list_all_returns_providers_in_insertion_order() {
    let workspace = TestWorkspace::new().expect("test workspace");
    workspace
        .ensure_fixture_device_key()
        .expect("device key fixture");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    let store = LlmProviderStore::new(pool, fixture_key(&workspace))
        .await
        .expect("store");

    for (name, category) in [("first", "a"), ("second", "b"), ("third", "c")] {
        store
            .create(
                LlmProviderInput::new(
                    name,
                    category,
                    LlmProviderTokenInput::Env("X".into()),
                    "https://example.com",
                )
                .expect("validated"),
            )
            .await
            .expect("create");
    }

    let all = store.list_all().await.expect("list");
    let names: Vec<&str> = all.iter().map(|p| p.name()).collect();
    assert_eq!(names, vec!["first", "second", "third"]);
}

fn fixture_key(workspace: &TestWorkspace) -> [u8; 32] {
    let bytes = std::fs::read(workspace.device_key_path()).expect("device key bytes");
    bytes[..32].try_into().expect("device key is 32 bytes")
}

// ── T047 Preset/Model 关系（对齐 data-model.md 权威 schema）──
//
// 这些测试走完整生产迁移（`migrate_to_current`）建出 agents + 权威 LLM
// 三表，再用 `LlmStore::migrate()` 建索引并 seed 内置 Preset。它们验证：
// Provider / Preset 相互独立，Model 同时关联 Provider（RESTRICT）与
// Preset（CASCADE），并按 (preset_id, priority, id) 排序。

async fn migrated_llm_store(workspace: &TestWorkspace) -> (LlmStore, sqlx::SqlitePool) {
    workspace
        .ensure_fixture_device_key()
        .expect("device key fixture");
    migrate_to_current(MigrationOptions::new(
        workspace.database_path(),
        workspace.plugin_root(),
    ))
    .await
    .expect("migrate to current");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    let store = LlmStore::new(pool.clone(), Crypto::new(&fixture_key(workspace)));
    store.migrate().await.expect("migrate llm tables");
    (store, pool)
}

async fn insert_agent(pool: &sqlx::SqlitePool, identifier: &str, model_preset: &str) {
    sqlx::query(
        "INSERT INTO agents (identifier, name, model_preset, created_at, updated_at) \
         VALUES (?, ?, ?, '', '')",
    )
    .bind(identifier)
    .bind(identifier)
    .bind(model_preset)
    .execute(pool)
    .await
    .expect("insert agent");
}

#[tokio::test(flavor = "current_thread")]
async fn preset_is_independent_and_model_links_provider_and_preset() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let (store, _pool) = migrated_llm_store(&workspace).await;

    let provider = store
        .create_provider(
            "deepseek-1",
            "deepseek",
            "https://api.deepseek.com",
            "secret",
            "",
        )
        .await
        .expect("provider");
    let preset = store
        .create_preset("coding-tier", "coding models", false, 4096, 0.2)
        .await
        .expect("preset");
    let model = store
        .create_model("deepseek-coder", preset.id, provider.id, 5)
        .await
        .expect("model");

    assert_eq!(model.preset_id, Some(preset.id));
    assert_eq!(model.provider_id, Some(provider.id));
    assert_eq!(model.priority, 5);
    // Provider 独立：不持有 preset/model 引用字段。
    assert_eq!(store.list_providers().await.expect("providers").len(), 1);
    // Preset 独立：seed 2 + 新建 1 = 3。
    assert_eq!(store.list_presets().await.expect("presets").len(), 3);
}

#[tokio::test(flavor = "current_thread")]
async fn only_one_default_preset_survives_atomic_switch() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let (store, _pool) = migrated_llm_store(&workspace).await;

    // seed 已含 is_default=true 的 cheap-fast；再设一个默认应原子切换。
    let preset = store
        .create_preset("expert-tier", "expert", true, 8192, 0.1)
        .await
        .expect("preset");

    let presets = store.list_presets().await.expect("presets");
    let defaults: Vec<_> = presets.iter().filter(|p| p.is_default == 1).collect();
    assert_eq!(defaults.len(), 1, "exactly one default preset");
    assert_eq!(defaults[0].id, preset.id);
}

#[tokio::test(flavor = "current_thread")]
async fn preset_rename_atomically_updates_referencing_agents() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let (store, pool) = migrated_llm_store(&workspace).await;

    let preset = store
        .create_preset("legacy-tier", "legacy", false, 2048, 0.7)
        .await
        .expect("preset");
    insert_agent(&pool, "agent-1", "legacy-tier").await;

    let updated = store
        .update_preset(preset.id, "renamed-tier", "legacy", false, 2048, 0.7)
        .await
        .expect("update preset");
    assert!(updated);

    let model_preset: String =
        sqlx::query_scalar("SELECT model_preset FROM agents WHERE identifier = 'agent-1'")
            .fetch_one(&pool)
            .await
            .expect("agent model_preset");
    assert_eq!(model_preset, "renamed-tier");
}

#[tokio::test(flavor = "current_thread")]
async fn deleting_referenced_preset_returns_conflict_with_safe_reference_list() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let (store, pool) = migrated_llm_store(&workspace).await;

    let preset = store
        .create_preset("used-tier", "used", false, 2048, 0.7)
        .await
        .expect("preset");
    insert_agent(&pool, "agent-1", "used-tier").await;
    insert_agent(&pool, "agent-2", "used-tier").await;

    let err = store
        .delete_preset(preset.id)
        .await
        .expect_err("must conflict");
    let msg = err.to_string();
    assert!(msg.contains("referenced_by_agent"), "msg: {msg}");
    assert!(msg.contains("agent-1"), "msg: {msg}");
    assert!(msg.contains("agent-2"), "msg: {msg}");

    // 零修改：agent 引用与 preset 均保留。
    let remaining: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM agents WHERE model_preset = 'used-tier'")
            .fetch_one(&pool)
            .await
            .expect("count agents");
    assert_eq!(remaining, 2);
    assert_eq!(store.list_presets().await.expect("presets").len(), 3);
}

#[tokio::test(flavor = "current_thread")]
async fn deleting_preset_cascades_to_its_models() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let (store, _pool) = migrated_llm_store(&workspace).await;

    let provider = store
        .create_provider(
            "openai-1",
            "openai",
            "https://api.openai.com/v1",
            "",
            "OPENAI_API_KEY",
        )
        .await
        .expect("provider");
    let preset = store
        .create_preset("temp-tier", "temp", false, 2048, 0.7)
        .await
        .expect("preset");
    store
        .create_model("gpt-4.1", preset.id, provider.id, 0)
        .await
        .expect("model");
    assert_eq!(store.list_models().await.expect("models").len(), 1);

    store
        .delete_preset(preset.id)
        .await
        .expect("delete preset without agent refs");

    // CASCADE：模型随 preset 一并删除。
    assert_eq!(store.list_models().await.expect("models").len(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn deleting_referenced_provider_is_restricted() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let (store, _pool) = migrated_llm_store(&workspace).await;

    let provider = store
        .create_provider(
            "openai-1",
            "openai",
            "https://api.openai.com/v1",
            "",
            "OPENAI_API_KEY",
        )
        .await
        .expect("provider");
    let preset = store
        .list_presets()
        .await
        .expect("presets")
        .into_iter()
        .find(|p| p.is_default == 1)
        .expect("default preset");
    store
        .create_model("gpt-4.1", preset.id, provider.id, 0)
        .await
        .expect("model");

    let err = store
        .delete_provider(provider.id)
        .await
        .expect_err("must restrict");
    assert!(err.to_string().contains("Model 引用"));
    assert_eq!(store.list_models().await.expect("models").len(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn models_order_by_preset_then_priority_then_id() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let (store, _pool) = migrated_llm_store(&workspace).await;

    let provider = store
        .create_provider(
            "openai-1",
            "openai",
            "https://api.openai.com/v1",
            "",
            "OPENAI_API_KEY",
        )
        .await
        .expect("provider");
    let preset = store
        .list_presets()
        .await
        .expect("presets")
        .into_iter()
        .find(|p| p.is_default == 1)
        .expect("default preset");

    store
        .create_model("m-p30", preset.id, provider.id, 30)
        .await
        .expect("m30");
    store
        .create_model("m-p10", preset.id, provider.id, 10)
        .await
        .expect("m10");
    store
        .create_model("m-p20", preset.id, provider.id, 20)
        .await
        .expect("m20");

    let priorities: Vec<i32> = store
        .list_models()
        .await
        .expect("models")
        .iter()
        .map(|m| m.priority)
        .collect();
    assert_eq!(priorities, vec![10, 20, 30]);
}

// ── T047 生产过滤/关联查询 EXPLAIN QUERY PLAN ──
//
// 验证 LLM 三表的过滤/关联查询命中权威索引，避免退化为全表扫描。
// EXPLAIN QUERY PLAN 的输出与具体绑定值无关，故用字面量 1 作占位。

async fn explain_plan(pool: &sqlx::SqlitePool, sql: &str) -> String {
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!("EXPLAIN QUERY PLAN {sql}")))
        .fetch_all(pool)
        .await
        .expect("explain query plan");
    let mut details = Vec::new();
    for row in rows {
        let detail: String = row.try_get(3).expect("detail column");
        details.push(detail);
    }
    details.join("\n")
}

#[tokio::test(flavor = "current_thread")]
async fn llm_filter_and_join_queries_use_expected_indexes() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let (store, pool) = migrated_llm_store(&workspace).await;

    // 建一条 model，让 planner 有真实行可规划。
    let provider = store
        .create_provider(
            "openai-1",
            "openai",
            "https://api.openai.com/v1",
            "",
            "OPENAI_API_KEY",
        )
        .await
        .expect("provider");
    let preset = store
        .list_presets()
        .await
        .expect("presets")
        .into_iter()
        .find(|p| p.is_default == 1)
        .expect("default preset");
    store
        .create_model("gpt-4.1", preset.id, provider.id, 0)
        .await
        .expect("model");

    // models 按 preset 分组 + priority 排序：命中 idx_models_preset_id_priority_id。
    let plan = explain_plan(
        &pool,
        "SELECT * FROM models WHERE preset_id = 1 ORDER BY priority, id",
    )
    .await;
    assert!(
        plan.contains("idx_models_preset_id_priority_id"),
        "models preset_id filter/order must use idx_models_preset_id_priority_id; plan: {plan}"
    );

    // delete_provider 的引用计数：命中 idx_models_provider_id。
    let plan = explain_plan(&pool, "SELECT COUNT(*) FROM models WHERE provider_id = 1").await;
    assert!(
        plan.contains("idx_models_provider_id"),
        "models provider_id filter must use idx_models_provider_id; plan: {plan}"
    );

    // 默认 Preset 过滤：命中 idx_llm_presets_is_default。
    let plan = explain_plan(&pool, "SELECT * FROM llm_presets WHERE is_default = 1").await;
    assert!(
        plan.contains("idx_llm_presets_is_default"),
        "llm_presets is_default filter must use idx_llm_presets_is_default; plan: {plan}"
    );
}

// ── T098 默认模型解析链 ──

#[tokio::test(flavor = "current_thread")]
async fn default_model_name_resolves_lowest_priority_model_of_default_preset() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let (store, _pool) = migrated_llm_store(&workspace).await;

    let default_preset = store
        .list_presets()
        .await
        .expect("presets")
        .into_iter()
        .find(|p| p.is_default == 1)
        .expect("default preset");
    let provider = store
        .create_provider(
            "openai-1",
            "openai",
            "https://api.openai.com/v1",
            "",
            "OPENAI_API_KEY",
        )
        .await
        .expect("provider");

    store
        .create_model("gpt-high-priority", default_preset.id, provider.id, 30)
        .await
        .expect("high priority model");
    store
        .create_model("gpt-low-priority", default_preset.id, provider.id, 10)
        .await
        .expect("low priority model");

    let name = store
        .default_model_name()
        .await
        .expect("default model name")
        .expect("default model must exist");
    assert_eq!(name, "gpt-low-priority", "lowest priority model wins");
}

#[tokio::test(flavor = "current_thread")]
async fn default_model_name_returns_none_when_default_preset_has_no_model() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let (store, _pool) = migrated_llm_store(&workspace).await;

    // seed 后有默认 preset（is_default=1）但没有 model。
    assert_eq!(
        store.default_model_name().await.expect("query"),
        None,
        "no model under the default preset"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn default_model_name_returns_none_without_default_preset() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let (store, pool) = migrated_llm_store(&workspace).await;

    // 清掉默认标记：没有任何 is_default=1 的 preset。
    sqlx::query("UPDATE llm_presets SET is_default = 0")
        .execute(&pool)
        .await
        .expect("clear default");

    assert_eq!(
        store.default_model_name().await.expect("query"),
        None,
        "no default preset → no default model"
    );
}
