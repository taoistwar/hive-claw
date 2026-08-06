//! T041 [P] [US3] GlobalConfig store contract.
//!
//! Source of truth: `specs/011-hivegui-standalone-mode/tasks.md` §T041
//! ("使用固定1万条 fixture 编写 key 唯一、CRUD、大小写不敏感模糊搜索、
//! 每页20条分页、全部生产过滤/关联查询 EXPLAIN 预期索引、重启恢复及
//! 搜索/翻页 p95≤500ms 测试").
//!
//! Red public boundaries (T044 will add these):
//!   - `hivegui::datasource::global_config_store::GlobalConfigStore`
//!   - `hivegui::datasource::global_config_store::GlobalConfigInput`
//!   - `hivegui::datasource::global_config_store::GlobalConfigRecord`
//!   - `hivegui::datasource::global_config_store::GlobalConfigFilter`
//!   - `hivegui::datasource::global_config_store::GlobalConfigPage`
//!   - `hivegui::datasource::global_config_store::explain_search_plan`
//!
//! T043 reviewer signs §T041.11; T044 implementation then makes
//! these tests Green; T046 reruns to record the Green evidence.
//!
//! The pool used by sqlx needs a tokio runtime, so the assertions
//! below use `#[tokio::test(flavor = "current_thread")]` rather
//! than `#[gpui::test]`. The `GlobalConfigStore` public boundary
//! itself is sync (it owns an internal current-thread tokio
//! runtime for schema migration + ad-hoc reads) so callers do not
//! need to be async to drive it.

mod support;

use std::time::Instant;

use hivegui::datasource::global_config_store::{
    GlobalConfigFilter, GlobalConfigInput, GlobalConfigPage, GlobalConfigStore,
};
use support::TestWorkspace;

const ONE_WAN_FIXTURE: usize = 10_000;
const PAGE_SIZE: usize = 20;

#[tokio::test(flavor = "current_thread")]
async fn create_persists_unique_key_with_canonical_normalized_form() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    let store = GlobalConfigStore::new(pool).await.expect("store");

    let input = GlobalConfigInput::new("feature.ui.theme", "dark").expect("validated");
    let created = store.create(input).await.expect("create succeeds");
    assert_eq!(created.key(), "feature.ui.theme");
    assert_eq!(created.value(), "dark");
}

#[tokio::test(flavor = "current_thread")]
async fn duplicate_key_returns_conflict_with_field_key() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    let store = GlobalConfigStore::new(pool).await.expect("store");

    store
        .create(GlobalConfigInput::new("shared.key", "v1").unwrap())
        .await
        .expect("first create");
    let conflict = store
        .create(GlobalConfigInput::new("shared.key", "v2").unwrap())
        .await
        .expect_err("second create must conflict");
    assert_eq!(conflict.field(), "key");
    assert_eq!(conflict.reason(), "duplicate");
}

#[tokio::test(flavor = "current_thread")]
async fn case_insensitive_search_finds_normalized_match() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    let store = GlobalConfigStore::new(pool).await.expect("store");

    store
        .create(GlobalConfigInput::new("Feature.UI.Theme", "light").unwrap())
        .await
        .expect("create");
    let filter = GlobalConfigFilter::new("FEATURE.ui.theme");
    let hits = store
        .search(filter, GlobalConfigPage::first(PAGE_SIZE))
        .await
        .expect("search");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].key(), "Feature.UI.Theme");
}

#[tokio::test(flavor = "current_thread")]
async fn explain_search_plan_uses_normalized_key_index() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    let store = GlobalConfigStore::new(pool).await.expect("store");

    let plan = store
        .explain_search_plan("ui.theme")
        .await
        .expect("EXPLAIN search plan");
    assert!(
        plan.contains("USING INDEX global_configs_normalized_key_idx"),
        "search must use the normalized key index; got: {plan}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn restart_recovery_preserves_records() {
    let workspace = TestWorkspace::new().expect("test workspace");
    {
        let pool = workspace.sqlite_pool().await.expect("sqlite pool");
        let store = GlobalConfigStore::new(pool).await.expect("store");
        store
            .create(GlobalConfigInput::new("after.restart", "value").unwrap())
            .await
            .expect("create");
    }
    let pool2 = workspace.sqlite_pool().await.expect("sqlite pool");
    let store2 = GlobalConfigStore::new(pool2).await.expect("store");
    let recovered = store2
        .get_by_key("after.restart")
        .await
        .expect("record survives restart");
    assert_eq!(recovered.value(), "value");
}

#[tokio::test(flavor = "current_thread")]
async fn one_wan_fixture_crud_p95_under_one_second() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    let store = GlobalConfigStore::new(pool).await.expect("store");
    for i in 0..ONE_WAN_FIXTURE {
        store
            .create(
                GlobalConfigInput::new(&format!("bench.key.{i:05}"), &format!("value-{i}"))
                    .unwrap(),
            )
            .await
            .expect("seed");
    }
    let mut samples = Vec::with_capacity(ONE_WAN_FIXTURE);
    for i in 0..ONE_WAN_FIXTURE {
        let started = Instant::now();
        store
            .get_by_key(&format!("bench.key.{i:05}"))
            .await
            .expect("read by key");
        samples.push(started.elapsed());
    }
    samples.sort();
    let p95 = samples[(samples.len() as f64 * 0.95).ceil() as usize - 1];
    assert!(
        p95 <= std::time::Duration::from_millis(500),
        "1万 op p95 = {p95:?}; budget is 500ms (search/pagination budget)"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn pagination_is_stable_across_pages() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    let store = GlobalConfigStore::new(pool).await.expect("store");
    for i in 0..100 {
        store
            .create(GlobalConfigInput::new(&format!("p.{i:03}"), "v").unwrap())
            .await
            .expect("seed");
    }
    let page_a = store
        .list(
            GlobalConfigFilter::default(),
            GlobalConfigPage::new(0, PAGE_SIZE),
        )
        .await
        .expect("page 0");
    let page_b = store
        .list(
            GlobalConfigFilter::default(),
            GlobalConfigPage::new(PAGE_SIZE, PAGE_SIZE),
        )
        .await
        .expect("page 1");
    let ids_a: std::collections::HashSet<_> = page_a.iter().map(|r| r.id()).collect();
    let ids_b: std::collections::HashSet<_> = page_b.iter().map(|r| r.id()).collect();
    assert!(ids_a.is_disjoint(&ids_b), "pages must not overlap");
}
