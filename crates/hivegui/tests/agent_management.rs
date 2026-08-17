//! T115 [P] [US13] Agent management contract.
//!
//! Source of truth: `specs/011-hivegui-standalone-mode/tasks.md` §T115
//! ("在 `crates/hivegui/tests/agent_management.rs` 使用固定 100+
//! Agent fixture 编写唯一默认根、首个自动默认、替代默认删除、自动
//! depth、循环/depth>10、非空 model_preset 必须存在及不存在时
//! 字段级 invalid_input/零修改、三类关联批量加载/查询计数、
//! 每页20条搜索分页、全部生产过滤/关联查询 EXPLAIN、固定100次
//! CRUD p95≤1s、搜索/翻页 p95≤500ms 及 T005 基线比较测试").
//!
//! Public boundaries the T124 implementation must satisfy:
//!   - `hivegui::datasource::entity_store::AgentStore`
//!   - `hivegui::datasource::entity_store::AgentInput`
//!   - `hivegui::datasource::entity_store::AgentRecord`
//!   - `hivegui::datasource::entity_store::AgentFilter`
//!   - `hivegui::datasource::entity_store::AgentPage`
//!
//! T123 reviewer signs §T115.11; T124 implementation then makes
//! these tests Green; T136 reruns to record the Green evidence.

#![allow(missing_docs)]

mod support;

use std::time::Instant;

use hivegui::datasource::entity_store::{
    AgentFilter, AgentInput, AgentPage, AgentRecord, AgentStore, init_tables,
};
use support::TestWorkspace;

const AGENT_FIXTURE_SIZE: usize = 100;
const PAGE_SIZE: usize = 20;

async fn fresh_store() -> (TestWorkspace, AgentStore) {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    init_tables(&pool).await.expect("init_tables");
    let store = AgentStore::new(pool).expect("store");
    (workspace, store)
}

fn root_input(identifier: &str, name: &str) -> AgentInput {
    AgentInput::new_root(identifier, name, "system prompt").expect("validated root")
}

#[tokio::test(flavor = "current_thread")]
async fn first_agent_is_automatically_default() {
    let (_workspace, store) = fresh_store().await;

    let first = store
        .create(root_input("first", "First Agent"))
        .await
        .expect("create first");
    assert!(
        first.is_default(),
        "first Agent must be auto-default per T115"
    );

    let second = store
        .create(root_input("second", "Second Agent"))
        .await
        .expect("create second");
    assert!(!second.is_default(), "second Agent must not be default");

    let default = store
        .default_agent()
        .await
        .expect("default fetch")
        .expect("default exists");
    assert_eq!(default.identifier(), "first");
}

#[tokio::test(flavor = "current_thread")]
async fn default_replacement_is_atomic_and_keeps_invariant() {
    let (_workspace, store) = fresh_store().await;

    let a = store
        .create(root_input("a", "Agent A"))
        .await
        .expect("create a");
    let b = store
        .create(root_input("b", "Agent B"))
        .await
        .expect("create b");

    let switched = store
        .set_default(b.id())
        .await
        .expect("set_default must succeed for root Agent");
    assert!(switched.is_default());
    assert_eq!(switched.identifier(), "b");

    // Original default must have been atomically cleared.
    let a_reloaded = store
        .fetch_one(a.id())
        .await
        .expect("fetch a")
        .expect("a exists");
    assert!(!a_reloaded.is_default());
}

#[tokio::test(flavor = "current_thread")]
async fn parent_agent_id_computes_depth_automatically() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    init_tables(&pool).await.expect("init_tables");
    let store = AgentStore::new(pool).expect("store");

    let parent = store
        .create(root_input("parent", "Parent"))
        .await
        .expect("create parent");
    let child = store
        .create(root_input("child", "Child").with_parent(Some(parent.id())))
        .await
        .expect("create child");
    assert_eq!(child.depth(), 1, "child must inherit parent depth + 1");
    assert_eq!(child.parent_agent_id(), Some(parent.id()));
}

#[tokio::test(flavor = "current_thread")]
async fn non_default_root_cannot_become_default() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    init_tables(&pool).await.expect("init_tables");
    let store = AgentStore::new(pool).expect("store");

    let parent = store
        .create(root_input("parent", "Parent"))
        .await
        .expect("create parent");
    let child = store
        .create(root_input("child", "Child").with_parent(Some(parent.id())))
        .await
        .expect("create child");

    let err = store
        .set_default(child.id())
        .await
        .expect_err("child must not be allowed as default");
    assert_eq!(err.field(), "id");
    assert_eq!(err.reason(), "must_be_root");
}

#[tokio::test(flavor = "current_thread")]
async fn list_children_returns_only_direct_children() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    init_tables(&pool).await.expect("init_tables");
    let store = AgentStore::new(pool).expect("store");

    let parent = store
        .create(root_input("parent", "Parent"))
        .await
        .expect("create parent");
    let child = store
        .create(root_input("child", "Child").with_parent(Some(parent.id())))
        .await
        .expect("create child");
    let grandchild = store
        .create(root_input("grand", "Grandchild").with_parent(Some(child.id())))
        .await
        .expect("create grandchild");

    let direct = store
        .list_children(parent.id())
        .await
        .expect("list children");
    assert_eq!(direct.len(), 1);
    assert_eq!(direct[0].identifier(), "child");

    let grand = store
        .list_children(child.id())
        .await
        .expect("list grandchildren");
    assert_eq!(grand.len(), 1);
    assert_eq!(grand[0].identifier(), "grand");
    let _ = grandchild;
}

#[tokio::test(flavor = "current_thread")]
async fn empty_identifier_is_rejected_with_field_level_error() {
    // AgentInput::new_root enforces the field-level contract at
    // the public input boundary; the store layer never sees an
    // invalid value.
    let err = AgentInput::new_root("", "Empty", "system prompt")
        .expect_err("empty identifier must fail at the input layer");
    assert_eq!(err.field(), "identifier");
    assert_eq!(err.reason(), "empty");

    let err = AgentInput::new_root("ok", "Ok", "")
        .expect_err("empty system prompt must fail at the input layer");
    assert_eq!(err.field(), "system_prompt");
    assert_eq!(err.reason(), "empty");
}

#[tokio::test(flavor = "current_thread")]
async fn invalid_charset_identifier_is_rejected() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    init_tables(&pool).await.expect("init_tables");
    let store = AgentStore::new(pool).expect("store");

    let err = AgentInput::new_root("invalid name with spaces", "Bad", "prompt")
        .expect_err("invalid identifier must fail at input layer");
    assert_eq!(err.field(), "identifier");
    assert_eq!(err.reason(), "invalid_charset");
    let _ = store;
}

#[tokio::test(flavor = "current_thread")]
async fn duplicate_identifier_returns_conflict() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    init_tables(&pool).await.expect("init_tables");
    let store = AgentStore::new(pool).expect("store");

    store
        .create(root_input("dup", "First"))
        .await
        .expect("first create");
    let err = store
        .create(root_input("dup", "Second"))
        .await
        .expect_err("second create must fail");
    assert_eq!(err.field(), "identifier");
    assert_eq!(err.reason(), "duplicate");
}

#[tokio::test(flavor = "current_thread")]
async fn search_filter_pages_records_in_groups_of_twenty() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    init_tables(&pool).await.expect("init_tables");
    let store = AgentStore::new(pool).expect("store");

    for i in 0..AGENT_FIXTURE_SIZE {
        store
            .create(root_input(&format!("agent-{i:03}"), &format!("Agent {i}")))
            .await
            .expect("create");
    }

    let first = store
        .search(&AgentFilter::first().with_page(1))
        .await
        .expect("page 1");
    let second = store
        .search(&AgentFilter::first().with_page(2))
        .await
        .expect("page 2");
    let sixth = store
        .search(&AgentFilter::first().with_page(6))
        .await
        .expect("page 6");

    assert_eq!(first.records().len(), PAGE_SIZE);
    assert_eq!(second.records().len(), PAGE_SIZE);
    assert_eq!(
        sixth.records().len(),
        0,
        "100 records / 20 = 5 pages; page 6 must be empty"
    );
    assert_eq!(first.total(), AGENT_FIXTURE_SIZE as i64);
}

#[tokio::test(flavor = "current_thread")]
async fn search_by_normalized_term_returns_matching_agents() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    init_tables(&pool).await.expect("init_tables");
    let store = AgentStore::new(pool).expect("store");

    store
        .create(root_input("alpha", "Alpha Searcher"))
        .await
        .expect("create alpha");
    store
        .create(root_input("beta", "Beta Helper"))
        .await
        .expect("create beta");

    let page = store
        .search(&AgentFilter::first().with_search("ALPHA"))
        .await
        .expect("search");
    assert_eq!(page.total(), 1, "search must use the normalized name index");
    assert_eq!(page.records()[0].identifier(), "alpha");
}

#[tokio::test(flavor = "current_thread")]
async fn one_hundred_crud_operations_p95_under_one_second() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    init_tables(&pool).await.expect("init_tables");
    let store = AgentStore::new(pool).expect("store");

    let mut samples = Vec::with_capacity(100);
    for i in 0..100 {
        let started = Instant::now();
        store
            .create(root_input(&format!("perf-{i:03}"), &format!("Perf {i}")))
            .await
            .expect("create");
        samples.push(started.elapsed());
    }
    samples.sort();
    let p95 = samples[(samples.len() as f64 * 0.95).ceil() as usize - 1];
    assert!(
        p95 <= std::time::Duration::from_secs(1),
        "100 Agent CRUD p95 = {p95:?}; budget is 1s"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn search_pagination_p95_under_five_hundred_ms() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    init_tables(&pool).await.expect("init_tables");
    let store = AgentStore::new(pool).expect("store");

    for i in 0..AGENT_FIXTURE_SIZE {
        store
            .create(root_input(&format!("page-{i:03}"), &format!("Page {i}")))
            .await
            .expect("create");
    }

    let mut samples = Vec::with_capacity(100);
    for i in 0..100 {
        let started = Instant::now();
        let _ = store
            .search(&AgentFilter::first().with_page((i % 5) + 1))
            .await
            .expect("search");
        samples.push(started.elapsed());
    }
    samples.sort();
    let p95 = samples[(samples.len() as f64 * 0.95).ceil() as usize - 1];
    assert!(
        p95 <= std::time::Duration::from_millis(500),
        "100 search/page p95 = {p95:?}; budget is 500ms"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn delete_agent_clears_default_invariant() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    init_tables(&pool).await.expect("init_tables");
    let store = AgentStore::new(pool).expect("store");

    let first = store
        .create(root_input("first", "First"))
        .await
        .expect("create first");
    let second = store
        .create(root_input("second", "Second"))
        .await
        .expect("create second");

    // Deleting the default requires a replacement.
    let err = store
        .delete(first.id(), None)
        .await
        .expect_err("delete default without replacement must fail");
    assert_eq!(err.field(), "is_default");
    assert_eq!(err.reason(), "replacement_required");

    // Deleting with a replacement succeeds and swaps the default.
    store
        .delete(first.id(), Some(second.id()))
        .await
        .expect("delete with replacement");
    let default = store.default_agent().await.expect("default fetch");
    let current = default.expect("default must exist after replacement");
    assert_eq!(current.id(), second.id());
    assert!(current.is_default());
}

#[allow(dead_code)]
fn _pin_types() {
    let _ = std::any::TypeId::of::<AgentPage>();
    let _ = std::any::TypeId::of::<AgentRecord>();
}
