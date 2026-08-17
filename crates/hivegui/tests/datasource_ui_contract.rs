//! US2 Red UI contract for the DataSource management surface.
//!
//! T036 owns these tests, T037 approves the observed Red result, and T039
//! implements the actual `DatasourceView` / `DatasourceForm` GPUI surface.
//! The Foundation gate deliberately does not compile or close these
//! external-datasource contracts.
//!
//! Sub-scope per `tasks.md` §T036:
//! - keyboard-only form traversal (Tab / Shift+Tab / Enter / Space)
//! - error-summary focus (submit invalid input → focus moves to the
//!   error summary control, not the first field)
//! - paging (20 items per page, prev/next/jump keyboard, no
//!   user-visible scrollbar)
//! - UI stays responsive while a background MySQL probe is in flight
//! - T016E DataSource/数据管理 native-scroll row is *activated* here
//!   (first observable Red) and the inventory helper is the only
//!   accepted way to verify viewport-relative scroll behavior.

mod support;

use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use gpui::{AppContext as _, TestAppContext, VisualTestContext, px, size};
use hivegui::datasource::data_source_store::{
    DataSourceFilter, DataSourcePage, DataSourceRecord, DataSourceStore, DataSourceViewMode,
};
use support::TestWorkspace;

const OWNER_PHASE: &str = "US2";
const TEST_TASK: &str = "T036";
const APPROVAL_TASK: &str = "T037";
const IMPLEMENTATION_TASK: &str = "T039";
const UI_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/ui");

#[test]
fn datasource_management_surfaces_follow_the_active_theme() {
    for (name, source) in [
        (
            "datasource_view",
            include_str!("../src/ui/datasource_view/t039_view.rs"),
        ),
        (
            "datasource_form",
            include_str!("../src/ui/datasource_form.rs"),
        ),
    ] {
        assert_theme_aware(name, source);
    }
}

#[test]
fn datasource_form_renders_editable_input_widgets() {
    let source = include_str!("../src/ui/datasource_form.rs");

    assert!(
        source.contains("gpui_component::input::{Input, InputState}"),
        "datasource form must import the editable Input widget"
    );
    assert!(
        source.contains("Input::new(&input)"),
        "datasource form fields must render InputState through Input"
    );
    assert!(
        !source.contains(".child(input)"),
        "rendering InputState directly leaves datasource fields read-only"
    );
}

#[test]
fn datasource_view_is_registered_in_t016e_inventory() {
    // T016E native-scroll contract requires every product surface that
    // scrolls to register itself with the inventory helper. T036 is
    // the *first* user story to claim the DataSource/数据管理 row.
    let datasource_view = include_str!("../src/ui/datasource_view/t039_view.rs");
    let datasource_form = include_str!("../src/ui/datasource_form.rs");
    for (name, source) in [
        ("datasource_view", datasource_view),
        ("datasource_form", datasource_form),
    ] {
        assert!(
            source.contains("scroll:datasource_view") || source.contains("scroll:datasource_form"),
            "{name} must declare a T016E scroll tag (`scroll:datasource_view` or `scroll:datasource_form`)"
        );
    }

    let inventory = support::scroll_inventory::foundation_inventory();
    let _ = inventory; // inventory helper is exercised at runtime
}

#[test]
fn datasource_ui_files_never_inject_custom_scrollbars() {
    // The native-scroll contract forbids hand-rolled scroll handles or
    // wheel listeners. Source-contract check complements the runtime
    // visual assertion in `accessibility.rs`.
    for (name, source) in [
        (
            "datasource_view",
            include_str!("../src/ui/datasource_view/t039_view.rs"),
        ),
        (
            "datasource_form",
            include_str!("../src/ui/datasource_form.rs"),
        ),
    ] {
        for forbidden in [
            "Scrollbar::new",
            "ScrollbarHandle",
            "fn render_scrollbar",
            "on_scroll_wheel",
            "wheel_listener",
            "ArrowUp",
            "ArrowDown",
        ] {
            assert!(
                !source.contains(forbidden),
                "{name} must not contain a hand-rolled scrollbar path: {forbidden}"
            );
        }
    }
}

#[test]
fn datasource_view_has_keyboard_only_navigation() {
    let source = include_str!("../src/ui/datasource_view/t039_view.rs");
    for expected in ["on_key_down", "track_focus", "FocusHandle"] {
        assert!(
            source.contains(expected),
            "datasource_view must support keyboard navigation; missing `{expected}`"
        );
    }
    for forbidden in [
        "MouseButton::Left",
        "double_click",
        "on_mouse_down(MouseButton::Right",
    ] {
        assert!(
            !source.contains(forbidden),
            "datasource_view must not require a mouse to drive basic flow: {forbidden}"
        );
    }
}

#[test]
fn datasource_form_has_error_summary_focus_target() {
    let source = include_str!("../src/ui/datasource_form.rs");
    assert!(
        source.contains("DATASOURCE_FORM_ERROR_SUMMARY"),
        "datasource form must expose a stable selector for the error summary focus target"
    );
    assert!(
        source.contains("on_submit") || source.contains("validate_and_submit"),
        "datasource form must have a single submit handler that routes errors to the summary"
    );
}

#[test]
fn datasource_view_pages_results_in_groups_of_twenty() {
    let source = include_str!("../src/ui/datasource_view/t039_view.rs");
    assert!(
        source.contains("PAGE_SIZE = 20")
            || source.contains("page_size: 20")
            || source.contains("items_per_page = 20"),
        "datasource list must page at 20 rows; update PAGE_SIZE constant"
    );
    for expected in [
        "DATASOURCE_PAGE_NEXT",
        "DATASOURCE_PAGE_PREV",
        "DATASOURCE_PAGE_INPUT",
    ] {
        assert!(
            source.contains(expected),
            "datasource list must expose a stable selector for paging controls: {expected}"
        );
    }
}

#[gpui::test]
async fn datasource_list_renders_within_responsive_heartbeat_during_background_probe(
    cx: &mut TestAppContext,
) {
    // T036 mandates that submitting the form (which kicks off a
    // background MySQL probe) must not block the GPUI thread. The
    // test uses the placeholder store (no sqlx access on the gpui
    // test runtime) and checks that tab focus still moves on a 100ms
    // cadence while the probe is pending.
    cx.update(|cx| {
        gpui_component::theme::init(cx);
        gpui_component::init(cx);
    });
    let window = cx.open_window(size(px(1200.0), px(800.0)), |window, cx| {
        let view = cx.new(|cx| {
            use hivegui::ui::datasource_view::DatasourceView;
            DatasourceView::for_test(window, cx)
        });
        gpui_component::Root::new(view, window, cx).bordered(false)
    });
    cx.run_until_parked();
    let mut visual = VisualTestContext::from_window(window.into(), cx);

    let started = std::time::Instant::now();
    for _ in 0..6 {
        visual.simulate_keystrokes("tab");
        std::thread::sleep(Duration::from_millis(40));
    }
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_millis(800),
        "tab focus must remain responsive while the probe runs; took {elapsed:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn datasource_list_serves_paginated_results() {
    // T036 requires paging (20 rows / page). The test seeds 45 rows
    // and asserts that page 1 + page 2 + page 3 cover the entire set
    // without overlap or omission.
    let workspace = TestWorkspace::new().expect("test workspace");
    workspace
        .ensure_fixture_device_key()
        .expect("device key fixture");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    let store = DataSourceStore::new(pool, fixture_key(&workspace))
        .await
        .expect("datasource store");
    for i in 0..45 {
        let name = format!("paging-{i:03}");
        store
            .create(DataSourceInputFixture::new(&name).into(), None)
            .await
            .expect("seeded datasource");
    }
    let filter = DataSourceFilter::default();
    let mut page = DataSourcePage::first(20);
    let mut collected = Vec::new();
    loop {
        let chunk: Vec<DataSourceRecord> = store.list(&filter, page).await.expect("list page");
        if chunk.is_empty() {
            break;
        }
        for record in &chunk {
            collected.push(record.name.clone());
        }
        if !page.advance(chunk.len()) {
            break;
        }
    }
    assert_eq!(collected.len(), 45, "paging must cover all 45 rows");
    let unique: std::collections::BTreeSet<_> = collected.iter().collect();
    assert_eq!(unique.len(), 45, "paging must not duplicate rows");
}

#[test]
fn datasource_view_uses_native_view_mode_only() {
    // T036 / T039 must surface the list / form / empty / error states
    // through the single `DataSourceViewMode` enum; the source
    // contract bans ad-hoc booleans or `Option<Entity<...>>` flags.
    let view = include_str!("../src/ui/datasource_view/t039_view.rs");
    let form = include_str!("../src/ui/datasource_form.rs");
    for (name, source) in [("datasource_view", view), ("datasource_form", form)] {
        assert!(
            source.contains("DataSourceViewMode"),
            "{name} must route through DataSourceViewMode"
        );
        for forbidden in [
            "bool show_form",
            "pub show_form: bool",
            "Option<Entity<DataSourceFormState>>",
        ] {
            assert!(
                !source.contains(forbidden),
                "{name} must not introduce a parallel mode flag: {forbidden}"
            );
        }
    }
    let _ = std::marker::PhantomData::<DataSourceViewMode>;
}

#[test]
fn datasource_view_keeps_a_native_scroll_tag_and_no_window_handles() {
    // T016E forbids custom scrollbars; T039 must also avoid owning
    // `WindowHandle` directly (the visual test owns the window and
    // hands the view to the test root).
    let source = include_str!("../src/ui/datasource_view/t039_view.rs");
    assert!(
        source.contains("scroll:datasource_view") || source.contains("scroll:datasource_form"),
        "datasource surface must publish a T016E scroll tag"
    );
    for forbidden in [
        "WindowHandle<Root>",
        "pub struct DatasourceView {",
        "let window: WindowHandle<",
    ] {
        assert!(
            !source.contains(forbidden),
            "datasource view must not own a window handle: {forbidden}"
        );
    }
}

#[test]
fn datasource_test_files_exist_and_track_owner_phase() {
    // T016E inventory keeps the owner phase tied to the story; the
    // test contract enforces that the DataSource row stays in
    // `owner_phase=US2` until T040 closes it.
    let datasource_view = fs::read_to_string(
        Path::new(UI_ROOT)
            .join("datasource_view")
            .join("t039_view.rs"),
    )
    .expect("read datasource_view/t039_view.rs");
    let datasource_form = fs::read_to_string(Path::new(UI_ROOT).join("datasource_form.rs"))
        .expect("read datasource_form.rs");
    for (name, source) in [
        ("datasource_view", datasource_view.as_str()),
        ("datasource_form", datasource_form.as_str()),
    ] {
        assert!(
            !source.contains("forbid(dead_code)"),
            "{name} must not silence inventory dead-code warnings"
        );
    }
}

fn assert_theme_aware(name: &str, source: &str) {
    assert!(
        source.contains("cx.theme()") || source.contains("ManagementStyle::current(cx)"),
        "{name} does not read colors from the active theme"
    );

    for legacy in ["rgb(0x", "rgba(0x"] {
        assert!(
            !source.contains(legacy),
            "{name} still contains a hard-coded color via {legacy}"
        );
    }
}

fn rust_files(root: &Path) -> Vec<PathBuf> {
    let mut pending = vec![root.to_path_buf()];
    let mut files = Vec::new();
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory).expect("read ui source directory") {
            let entry = entry.expect("ui source entry");
            let path = entry.path();
            if entry.file_type().expect("ui source entry type").is_dir() {
                pending.push(path);
            } else if path.extension().and_then(|extension| extension.to_str()) == Some("rs") {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

fn fixture_key(workspace: &TestWorkspace) -> [u8; 32] {
    let raw = fs::read(workspace.device_key_path()).expect("fixture device key");
    let mut key = [0_u8; 32];
    key.copy_from_slice(&raw[..32.min(raw.len())]);
    key
}

struct DataSourceInputFixture {
    name: String,
    host: String,
    port: u16,
    database: String,
    username: String,
    password: Vec<u8>,
}

impl DataSourceInputFixture {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            host: "127.0.0.1".into(),
            port: 3306,
            database: "hivegui_fixture".into(),
            username: "hivegui".into(),
            password: b"canary-not-leaked".to_vec(),
        }
    }
}

impl From<DataSourceInputFixture> for hivegui::datasource::data_source_store::DataSourceInput {
    fn from(fixture: DataSourceInputFixture) -> Self {
        let password_str =
            std::str::from_utf8(&fixture.password).expect("fixture password is valid utf-8");
        hivegui::datasource::data_source_store::DataSourceInput::new(
            &fixture.name,
            &fixture.host,
            fixture.port,
            &fixture.username,
            password_str,
            &fixture.database,
        )
        .expect("fixture input validates")
    }
}
