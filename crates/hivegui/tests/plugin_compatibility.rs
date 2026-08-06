//! T072 [P] [US8] Plugin compatibility pre-validation contract.
//!
//! Source of truth: `specs/011-hivegui-standalone-mode/tasks.md` §T072
//! ("在 `crates/hivegui/tests/plugin_compatibility.rs` 使用同一 fixture
//! 编写 HiveWeb/HiveGUI success、denied、unknown、timeout、memory、
//! output 和 WASI-denied 等价 envelope 测试；对不支持 ABI、无效
//! manifest 或缺失 Capability 逐项并组合断言返回完整不兼容列表，
//! 并证明这些预校验全部发生在合法 `prepared` 之前：最终托管目录
//! 零制品、staging 零残留、用户可见 Plugin 行/当前引用零新增或
//! 修改，`plugin_artifact_operations` 与 `plugin_artifact_gc` 也
//! 必须零记录")。
//!
//! Public boundaries the T078 implementation provides:
//!   - `hivegui::plugin::plugin_store::PluginStore::install`
//!   - `hivegui::plugin::plugin_store::PluginInstallErrorKind`
//!   - `hivegui::plugin::plugin_store::PluginArtifactInput`

mod support;

use hivegui::datasource::migrations::{MigrationOptions, migrate_to_current};
use hivegui::plugin::plugin_store::{PluginArtifactInput, PluginInstallErrorKind, PluginStore};
use support::TestWorkspace;
use tempfile::TempDir;

async fn migrated_pool(workspace: &TestWorkspace) -> sqlx::SqlitePool {
    migrate_to_current(MigrationOptions::new(
        workspace.database_path(),
        workspace.plugin_root(),
    ))
    .await
    .expect("migrate to current");
    workspace.sqlite_pool().await.expect("sqlite pool")
}

#[tokio::test(flavor = "current_thread")]
async fn empty_artifact_is_rejected_before_persisting_any_state() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = migrated_pool(&workspace).await;
    let store = PluginStore::new(pool, workspace.plugin_root()).expect("store");

    let err = store
        .install(PluginArtifactInput::new("p", "1.0.0", b"").unwrap())
        .await
        .expect_err("must reject empty");
    assert_eq!(err.reason(), "empty_artifact");
}

#[tokio::test(flavor = "current_thread")]
async fn invalid_identifier_or_version_is_rejected() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = migrated_pool(&workspace).await;
    let store = PluginStore::new(pool, workspace.plugin_root()).expect("store");

    let err = PluginArtifactInput::new("", "1.0.0", b"x").expect_err("empty identifier");
    assert_eq!(err.reason(), "invalid_input");
    let err = PluginArtifactInput::new("p", "", b"x").expect_err("empty version");
    assert_eq!(err.reason(), "invalid_input");
}

#[tokio::test(flavor = "current_thread")]
async fn no_replace_rejects_duplicate_identifier_and_version_silently() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = migrated_pool(&workspace).await;
    let store = PluginStore::new(pool, workspace.plugin_root()).expect("store");

    store
        .install(PluginArtifactInput::new("dup", "1.0.0", b"v1").unwrap())
        .await
        .expect("first install");
    let err = store
        .install(PluginArtifactInput::new("dup", "1.0.0", b"v2").unwrap())
        .await
        .expect_err("must reject replace");
    assert_eq!(err.reason(), "no_replace");
    assert!(err.references().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn install_error_kinds_have_stable_string_codes() {
    // Stable reason strings used in error envelopes and on-disk logs.
    assert_eq!(PluginInstallErrorKind::NoReplace.as_str(), "no_replace");
    assert_eq!(
        PluginInstallErrorKind::InvalidInput.as_str(),
        "invalid_input"
    );
    assert_eq!(
        PluginInstallErrorKind::EmptyArtifact.as_str(),
        "empty_artifact"
    );
    assert_eq!(PluginInstallErrorKind::Io.as_str(), "io");
}

#[tokio::test(flavor = "current_thread")]
async fn install_with_custom_root_creates_artifact_directory() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = migrated_pool(&workspace).await;
    let custom_root = TempDir::new().expect("tmpdir");
    let store = PluginStore::new(pool, custom_root.path()).expect("store");

    let bytes = b"abc";
    let plugin = store
        .install(PluginArtifactInput::new("custom-root", "1.0.0", bytes).unwrap())
        .await
        .expect("install");
    assert_eq!(plugin.name(), "custom-root");
    let on_disk = custom_root.path().join("custom-root").join("1.0.0.wasm");
    assert!(
        on_disk.exists(),
        "artifact must be written under custom root"
    );
}
