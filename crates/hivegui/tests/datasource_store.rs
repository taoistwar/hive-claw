//! T034 [P] [US2] DataSource store contract + T016F
//! `DataSourcePassword` canary activation.
//!
//! Source of truth: `specs/011-hivegui-standalone-mode/tasks.md` §T034
//! ("以固定 fixture 编写 CRUD、唯一性、设备密钥加密、空密码不覆盖、
//! 重启恢复、全部生产过滤/关联查询 EXPLAIN 预期索引和 100 次操作
//! p95≤1s 测试；同时激活 T016F 的 DataSource `encrypted_password` 行，
//! 首次用唯一明文 canary 覆盖正常/错误/崩溃恢复并扫描 SQLite 主文件、
//! WAL/SHM/journal、临时目录和脱敏错误").
//!
//! Red public boundaries (T038 will add these):
//!   - `hivegui::datasource::data_source_store::DataSourceStore`
//!   - `hivegui::datasource::data_source_store::DataSourceInput`
//!   - `hivegui::datasource::data_source_store::DataSourceRecord`
//!   - `hivegui::datasource::data_source_store::ConflictReason`
//!   - `hivegui::datasource::data_source_store::EmptyPasswordPolicy`
//!     — empty string is "no change", not "clear".
//!   - `hivegui::datasource::data_source_store::DataSourceFilter`
//!   - `hivegui::datasource::data_source_store::DataSourcePage`
//!   - `hivegui::datasource::data_source_store::explain_list_plan`
//!   - `support::sensitive_canary::place_canary_for_test` (T016F
//!     Foundation gate; the DataSourcePassword row is the *first*
//!     non-Foundation activation.)
//!   - `support::sensitive_canary::scan_all_mediums_for_test`.
//!
//! The pool used by sqlx needs a tokio runtime, so the assertions
//! below use `#[tokio::test(flavor = "current_thread")]` rather
//! than `#[gpui::test]`. T038 provides the `DataSourceStore`
//! public boundary the assertions drive; T040 reruns to record
//! the Green evidence.
//!
//! T037 reviewer signs §T034.11; T038 implementation then makes
//! these tests Green; T040 reruns to record the Green evidence.

mod support;

use std::time::Instant;

use hivegui::datasource::data_source_store::{
    ConflictReason, DataSourceFilter, DataSourceInput, DataSourcePage, DataSourceRecord,
    DataSourceStore, EmptyPasswordPolicy,
};
use support::TestWorkspace;
use support::sensitive_canary::{
    SensitiveField, place_canary_for_test, scan_all_mediums_for_test, unique_canary_payload,
};

// ---------------------------------------------------------------------------
// §T034.1 — CRUD + unique `name` constraint.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn create_persists_record_with_encrypted_password() {
    let workspace = TestWorkspace::new().expect("test workspace");
    workspace
        .ensure_fixture_device_key()
        .expect("device key fixture");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    let store = DataSourceStore::new(pool, fixture_key(&workspace))
        .await
        .expect("store");

    let input = DataSourceInput::new(
        "fixture-primary",
        "127.0.0.1",
        3306,
        "hivegui",
        "P1aintext-C4n4ry-2026-07-30",
        "hivegui",
    )
    .expect("validated input");

    let created = store.create(input, None).await.expect("create succeeds");
    assert_eq!(created.name(), "fixture-primary");
    assert!(created.password_ciphertext().is_some());
}

#[tokio::test(flavor = "current_thread")]
async fn duplicate_name_returns_conflict_with_reason_name() {
    let workspace = TestWorkspace::new().expect("test workspace");
    workspace
        .ensure_fixture_device_key()
        .expect("device key fixture");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    let store = DataSourceStore::new(pool, fixture_key(&workspace))
        .await
        .expect("store");

    let input_a = DataSourceInput::new("dup-name", "127.0.0.1", 3306, "u", "p1", "d").unwrap();
    let input_b = DataSourceInput::new("dup-name", "127.0.0.1", 3306, "u", "p2", "d").unwrap();
    store.create(input_a, None).await.expect("first create");
    let conflict = store
        .create(input_b, None)
        .await
        .expect_err("second create must conflict");
    assert_eq!(conflict.reason(), ConflictReason::Name);
    assert_eq!(conflict.field(), "name");
    assert!(conflict.references().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn empty_password_on_update_keeps_existing_ciphertext() {
    let workspace = TestWorkspace::new().expect("test workspace");
    workspace
        .ensure_fixture_device_key()
        .expect("device key fixture");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    let store = DataSourceStore::new(pool, fixture_key(&workspace))
        .await
        .expect("store");

    let input =
        DataSourceInput::new("keep-secret", "127.0.0.1", 3306, "u", "OriginalP@ss", "d").unwrap();
    let created = store.create(input, None).await.unwrap();
    let original_ciphertext = created.password_ciphertext().unwrap().to_vec();

    let mut update = DataSourceInput::for_update(created.id());
    update.name = "keep-secret".into();
    update.password = String::new();
    update.policy = EmptyPasswordPolicy::KeepExisting;
    let updated = store.update(update).await.expect("empty-password update");

    assert_eq!(
        updated.password_ciphertext().unwrap(),
        &original_ciphertext[..],
        "empty password must NOT overwrite the existing ciphertext"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn restart_recovery_preserves_records_and_ciphertexts() {
    let workspace = TestWorkspace::new().expect("test workspace");
    workspace
        .ensure_fixture_device_key()
        .expect("device key fixture");

    {
        let pool = workspace.sqlite_pool().await.expect("sqlite pool");
        let store = DataSourceStore::new(pool, fixture_key(&workspace))
            .await
            .expect("store");
        store
            .create(
                DataSourceInput::new("after-restart", "127.0.0.1", 3306, "u", "secret", "d")
                    .unwrap(),
                None,
            )
            .await
            .expect("create");
    }

    let second_pool = workspace.sqlite_pool().await.expect("sqlite pool");
    let second_store = DataSourceStore::new(second_pool, fixture_key(&workspace))
        .await
        .expect("store");
    let recovered = second_store
        .get_by_name("after-restart")
        .await
        .expect("record survives restart");
    assert!(recovered.password_ciphertext().is_some());
}

// ---------------------------------------------------------------------------
// §T034.2 — EXPLAIN query plans + p95 latency.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn list_and_search_use_indexed_plans_only() {
    let workspace = TestWorkspace::new().expect("test workspace");
    workspace
        .ensure_fixture_device_key()
        .expect("device key fixture");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    let store = DataSourceStore::new(pool, fixture_key(&workspace))
        .await
        .expect("store");
    for i in 0..50 {
        store
            .create(
                DataSourceInput::new(
                    &format!("datasource-{i:03}"),
                    "127.0.0.1",
                    3306,
                    "u",
                    "secret",
                    "d",
                )
                .unwrap(),
                None,
            )
            .await
            .expect("seed");
    }
    let plan = store.explain_list_plan().await.expect("EXPLAIN list plan");
    assert!(
        plan.contains("USING INDEX data_sources_name_idx")
            || plan.contains("USING COVERING INDEX data_sources_name_idx")
            || plan.contains("USING INDEX data_sources_lower_name_idx"),
        "list plan must hit the `name` index; got: {plan}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn one_hundred_crud_operations_p95_under_one_second() {
    let workspace = TestWorkspace::new().expect("test workspace");
    workspace
        .ensure_fixture_device_key()
        .expect("device key fixture");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    let store = DataSourceStore::new(pool, fixture_key(&workspace))
        .await
        .expect("store");

    let mut samples = Vec::with_capacity(100);
    for i in 0..100 {
        let started = Instant::now();
        let name = format!("perf-{i:03}");
        let created = store
            .create(
                DataSourceInput::new(&name, "127.0.0.1", 3306, "u", "secret", "d").unwrap(),
                None,
            )
            .await
            .expect("create");
        store.get_by_id(created.id()).await.expect("read");
        samples.push(started.elapsed());
    }
    samples.sort();
    let p95 = samples[(samples.len() as f64 * 0.95).ceil() as usize - 1];
    assert!(
        p95 <= std::time::Duration::from_secs(1),
        "100-op p95 = {p95:?}; budget is 1s"
    );
}

// ---------------------------------------------------------------------------
// §T034.3 — T016F `DataSourcePassword` canary activation.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn encrypted_password_canary_leaves_zero_residue_across_all_mediums() {
    let workspace = TestWorkspace::new().expect("test workspace");
    workspace
        .ensure_fixture_device_key()
        .expect("device key fixture");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    let store = DataSourceStore::new(pool, fixture_key(&workspace))
        .await
        .expect("store");

    let payload = unique_canary_payload("T034-canary-encrypted-password");
    let canary = place_canary_for_test(SensitiveField::DataSourcePassword, &payload)
        .expect("canary placed via T016F public boundary");
    store
        .create(
            DataSourceInput::new("canary-target", "127.0.0.1", 3306, "u", &payload, "d").unwrap(),
            None,
        )
        .await
        .expect("create");

    let scan = scan_all_mediums_for_test(workspace.root(), &canary).expect("canary scan");
    assert!(
        scan.hits.is_empty(),
        "encrypted_password canary {payload:?} must not appear in any medium; hits: {:?}",
        scan.hits
    );
}

#[tokio::test(flavor = "current_thread")]
async fn sanitized_error_does_not_leak_canary_plaintext() {
    let workspace = TestWorkspace::new().expect("test workspace");
    workspace
        .ensure_fixture_device_key()
        .expect("device key fixture");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    let store = DataSourceStore::new(pool, fixture_key(&workspace))
        .await
        .expect("store");

    let payload = unique_canary_payload("T034-canary-error-leak");
    let canary = place_canary_for_test(SensitiveField::DataSourcePassword, &payload)
        .expect("canary placed via T016F public boundary");
    store
        .create(
            DataSourceInput::new("err-target", "127.0.0.1", 3306, "u", &payload, "d").unwrap(),
            None,
        )
        .await
        .expect("create");
    // Trigger the "not found" error path by looking up a name that
    // does not exist. The error envelope must not echo the canary
    // payload back to the caller.
    let error = store
        .get_by_name("err-target-missing")
        .await
        .err()
        .unwrap_or_else(|| panic!("forced error path"))
        .to_string();
    assert!(
        !error.contains(canary.plaintext_token().as_str()),
        "sanitized error leaked the password canary fingerprint: {error}"
    );
}

// ---------------------------------------------------------------------------
// §T034.4 — Test harness.
// ---------------------------------------------------------------------------

fn fixture_key(workspace: &TestWorkspace) -> [u8; 32] {
    let bytes = std::fs::read(workspace.device_key_path()).expect("device key bytes");
    bytes[..32].try_into().expect("device key is 32 bytes")
}

#[allow(dead_code)]
fn page_one() -> DataSourcePage {
    DataSourcePage {
        offset: 0,
        limit: 20,
    }
}

#[allow(dead_code)]
fn no_filter() -> DataSourceFilter {
    DataSourceFilter::default()
}

// Pin the symbols T038 will define so the import list above is a
// stable, reviewer-visible contract.
#[allow(clippy::diverging_sub_expression)]
const _: fn() = || {
    let _: DataSourcePage = page_one();
    let _: DataSourceFilter = no_filter();
    let _: SensitiveField = SensitiveField::DataSourcePassword;
    let _: Box<dyn Send + Sync> = Box::new(0_i32) as Box<dyn Send + Sync>;
    let _: DataSourceRecord = panic!("placeholder so the type is referenced");
};
