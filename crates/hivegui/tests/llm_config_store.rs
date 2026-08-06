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

use hivegui::datasource::llm_provider_store::{
    LlmProviderInput, LlmProviderStore, LlmProviderTokenInput, MaskedToken,
};
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

fn fixture_key(workspace: &TestWorkspace) -> [u8; 32] {
    let bytes = std::fs::read(workspace.device_key_path()).expect("device key bytes");
    bytes[..32].try_into().expect("device key is 32 bytes")
}
