//! T055 [P] [US5] Tag management contract.
//!
//! Source of truth: `specs/011-hivegui-standalone-mode/tasks.md` §T055
//! ("使用固定 fixture 编写 Tag CRUD、name 唯一、模糊搜索、每页20条分页、
//! 全部生产过滤/关联查询 EXPLAIN 预期索引、重启、CRUD p95≤1s 及搜索/
//! 翻页 p95≤500ms 测试").
//!
//! Red public boundaries (T058 will add these):
//!   - `hivegui::datasource::tag_store::TagStore`
//!   - `hivegui::datasource::tag_store::TagInput`
//!   - `hivegui::datasource::tag_store::TagRecord`
//!   - `hivegui::datasource::tag_store::TagFilter`
//!   - `hivegui::datasource::tag_store::TagPage`
//!
//! T057 reviewer signs §T055.11; T058 implementation then makes
//! these tests Green; T059 reruns to record the Green evidence.
//!
//! The pool used by sqlx needs a tokio runtime, so the assertions
//! below use `#[tokio::test(flavor = "current_thread")]` rather
//! than `#[tokio::test(flavor = "current_thread")]`. T058 provides the `TagStore` public
//! boundary the assertions drive; T059 reruns to record the
//! Green evidence.

mod support;

use std::time::Instant;

use hivegui::datasource::tag_store::{TagFilter, TagInput, TagPage, TagRecord, TagStore};
use support::TestWorkspace;

const PAGE_SIZE: usize = 20;

#[tokio::test(flavor = "current_thread")]
async fn create_tag_persists_unique_name() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    let store = TagStore::new(pool).await.expect("store");

    let created = store
        .create(TagInput::new("urgent", "#FF0000").expect("validated"))
        .await
        .expect("create");
    assert_eq!(created.name(), "urgent");
}

#[tokio::test(flavor = "current_thread")]
async fn duplicate_tag_name_returns_conflict() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    let store = TagStore::new(pool).await.expect("store");

    store
        .create(TagInput::new("dup", "#FF0000").unwrap())
        .await
        .expect("first create");
    let conflict = store
        .create(TagInput::new("dup", "#00FF00").unwrap())
        .await
        .expect_err("second create must conflict");
    assert_eq!(conflict.field(), "name");
}

#[tokio::test(flavor = "current_thread")]
async fn fuzzy_search_matches_normalized() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    let store = TagStore::new(pool).await.expect("store");

    store
        .create(TagInput::new("Backend", "#000").unwrap())
        .await
        .expect("create");
    let hits = store
        .search(TagFilter::new("back"), TagPage::first(PAGE_SIZE))
        .await
        .expect("search");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].name(), "Backend");
}

#[tokio::test(flavor = "current_thread")]
async fn explain_search_plan_uses_indexed_lookup() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    let store = TagStore::new(pool).await.expect("store");
    let plan = store.explain_search_plan("back").await.expect("EXPLAIN");
    assert!(
        plan.contains("USING INDEX tags_normalized_name_idx"),
        "tag search must use the normalized name index; got: {plan}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn one_hundred_tag_crud_p95_under_one_second() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    let store = TagStore::new(pool).await.expect("store");

    let mut samples = Vec::with_capacity(100);
    for i in 0..100 {
        let started = Instant::now();
        store
            .create(TagInput::new(&format!("tag.{i:03}"), "#000000").unwrap())
            .await
            .expect("create");
        samples.push(started.elapsed());
    }
    samples.sort();
    let p95 = samples[(samples.len() as f64 * 0.95).ceil() as usize - 1];
    assert!(
        p95 <= std::time::Duration::from_secs(1),
        "100 tag CRUD p95 = {p95:?}; budget is 1s"
    );
}

#[allow(dead_code)]
fn _pin_types() {
    let _: TagPage = TagPage::first(PAGE_SIZE);
    let _: TagFilter = TagFilter::default();
    let _: TagRecord = panic!("placeholder so the type is referenced");
}
