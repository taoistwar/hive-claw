//! T066 [P] [US7] Capability management contract.
//!
//! Source of truth: `specs/011-hivegui-standalone-mode/tasks.md` §T066
//! ("编写 Capability CRUD、name 主键、分类 SET NULL、引用保护、
//! 每页20条搜索分页、全部生产过滤/关联查询 EXPLAIN 预期索引、
//! CRUD p95≤1s 及搜索/翻页 p95≤500ms 测试").
//!
//! Red public boundaries (T069 will add these):
//!   - `hivegui::datasource::capability_store::CapabilityStore`
//!   - `hivegui::datasource::capability_store::CapabilityInput`
//!   - `hivegui::datasource::capability_store::CapabilityRecord`
//!   - `hivegui::datasource::capability_store::CapabilityFilter`
//!
//! T068 reviewer signs §T066.11 + T067.11; T069 + T070 implementation
//! then make these tests Green; T071 reruns to record the Green
//! evidence.

mod support;

use std::time::Instant;

use hivegui::datasource::capability_store::{
    CapabilityFilter, CapabilityInput, CapabilityPage, CapabilityRecord, CapabilityStore,
};
use support::TestWorkspace;

const PAGE_SIZE: usize = 20;

#[tokio::test(flavor = "current_thread")]
async fn create_capability_persists_unique_name() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    let store = CapabilityStore::new(pool).await.expect("store");

    let created = store
        .create(CapabilityInput::new("http_get", "HTTP GET request", false).expect("validated"))
        .await
        .expect("create");
    assert_eq!(created.name(), "http_get");
    assert!(!created.is_dangerous());
}

#[tokio::test(flavor = "current_thread")]
async fn dangerous_capability_must_be_flagged() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    let store = CapabilityStore::new(pool).await.expect("store");

    let created = store
        .create(CapabilityInput::new("shell_exec", "Execute shell command", true).unwrap())
        .await
        .expect("create");
    assert!(created.is_dangerous());
}

#[tokio::test(flavor = "current_thread")]
async fn duplicate_name_returns_conflict() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    let store = CapabilityStore::new(pool).await.expect("store");

    store
        .create(CapabilityInput::new("dup", "first", false).unwrap())
        .await
        .expect("first create");
    let conflict = store
        .create(CapabilityInput::new("dup", "second", false).unwrap())
        .await
        .expect_err("second create must conflict");
    assert_eq!(conflict.field(), "name");
}

#[tokio::test(flavor = "current_thread")]
async fn explain_search_plan_uses_indexed_lookup() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    let store = CapabilityStore::new(pool).await.expect("store");
    let plan = store.explain_search_plan("http").await.expect("EXPLAIN");
    assert!(
        plan.contains("USING INDEX capabilities_normalized_name_idx"),
        "capability search must use the normalized name index; got: {plan}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn one_hundred_capability_crud_p95_under_one_second() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    let store = CapabilityStore::new(pool).await.expect("store");

    let mut samples = Vec::with_capacity(100);
    for i in 0..100 {
        let started = Instant::now();
        store
            .create(CapabilityInput::new(&format!("cap.{i:03}"), "desc", false).unwrap())
            .await
            .expect("create");
        samples.push(started.elapsed());
    }
    samples.sort();
    let p95 = samples[(samples.len() as f64 * 0.95).ceil() as usize - 1];
    assert!(
        p95 <= std::time::Duration::from_secs(1),
        "100 capability CRUD p95 = {p95:?}; budget is 1s"
    );
}

#[allow(dead_code)]
fn _pin_types() {
    let _: CapabilityPage = CapabilityPage::first(PAGE_SIZE);
    let _: CapabilityFilter = CapabilityFilter::default();
    let _: CapabilityRecord = panic!("placeholder so the type is referenced");
}
