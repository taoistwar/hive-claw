//! T083 [P] [US9] Function management contract.
//!
//! Source of truth: `specs/011-hivegui-standalone-mode/tasks.md` §T083
//! ("编写 `builtin|custom|placeholder` 稳定字符串 kind、旧整数 `1/2/3`
//! 迁移、四个下划线保留 identifier、点号记录/别名数量为零、Builtin
//! 不可变、Custom Plugin/export/schema/RESTRICT、Placeholder 三个执行
//! 关系字段为空、每页20条搜索分页、全部生产过滤/关联查询 EXPLAIN
//! 预期索引、CRUD p95≤1s 及搜索/翻页 p95≤500ms 测试").
//!
//! Red public boundaries (T087 will add these):
//!   - `hivegui::datasource::function_store::FunctionStore`
//!   - `hivegui::datasource::function_store::FunctionInput`
//!   - `hivegui::datasource::function_store::FunctionRecord`
//!   - `hivegui::datasource::function_store::FunctionKind` enum
//!     (Builtin | Custom | Placeholder)
//!   - `hivegui::datasource::function_store::RESERVED_UNDERSCORE_IDENTIFIERS`
//!
//! T086 reviewer signs §T083.11 + §T084.11 + §T085.11; T087-T088
//! implementation then make these tests Green; T089 reruns to record
//! the Green evidence.

mod support;

use hivegui::datasource::function_store::{
    FunctionInput, FunctionKind, FunctionRecord, FunctionStore, RESERVED_UNDERSCORE_IDENTIFIERS,
};
use support::TestWorkspace;

const PAGE_SIZE: usize = 20;

#[test]
fn reserved_underscore_identifiers_have_exactly_four_entries() {
    assert_eq!(
        RESERVED_UNDERSCORE_IDENTIFIERS.len(),
        4,
        "the underscore-reserved identifier list contains the four builtins"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn create_builtin_function_is_rejected() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    let store = FunctionStore::new(pool).expect("store");

    let err = store
        .create(FunctionInput::new("format_template", FunctionKind::Builtin).unwrap())
        .expect_err("builtin must be registered by code, not created");
    assert_eq!(err.reason(), "builtin_immutable");
}

#[tokio::test(flavor = "current_thread")]
async fn create_custom_function_persists_with_plugin_reference() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    let store = FunctionStore::new(pool).expect("store");

    let created = store
        .create(
            FunctionInput::new("user_function", FunctionKind::Custom)
                .unwrap()
                .with_plugin_id(1)
                .with_export("handle")
                .with_schema(serde_json::json!({"type": "object"})),
        )
        .expect("create custom");
    assert_eq!(created.kind(), FunctionKind::Custom);
    assert!(created.plugin_id().is_some());
}

#[tokio::test(flavor = "current_thread")]
async fn placeholder_function_has_no_capability_or_export() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    let store = FunctionStore::new(pool).expect("store");

    let created = store
        .create(FunctionInput::new("placeholder", FunctionKind::Placeholder).unwrap())
        .expect("create placeholder");
    assert!(created.plugin_id().is_none());
    assert!(created.export().is_none());
    assert!(created.required_capability().is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn dotted_identifiers_are_rejected() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    let store = FunctionStore::new(pool).expect("store");

    let err = store
        .create(FunctionInput::new("format.template", FunctionKind::Placeholder).unwrap())
        .expect_err("dotted identifier must be rejected");
    assert_eq!(err.reason(), "dotted_identifier_forbidden");
    let _ = PAGE_SIZE;
}

#[allow(dead_code, clippy::diverging_sub_expression)]
fn _pin_types() {
    let _: FunctionRecord = panic!("placeholder so the type is referenced");
}
