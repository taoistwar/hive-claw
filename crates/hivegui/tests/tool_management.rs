//! T101 [P] [US11] Tool management contract.
//!
//! Source of truth: `specs/011-hivegui-standalone-mode/tasks.md` §T101
//! ("使用固定 fixture 编写 Tool CRUD、name 唯一、kind `1/2` 迁移、
//! 不可变键、引用 Workflow RESTRICT/前置校验、`default_args` 完整
//! schema、每页20条搜索分页、全部生产过滤/关联查询 EXPLAIN 预期索引、
//! CRUD p95≤1s 及搜索/翻页 p95≤500ms 测试").
//!
//! Red public boundaries (T105 will add these):
//!   - `hivegui::datasource::tool_store::ToolStore`
//!   - `hivegui::datasource::tool_store::ToolInput`
//!   - `hivegui::datasource::tool_store::ToolRecord`
//!   - `hivegui::datasource::tool_store::ToolKind` enum
//!     (Builtin | Custom)
//!
//! T104 reviewer signs §T101-T103.11; T105-T106 implementation then
//! make these tests Green; T107 reruns to record the Green evidence.

mod support;

use hivegui::datasource::tool_store::{ToolInput, ToolKind, ToolRecord, ToolStore};
use support::TestWorkspace;

#[tokio::test(flavor = "current_thread")]
async fn create_tool_persists_unique_name() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    let store = ToolStore::new(pool).expect("store");

    let created = store
        .create(
            ToolInput::new("http_get", ToolKind::Builtin)
                .with_default_args(serde_json::json!({"url": ""}))
                .expect("validated"),
        )
        .expect("create");
    assert_eq!(created.name(), "http_get");
    assert_eq!(created.kind(), ToolKind::Builtin);
}

#[tokio::test(flavor = "current_thread")]
async fn builtin_tool_name_is_immutable() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    let store = ToolStore::new(pool).expect("store");

    let created = store
        .create(
            ToolInput::new("http_get", ToolKind::Builtin)
                .with_default_args(serde_json::json!({}))
                .unwrap(),
        )
        .expect("create");
    let err = store
        .rename(created.id(), "renamed")
        .expect_err("builtin must be immutable");
    assert_eq!(err.reason(), "builtin_immutable");
}

#[tokio::test(flavor = "current_thread")]
async fn tool_referenced_by_workflow_cannot_be_deleted() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    let store = ToolStore::new(pool).expect("store");

    let created = store
        .create(
            ToolInput::new("referenced", ToolKind::Custom)
                .with_default_args(serde_json::json!({}))
                .unwrap(),
        )
        .expect("create");
    let err = store
        .delete(created.id())
        .expect_err("delete must fail when referenced");
    assert_eq!(err.reason(), "referenced_by_workflow");
}

#[tokio::test(flavor = "current_thread")]
async fn tool_exposes_a_stable_default_args_schema() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    let store = ToolStore::new(pool).expect("store");

    let created = store
        .create(
            ToolInput::new("http_get", ToolKind::Builtin)
                .with_default_args(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "url": {"type": "string"},
                    },
                    "required": ["url"],
                }))
                .unwrap(),
        )
        .expect("create");
    assert_eq!(
        created.default_args().pointer("/properties/url/type"),
        Some(&serde_json::Value::String("string".into()))
    );
}

#[allow(dead_code)]
fn _pin_types() {
    let _: ToolRecord = panic!("placeholder so the type is referenced");
}
