//! 「提示词管理」契约：可复用提示词的 CRUD、数据根补表，以及调试页把提示词
//! 填进 message 的交互。
//!
//! 覆盖三层：
//!   1. `PromptTemplate`（`prompts` 表）读写与校验；
//!   2. 已被标记 v4 的旧库在打开时自动补建 `prompts` 表；
//!   3. 视图交互：提示词管理页的编辑/删除，以及提示词调试页 message 的
//!      「选择提示词」。

mod support;

use gpui_kit::{AppContext, TestAppContext, VisualTestContext, WindowHandle, px, size};
use hivegui::config::AppIdentity;
use hivegui::datasource::Store;
use hivegui::datasource::entity_store::PromptTemplate;

// ---------------------------------------------------------------------------
// 数据层
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn prompt_crud_round_trips_and_validates_input() {
    let directory = tempfile::tempdir().expect("temporary Store directory");
    let store = Store::new(directory.path()).await.expect("open Store");

    let created = PromptTemplate::create(
        store.pool(),
        "总结".to_string(),
        "请总结下面的内容：{{input}}".to_string(),
        Some("通用总结".to_string()),
    )
    .await
    .expect("create prompt");
    assert_eq!(created.name, "总结");
    assert_eq!(created.description.as_deref(), Some("通用总结"));

    assert_eq!(
        PromptTemplate::list(store.pool(), None, 50, 0)
            .await
            .expect("list")
            .len(),
        1
    );
    assert_eq!(
        PromptTemplate::list(store.pool(), Some("总结".to_string()), 50, 0)
            .await
            .expect("search by name")
            .len(),
        1
    );
    assert_eq!(
        PromptTemplate::list(store.pool(), Some("{{input}}".to_string()), 50, 0)
            .await
            .expect("search by content")
            .len(),
        1
    );
    assert!(
        PromptTemplate::list(store.pool(), Some("不存在".to_string()), 50, 0)
            .await
            .expect("search miss")
            .is_empty()
    );
    assert_eq!(
        PromptTemplate::count(store.pool(), None)
            .await
            .expect("count"),
        1
    );

    let updated = PromptTemplate::update(
        store.pool(),
        created.id,
        "总结 v2".to_string(),
        "新内容".to_string(),
        None,
    )
    .await
    .expect("update prompt");
    assert_eq!(updated.name, "总结 v2");
    assert!(updated.description.is_none());

    // 名称与内容都必须非空（写入边界校验）。
    let empty_name = PromptTemplate::create(store.pool(), "  ".to_string(), "x".to_string(), None)
        .await
        .expect_err("blank name must be rejected");
    assert!(empty_name.to_string().contains("name"), "{empty_name}");
    let empty_content =
        PromptTemplate::create(store.pool(), "n".to_string(), "   ".to_string(), None)
            .await
            .expect_err("blank content must be rejected");
    assert!(
        empty_content.to_string().contains("content"),
        "{empty_content}"
    );

    PromptTemplate::delete(store.pool(), created.id)
        .await
        .expect("delete prompt");
    assert!(
        PromptTemplate::get(store.pool(), created.id)
            .await
            .expect("get deleted")
            .is_none()
    );
    assert_eq!(
        PromptTemplate::count(store.pool(), None)
            .await
            .expect("count"),
        0
    );
}

/// 已经标记为 v4、但建表时还没有 `prompts` 的老库：打开路径必须补建这张表，
/// 否则提示词管理与调试页的提示词选择会报 "no such table: prompts"。
#[tokio::test(flavor = "current_thread")]
async fn opening_a_legacy_v4_store_recreates_the_prompts_table() {
    let directory = tempfile::tempdir().expect("temporary Store directory");
    {
        let store = Store::new(directory.path()).await.expect("open Store");
        sqlx::query("DROP TABLE prompts")
            .execute(store.pool())
            .await
            .expect("simulate a pre-prompts v4 database");
    }

    let reopened = Store::new(directory.path()).await.expect("reopen Store");
    let created = PromptTemplate::create(
        reopened.pool(),
        "修复后可用".to_string(),
        "内容".to_string(),
        None,
    )
    .await
    .expect("prompts table must be recreated on open");
    assert_eq!(created.name, "修复后可用");
}

// ---------------------------------------------------------------------------
// 视图辅助
// ---------------------------------------------------------------------------

fn init_gpui(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::component::theme::init(cx);
        gpui_kit::component::init(cx);
    });
}

fn visual_store(
    cx: &mut TestAppContext,
) -> (
    support::TestWorkspace,
    Store,
    gpui_kit::Entity<Store>,
    tokio::runtime::Runtime,
) {
    use hivegui::datasource::store::StoreOpenOptions;

    init_gpui(cx);
    cx.executor().allow_parking();
    let workspace = support::TestWorkspace::new().expect("create visual-test workspace");
    let runtime = tokio::runtime::Runtime::new().expect("create visual-test runtime");
    let store = runtime
        .block_on(Store::open_local(StoreOpenOptions::new(
            workspace.database_path(),
            workspace.plugin_root(),
        )))
        .expect("open canonical visual-test Store");
    let store_entity = cx.new(|_| store.clone());
    (workspace, store, store_entity, runtime)
}

fn click_selector(visual: &mut VisualTestContext, selector: &str) -> bool {
    let selector: &'static str = Box::leak(selector.to_string().into_boxed_str());
    let Some(bounds) = visual.debug_bounds(selector) else {
        return false;
    };
    visual.simulate_click(bounds.center(), gpui_kit::Modifiers::default());
    visual.run_until_parked();
    true
}

fn settle_selector(visual: &mut VisualTestContext, selector: &str) -> bool {
    let selector: &'static str = Box::leak(selector.to_string().into_boxed_str());
    for _ in 0..60 {
        visual.run_until_parked();
        if visual.debug_bounds(selector).is_some() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    false
}

fn selector_absent(visual: &mut VisualTestContext, selector: &str) -> bool {
    let selector: &'static str = Box::leak(selector.to_string().into_boxed_str());
    for _ in 0..60 {
        visual.run_until_parked();
        if visual.debug_bounds(selector).is_none() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    false
}

// ---------------------------------------------------------------------------
// 提示词管理页
// ---------------------------------------------------------------------------

#[gpui_kit::test]
fn prompt_library_view_edits_and_deletes_a_seeded_prompt(cx: &mut TestAppContext) {
    use hivegui::ui::prompt_library_view::PromptLibraryView;

    let (_workspace, store, store_entity, runtime) = visual_store(cx);
    let seeded = runtime
        .block_on(PromptTemplate::create(
            store.pool(),
            "种子提示词".to_string(),
            "种子内容".to_string(),
            Some("种子描述".to_string()),
        ))
        .expect("seed prompt");
    let _runtime_guard = runtime.enter();

    let window = cx.open_window(size(px(1000.0), px(700.0)), move |window, cx| {
        let view = cx.new(|cx| PromptLibraryView::new(store_entity, cx));
        gpui_kit::component::Root::new(view, window, cx).bordered(false)
    });
    cx.run_until_parked();
    let mut visual = VisualTestContext::from_window(window.into(), cx);

    let row_selector = format!("PROMPT_ROW-{}", seeded.id);
    let edit_selector = format!("PROMPT_EDIT-{}", seeded.id);
    let delete_selector = format!("PROMPT_DELETE-{}", seeded.id);

    let listed = settle_selector(&mut visual, &row_selector);
    let edit_opened =
        click_selector(&mut visual, &edit_selector) && settle_selector(&mut visual, "PROMPT_MODAL");
    let saved = click_selector(&mut visual, "PROMPT_FORM_SAVE")
        && selector_absent(&mut visual, "PROMPT_MODAL");
    let listed_again = settle_selector(&mut visual, &row_selector);
    let confirm_opened = click_selector(&mut visual, &delete_selector)
        && settle_selector(&mut visual, "PROMPT_DELETE_CONFIRM");
    let deleted = click_selector(&mut visual, "PROMPT_DELETE_CONFIRM")
        && selector_absent(&mut visual, &row_selector);
    // 删除后的异步重载必须在本测试仍持有 Tokio 上下文时跑完。
    for _ in 0..40 {
        std::thread::sleep(std::time::Duration::from_millis(5));
        visual.run_until_parked();
    }
    let remaining = runtime
        .block_on(PromptTemplate::count(store.pool(), None))
        .expect("count after delete");
    visual.update(|window, _| window.remove_window());
    visual.run_until_parked();

    assert!(listed, "seeded prompt must show up in the管理列表");
    assert!(edit_opened, "PROMPT_EDIT must open the edit modal");
    assert!(saved, "PROMPT_FORM_SAVE must close the modal");
    assert!(listed_again, "the list must stay rendered after saving");
    assert!(confirm_opened, "PROMPT_DELETE must open the confirm modal");
    assert!(deleted, "PROMPT_DELETE_CONFIRM must remove the row");
    assert_eq!(remaining, 0, "the deleted prompt must be gone from storage");
}

#[gpui_kit::test]
fn prompt_library_view_creates_a_prompt_from_the_form(cx: &mut TestAppContext) {
    use hivegui::ui::prompt_library_view::PromptLibraryView;

    let (_workspace, store, store_entity, runtime) = visual_store(cx);
    let _runtime_guard = runtime.enter();

    let window = cx.open_window(size(px(1000.0), px(700.0)), move |window, cx| {
        let view = cx.new(|cx| PromptLibraryView::new(store_entity, cx));
        gpui_kit::component::Root::new(view, window, cx).bordered(false)
    });
    cx.run_until_parked();
    let mut visual = VisualTestContext::from_window(window.into(), cx);

    let add_opened =
        click_selector(&mut visual, "PROMPT_ADD") && settle_selector(&mut visual, "PROMPT_MODAL");
    // 名称：Input
    let name_clicked = click_selector(&mut visual, "PROMPT_NAME");
    if name_clicked {
        visual.simulate_keystrokes("ctrl-a");
        visual.simulate_input("新提示词");
        visual.run_until_parked();
    }
    // 内容：Textarea
    let content_clicked = click_selector(&mut visual, "PROMPT_CONTENT");
    if content_clicked {
        visual.simulate_keystrokes("ctrl-a");
        visual.simulate_input("请翻译：{{input}}");
        visual.run_until_parked();
    }
    let saved = click_selector(&mut visual, "PROMPT_FORM_SAVE")
        && selector_absent(&mut visual, "PROMPT_MODAL");
    let listed = settle_selector(&mut visual, "PROMPT_LIST_LOADED");
    for _ in 0..40 {
        std::thread::sleep(std::time::Duration::from_millis(5));
        visual.run_until_parked();
    }
    let rows = runtime
        .block_on(PromptTemplate::list(store.pool(), None, 50, 0))
        .expect("list after create");
    visual.update(|window, _| window.remove_window());
    visual.run_until_parked();

    assert!(add_opened, "PROMPT_ADD must open the create modal");
    assert!(
        name_clicked && content_clicked,
        "form fields must be clickable"
    );
    assert!(saved, "PROMPT_FORM_SAVE must close the create modal");
    assert!(listed, "the created prompt must render in the list");
    assert_eq!(rows.len(), 1, "the created prompt must be persisted");
    assert_eq!(rows[0].name, "新提示词");
    assert_eq!(rows[0].content, "请翻译：{{input}}");
}

// ---------------------------------------------------------------------------
// 提示词调试页：message 可选择提示词
// ---------------------------------------------------------------------------

fn open_prompt_debugger_visual_window(
    cx: &mut TestAppContext,
    store: gpui_kit::Entity<Store>,
    llm_store: hivegui::datasource::llm_store::LlmStore,
    identity: AppIdentity,
    window_size: gpui_kit::Size<gpui_kit::Pixels>,
) -> WindowHandle<gpui_kit::component::Root> {
    cx.open_window(window_size, move |window, cx| {
        let view = cx.new(|cx| {
            hivegui::ui::prompt_debugger::PromptDebugger::new(cx, store, llm_store, identity)
        });
        gpui_kit::component::Root::new(view, window, cx).bordered(false)
    })
}

#[gpui_kit::test]
fn prompt_debugger_message_can_pick_a_managed_prompt(cx: &mut TestAppContext) {
    use hivegui::datasource::llm_store::LlmStore;

    let (_workspace, store, store_entity, runtime) = visual_store(cx);
    let seeded = runtime
        .block_on(PromptTemplate::create(
            store.pool(),
            "系统提示".to_string(),
            "你是一个严谨的助手。".to_string(),
            None,
        ))
        .expect("seed prompt");
    let llm_store = LlmStore::new(store.pool().clone(), store.crypto().clone());
    let _runtime_guard = runtime.enter();

    let window = open_prompt_debugger_visual_window(
        cx,
        store_entity.clone(),
        llm_store.clone(),
        AppIdentity::NGY_PROMPT_STUDIO,
        size(px(1280.0), px(900.0)),
    );
    cx.run_until_parked();
    let mut visual = VisualTestContext::from_window(window.into(), cx);

    // 第一条 message 的内置 id 是 1（见 PromptDebugger::new_unloaded 的初始消息）。
    let edit_opened = click_selector(&mut visual, "PROMPT_MSG_EDIT-1");
    let picker_button_present = settle_selector(&mut visual, "PROMPT_MSG_PICKER-1");
    let picker_opened = click_selector(&mut visual, "PROMPT_MSG_PICKER-1")
        && settle_selector(&mut visual, "PROMPT_PICKER");
    let item_selector = format!("PROMPT_PICKER_ITEM-{}", seeded.id);
    let item_selected =
        settle_selector(&mut visual, &item_selector) && click_selector(&mut visual, &item_selector);
    let picker_closed = selector_absent(&mut visual, "PROMPT_PICKER");
    visual.update(|window, _| window.remove_window());
    visual.run_until_parked();

    assert!(
        edit_opened,
        "PROMPT_MSG_EDIT must enter the message edit state"
    );
    assert!(
        picker_button_present,
        "the edit state must expose the「选择提示词」button"
    );
    assert!(picker_opened, "the button must open the prompt picker");
    assert!(item_selected, "the managed prompt must be selectable");
    assert!(picker_closed, "selecting a prompt must close the picker");

    // 桌面版没有提示词管理入口，同一个编辑态里不渲染「选择提示词」。
    let desktop_window = open_prompt_debugger_visual_window(
        cx,
        store_entity,
        llm_store,
        AppIdentity::HIVEGUI,
        size(px(1280.0), px(900.0)),
    );
    cx.run_until_parked();
    let mut desktop_visual = VisualTestContext::from_window(desktop_window.into(), cx);
    let desktop_edit_opened = click_selector(&mut desktop_visual, "PROMPT_MSG_EDIT-1");
    let desktop_picker_absent = desktop_visual.debug_bounds("PROMPT_MSG_PICKER-1").is_none();
    desktop_visual.update(|window, _| window.remove_window());
    desktop_visual.run_until_parked();

    assert!(
        desktop_edit_opened,
        "the desktop debugger must still enter the message edit state"
    );
    assert!(
        desktop_picker_absent,
        "the desktop debugger must not expose a prompt-library picker"
    );
}
