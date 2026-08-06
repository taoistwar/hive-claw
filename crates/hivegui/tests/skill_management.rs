//! T108 [P] [US12] Skill management contract.
//!
//! Source of truth: `specs/011-hivegui-standalone-mode/tasks.md` §T108
//! ("使用固定 fixture 编写 Skill CRUD、name 唯一、content 不可空、
//! 引用 Plugin/RESTRICT 或 Capability SET NULL、每页20条搜索分页、
//! 全部生产过滤/关联查询 EXPLAIN 预期索引、CRUD p95≤1s 及搜索/
//! 翻页 p95≤500ms 测试").
//!
//! Red public boundaries (T112 will add these):
//!   - `hivegui::datasource::skill_store::SkillStore`
//!   - `hivegui::datasource::skill_store::SkillInput`
//!   - `hivegui::datasource::skill_store::SkillRecord`
//!
//! T111 reviewer signs §T108-T110.11; T112-T113 implementation then
//! make these tests Green; T114 reruns to record the Green evidence.

mod support;

use hivegui::datasource::skill_store::{SkillInput, SkillRecord, SkillStore};
use support::TestWorkspace;

#[tokio::test(flavor = "current_thread")]
async fn create_skill_persists_unique_name_with_content() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    let store = SkillStore::new(pool).expect("store");

    let created = store
        .create(
            SkillInput::new(
                "summarise",
                "Summarise the conversation into 3 bullet points",
            )
            .expect("validated"),
        )
        .expect("create");
    assert_eq!(created.name(), "summarise");
    assert!(!created.content().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn empty_content_is_rejected() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    let store = SkillStore::new(pool).expect("store");

    let err = store
        .create(SkillInput::new("empty", "").unwrap())
        .expect_err("empty content must be rejected");
    assert_eq!(err.field(), "content");
}

#[tokio::test(flavor = "current_thread")]
async fn duplicate_skill_name_returns_conflict() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    let store = SkillStore::new(pool).expect("store");

    store
        .create(SkillInput::new("dup", "first").unwrap())
        .expect("first create");
    let conflict = store
        .create(SkillInput::new("dup", "second").unwrap())
        .expect_err("second create must conflict");
    assert_eq!(conflict.field(), "name");
}

#[allow(dead_code)]
fn _pin_types() {
    let _: SkillRecord = panic!("placeholder so the type is referenced");
}
