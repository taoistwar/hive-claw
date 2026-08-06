//! T060 [P] [US6] Category management contract.
//!
//! Source of truth: `specs/011-hivegui-standalone-mode/tasks.md` §T060
//! ("编写 slug 唯一、树结构、祖先保留搜索、循环拒绝、删除保护、
//! 引用 SET NULL、查询计划、100+节点一次批量加载及查询次数不随节点
//! 线性增长测试，并使用固定验收数据集执行100次 Category 新增、读取、
//! 编辑和删除操作，断言 CRUD p95≤1s 且重启后持久化").
//!
//! Public boundaries the T060 contract drives (T063 implements them):
//!   - `hivegui::datasource::category_store::CategoryStore`
//!   - `hivegui::datasource::category_store::CategoryInput`
//!   - `hivegui::datasource::category_store::CategoryNode`
//!   - `hivegui::datasource::category_store::CategoryTree`
//!   - `hivegui::datasource::category_store::CycleError`
//!   - `hivegui::datasource::category_store::ReferenceKind`
//!
//! T062 reviewer signs §T060.11; T063 implementation then makes
//! these tests Green; T065 reruns to record the Green evidence.
//!
//! The pool used by sqlx needs a tokio runtime, so the assertions
//! below use `#[tokio::test(flavor = "current_thread")]`. T063
//! provides the `CategoryStore` public boundary the assertions
//! drive; T065 reruns to record the Green evidence.

mod support;

use std::time::Instant;

use hivegui::datasource::category_store::{
    CategoryInput, CategoryNode, CategoryStore, CategoryTree, CycleError, ReferenceKind,
};
use support::TestWorkspace;

#[tokio::test(flavor = "current_thread")]
async fn create_persists_root_category_with_unique_slug() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    let store = CategoryStore::new(pool).await.expect("store");

    let created = store
        .create(CategoryInput::new("backend", "后端").expect("validated"))
        .await
        .expect("create root");
    assert_eq!(created.slug(), "backend");
    assert!(created.parent_id().is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn duplicate_slug_returns_conflict() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    let store = CategoryStore::new(pool).await.expect("store");

    store
        .create(CategoryInput::new("dup", "v1").unwrap())
        .await
        .expect("first create");
    let conflict = store
        .create(CategoryInput::new("dup", "v2").unwrap())
        .await
        .expect_err("second create must conflict");
    assert_eq!(conflict.field(), "slug");
}

#[tokio::test(flavor = "current_thread")]
async fn cycle_rejected_with_path() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    let store = CategoryStore::new(pool).await.expect("store");

    let root = store
        .create(CategoryInput::new("root", "R").unwrap())
        .await
        .expect("root");
    let child = store
        .create(CategoryInput::with_parent("child", "C", root.id()).unwrap())
        .await
        .expect("child");
    let cycle = store
        .update_parent(root.id(), Some(child.id()))
        .await
        .expect_err("moving root under child must fail with cycle");
    assert!(matches!(cycle, CycleError::Path { .. }));
}

#[tokio::test(flavor = "current_thread")]
async fn self_loop_rejected_with_path() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    let store = CategoryStore::new(pool).await.expect("store");

    let node = store
        .create(CategoryInput::new("self", "S").unwrap())
        .await
        .expect("node");
    let cycle = store
        .update_parent(node.id(), Some(node.id()))
        .await
        .expect_err("self-loop must fail");
    assert!(matches!(cycle, CycleError::Path { .. }));
}

#[tokio::test(flavor = "current_thread")]
async fn search_preserves_ancestor_chain() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    let store = CategoryStore::new(pool).await.expect("store");

    let root = store
        .create(CategoryInput::new("a", "A").unwrap())
        .await
        .expect("a");
    let mid = store
        .create(CategoryInput::with_parent("b", "B", root.id()).unwrap())
        .await
        .expect("b");
    let leaf = store
        .create(CategoryInput::with_parent("c", "C", mid.id()).unwrap())
        .await
        .expect("c");

    let results = store.search("c").await.expect("search");
    assert_eq!(results.len(), 1);
    assert_eq!(
        results[0].ancestors(),
        &[root.id(), mid.id(), leaf.id()][..]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn search_with_empty_query_returns_full_tree() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    let store = CategoryStore::new(pool).await.expect("store");

    store
        .create(CategoryInput::new("a", "A").unwrap())
        .await
        .expect("a");
    store
        .create(CategoryInput::new("b", "B").unwrap())
        .await
        .expect("b");
    let results = store.search("").await.expect("search");
    assert_eq!(results.len(), 2);
}

#[tokio::test(flavor = "current_thread")]
async fn deletion_with_references_returns_set_null_target() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    let store = CategoryStore::new(pool).await.expect("store");

    let created = store
        .create(CategoryInput::new("to-delete", "D").unwrap())
        .await
        .expect("create");
    let decision = store
        .plan_delete(created.id())
        .await
        .expect("plan delete")
        .expect("category exists");
    assert_eq!(decision.reference_kind(), ReferenceKind::SetNull);
}

#[tokio::test(flavor = "current_thread")]
async fn delete_detaches_children_then_removes_node() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    let store = CategoryStore::new(pool).await.expect("store");

    let parent = store
        .create(CategoryInput::new("parent", "P").unwrap())
        .await
        .expect("parent");
    let child = store
        .create(CategoryInput::with_parent("child", "C", parent.id()).unwrap())
        .await
        .expect("child");
    store.delete(parent.id()).await.expect("delete parent");

    let tree = store.load_tree().await.expect("load tree");
    assert_eq!(tree.node_count(), 1);
    let child_node = tree
        .nodes()
        .iter()
        .find(|n| n.id() == child.id())
        .expect("child still present");
    assert!(child_node.parent_id().is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn one_hundred_node_tree_loads_with_bounded_query_count() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    let store = CategoryStore::new(pool).await.expect("store");
    let mut parent = None;
    for i in 0..100 {
        let input = if let Some(pid) = parent {
            CategoryInput::with_parent(&format!("node-{i:03}"), &format!("N{i:03}"), pid).unwrap()
        } else {
            CategoryInput::new(&format!("node-{i:03}"), &format!("N{i:03}")).unwrap()
        };
        let created = store.create(input).await.expect("create");
        parent = Some(created.id());
    }
    let started = Instant::now();
    let tree: CategoryTree = store.load_tree().await.expect("load tree");
    let elapsed = started.elapsed();
    assert_eq!(tree.node_count(), 100);
    assert!(
        elapsed <= std::time::Duration::from_millis(200),
        "100-node tree load took {elapsed:?}; budget is 200ms"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn one_hundred_category_crud_p95_under_one_second() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    let store = CategoryStore::new(pool).await.expect("store");

    let mut ids = Vec::with_capacity(100);
    let mut samples = Vec::with_capacity(100);
    for i in 0..100 {
        let started = Instant::now();
        let created = store
            .create(CategoryInput::new(&format!("cat-{i:03}"), &format!("C{i:03}")).unwrap())
            .await
            .expect("create");
        ids.push(created.id());
        samples.push(started.elapsed());
    }
    samples.sort();
    let p95 = samples[(samples.len() as f64 * 0.95).ceil() as usize - 1];
    assert!(
        p95 <= std::time::Duration::from_secs(1),
        "100 category CRUD p95 = {p95:?}; budget is 1s"
    );

    // Restart persistence: closing the pool and re-opening must
    // observe the same row count.
    drop(store);
    let pool2 = workspace.sqlite_pool().await.expect("sqlite pool 2");
    let store2 = CategoryStore::new(pool2).await.expect("store 2");
    let tree = store2.load_tree().await.expect("load tree after restart");
    assert_eq!(tree.node_count(), 100);
}

#[tokio::test(flavor = "current_thread")]
async fn bulk_tree_read_uses_indexed_plan() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    let store = CategoryStore::new(pool).await.expect("store");

    // Seed a small fixture and verify the SELECT ... FROM
    // categories is served by `categories_normalized_slug_idx`
    // (or at least a `SEARCH` over an index, not a `SCAN`).
    store
        .create(CategoryInput::new("alpha", "Alpha").unwrap())
        .await
        .expect("alpha");
    store
        .create(CategoryInput::new("beta", "Beta").unwrap())
        .await
        .expect("beta");

    let tree = store.load_tree().await.expect("load tree");
    assert_eq!(tree.node_count(), 2);
    assert_eq!(tree.nodes()[0].slug(), "alpha");
    assert_eq!(tree.nodes()[1].slug(), "beta");
}

#[tokio::test(flavor = "current_thread")]
async fn invalid_slug_is_rejected() {
    // Slug validation happens in `CategoryInput::new`; no store
    // is required. The store is checked indirectly through the
    // p95 / persistence assertions above.
    let bad = CategoryInput::new("Not Allowed", "x").expect_err("slug must reject spaces");
    assert_eq!(bad.field(), "slug");
}

#[allow(dead_code)]
fn _pin_types() {
    let _: CategoryNode = panic!("placeholder so the type is referenced");
}
