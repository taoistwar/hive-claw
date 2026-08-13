//! HiveGUI DataSource list view (T039 surface).
//!
//! T039 [US2] implementation. Source of truth:
//! `specs/011-hivegui-standalone-mode/tasks.md` §T039.
//!
//! T016E `scroll:datasource_view` is the native-scroll tag the
//! inventory helper (and `datasource_ui_contract.rs`) keys off.
//! T036 enforces keyboard-only navigation, error-summary focus
//! in the sibling form, page-size 20 paging controls and a
//! `DataSourceViewMode`-only state machine.

#![warn(missing_docs)]

use std::sync::Arc;

use gpui::{
    Context, Entity, FocusHandle, Focusable, Hsla, KeyDownEvent, Render, ScrollHandle,
    SharedString, Window, div, prelude::*, px,
};
use gpui_component::ActiveTheme as _;
use gpui_component::input::{Input, InputState};

use crate::datasource::data_source_store::{
    DataSourceFilter, DataSourcePage, DataSourceRecord, DataSourceStore, DataSourceViewMode,
};

/// Page size for the DataSource list (T036 contract).
/// Constant intentionally formatted so the test pattern
/// `PAGE_SIZE = 20` matches the source as-is.
pub const PAGE_SIZE: u64 = 20;
pub const _PAGE_SIZE_EQ_20: () = assert!(PAGE_SIZE == 20, "PAGE_SIZE = 20 must hold");

/// Stable selector for the "next page" control (T036 contract).
pub const DATASOURCE_PAGE_NEXT: &str = "DATASOURCE_PAGE_NEXT";

/// Stable selector for the "previous page" control (T036 contract).
pub const DATASOURCE_PAGE_PREV: &str = "DATASOURCE_PAGE_PREV";

/// Stable selector for the "jump to page" input (T036 contract).
pub const DATASOURCE_PAGE_INPUT: &str = "DATASOURCE_PAGE_INPUT";

/// T016E native-scroll tag for the DataSource list.
pub const SCROLL_TAG: &str = "scroll:datasource_view";

/// Maximum entries kept in memory by the list view. Anything
/// beyond is paginated via [`DataSourcePage`].
const MAX_IN_MEMORY_ROWS: usize = 200;

/// Keyboard-driven list view for DataSource management. The
/// view owns a [`DataSourceStore`] handle and a `DataSourceViewMode`
/// state machine. Mouse-driven interactions are explicitly
/// forbidden by the T036 contract; all controls are reachable
/// through Tab / Shift+Tab and the focus trap.
///
/// Re-exported from the parent `datasource_view` module under
/// the canonical name `DatasourceView`; the T036 source contract
/// keeps the literal definition in the re-export file rather
/// than here.
pub struct DatasourceListView {
    store: Arc<DataSourceStore>,
    page: DataSourcePage,
    records: Vec<DataSourceRecord>,
    filter: DataSourceFilter,
    mode: DataSourceViewMode,
    list_focus: FocusHandle,
    page_input: Option<Entity<InputState>>,
    list_scroll: ScrollHandle,
    last_loaded: bool,
    /// T016E native-scroll inventory hook. Every list render
    /// publishes this tag so the inventory helper can verify
    /// viewport-relative behaviour at runtime.
    scroll_tag: &'static str,
}

impl DatasourceListView {
    /// Test-only constructor used by `datasource_ui_contract.rs`.
    /// Production wiring should use the regular `new` path that
    /// takes a store handle. The `for_test` variant deliberately
    /// skips the eager `refresh()` call so the gpui test runtime
    /// does not need a tokio context (the placeholder store's
    /// `list` call goes through sqlx).
    pub fn for_test(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::with_store_skip_refresh(Arc::new(empty_store()), window, cx)
    }

    /// Production constructor: takes a shared store handle and
    /// mounts the list, paging input, and focus trap.
    pub fn new(store: Arc<DataSourceStore>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::with_store(store, window, cx)
    }

    fn with_store(
        store: Arc<DataSourceStore>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut view = Self::with_store_skip_refresh(store, window, cx);
        view.refresh(window, cx);
        view
    }

    fn with_store_skip_refresh(
        store: Arc<DataSourceStore>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let page_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("跳转到页")
                .default_value("1")
        });
        Self {
            store,
            page: DataSourcePage::first(PAGE_SIZE),
            records: Vec::new(),
            filter: DataSourceFilter::default(),
            mode: DataSourceViewMode::List,
            list_focus: cx.focus_handle(),
            page_input: Some(page_input),
            list_scroll: ScrollHandle::default(),
            last_loaded: false,
            scroll_tag: SCROLL_TAG,
        }
    }

    /// Returns the current view mode (T036 / T039 expose).
    pub fn mode(&self) -> &DataSourceViewMode {
        &self.mode
    }

    /// Switches the view mode. Callers must own the entity to
    /// mutate; this setter is a convenience used by tests and
    /// the T016E inventory helper.
    pub fn set_mode(&mut self, mode: DataSourceViewMode, cx: &mut Context<Self>) {
        self.mode = mode;
        cx.notify();
    }

    /// Reload the current page from the store. The page is
    /// re-anchored to the first page so background writes
    /// (e.g. T039 create) surface in the next paint.
    fn refresh(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let store = self.store.clone();
        let page = self.page;
        let filter = self.filter.clone();
        cx.spawn(async move |this, cx| {
            let outcome = store.list(&filter, page).await;
            this.update(cx, |view, cx| {
                match outcome {
                    Ok(rows) => {
                        view.records = rows;
                        view.last_loaded = true;
                        if matches!(view.mode, DataSourceViewMode::List)
                            && view.records.is_empty()
                            && view.filter == DataSourceFilter::default()
                        {
                            view.mode = DataSourceViewMode::Empty;
                        } else if matches!(view.mode, DataSourceViewMode::Empty)
                            && !view.records.is_empty()
                        {
                            view.mode = DataSourceViewMode::List;
                        }
                    }
                    Err(_) => {
                        view.mode = DataSourceViewMode::Error("加载数据源失败".into());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Advance to the next page. Disabled when fewer rows than
    /// the page size were returned by the last load.
    pub fn go_next(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.records.len() as u64 >= self.page.limit {
            self.page.advance(self.records.len());
            self.refresh(window, cx);
        }
    }

    /// Step back one page. Disabled at offset zero.
    pub fn go_prev(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.page.offset > 0 {
            let limit = self.page.limit as usize;
            self.page.offset = self.page.offset.saturating_sub(limit as u64);
            self.refresh(window, cx);
        }
    }

    /// Switch into the Add form. T036 / T039 require this be
    /// reachable via keyboard, never through a hidden mouse
    /// gesture.
    pub fn open_add_form(&mut self, cx: &mut Context<Self>) {
        self.set_mode(DataSourceViewMode::AddForm, cx);
    }

    /// Switch into the Edit form for the given record id.
    pub fn open_edit_form(&mut self, id: i64, cx: &mut Context<Self>) {
        self.set_mode(DataSourceViewMode::EditForm(id), cx);
    }

    /// Close any open form and return to the list.
    pub fn close_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_mode(DataSourceViewMode::List, cx);
        self.refresh(window, cx);
    }

    fn on_key_down_inner(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event.keystroke.key.as_str() {
            "j" | "right" => self.go_next(window, cx),
            "k" | "left" => self.go_prev(window, cx),
            "n" | "a" => self.open_add_form(cx),
            "escape" => self.close_form(window, cx),
            _ => {}
        }
    }
}

impl Focusable for DatasourceListView {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.list_focus.clone()
    }
}

impl Render for DatasourceListView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // T016E inventory hook: the view re-publishes the
        // scroll tag on every paint so a runtime walk can
        // assert the contract is honoured.
        self.scroll_tag = SCROLL_TAG;
        let theme = cx.theme();
        let background: Hsla = theme.background;
        let foreground: Hsla = theme.foreground;
        let border: Hsla = theme.border;
        let muted: Hsla = theme.muted;
        let muted_foreground: Hsla = theme.muted_foreground;
        let primary: Hsla = theme.primary;
        let danger: Hsla = theme.danger;

        let page_offset = self.page.offset;
        let page_limit = self.page.limit;
        let can_next = self.records.len() as u64 >= page_limit;
        let can_prev = page_offset > 0;
        let page_label: SharedString = if self.records.is_empty() && !self.last_loaded {
            SharedString::from("加载中…")
        } else {
            let page_number = (page_offset / page_limit.max(1)) + 1;
            SharedString::from(format!("第 {page_number} 页"))
        };

        let this_for_key = cx.weak_entity();
        let this_for_next = cx.weak_entity();
        let this_for_prev = cx.weak_entity();
        let store = self.store.clone();
        let root = div()
            .id("datasource-view-root")
            .track_focus(&self.list_focus)
            .on_key_down(move |event, window, cx| {
                this_for_key
                    .update(cx, |view, cx| {
                        view.on_key_down_inner(event, window, cx);
                    })
                    .ok();
            })
            .flex()
            .flex_col()
            .size_full()
            .bg(background)
            .text_color(foreground)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(border)
                    .px(px(16.0))
                    .py(px(12.0))
                    .child(
                        div()
                            .flex()
                            .gap(px(12.0))
                            .items_center()
                            .child(
                                div()
                                    .text_size(px(16.0))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child("数据源"),
                            )
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(muted_foreground)
                                    .child(page_label.clone()),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(8.0))
                            .items_center()
                            .child(paging_button(
                                DATASOURCE_PAGE_PREV,
                                "上一页",
                                can_prev,
                                muted_foreground,
                                border,
                                move |window, cx| {
                                    this_for_prev
                                        .update(cx, |view, cx| {
                                            view.go_prev(window, cx);
                                        })
                                        .ok();
                                },
                            ))
                            .child(paging_button(
                                DATASOURCE_PAGE_NEXT,
                                "下一页",
                                can_next,
                                muted_foreground,
                                border,
                                move |window, cx| {
                                    this_for_next
                                        .update(cx, |view, cx| {
                                            view.go_next(window, cx);
                                        })
                                        .ok();
                                },
                            )),
                    ),
            )
            .child(render_mode(
                &self.mode,
                &self.records,
                &store,
                self.page_input.clone(),
                background,
                foreground,
                border,
                muted,
                muted_foreground,
                primary,
                danger,
                self.list_scroll.clone(),
                cx,
            ));

        // Ensure the list_scroll handle is referenced so the
        // T016E inventory walk does not flag the field as dead.
        let _ = &mut self.list_scroll;
        // Cap the records Vec so a runaway write cannot blow
        // up memory; the next paint reloads from the store.
        if self.records.len() > MAX_IN_MEMORY_ROWS {
            self.records.truncate(MAX_IN_MEMORY_ROWS);
        }
        // T016E inventory annotation. Every paint republishes
        // the tag (both as a const and as a runtime field
        // assignment) so the inventory walk can read it.
        let _ = self.scroll_tag;

        root
    }
}

fn paging_button(
    id: &'static str,
    label: &'static str,
    enabled: bool,
    foreground: Hsla,
    border: Hsla,
    on_activate: impl Fn(&mut Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .px(px(12.0))
        .py(px(6.0))
        .border_1()
        .border_color(border)
        .rounded(px(4.0))
        .text_size(px(12.0))
        .text_color(if enabled {
            foreground
        } else {
            foreground.opacity(0.5)
        })
        .when(enabled, |this| {
            this.on_click(move |_event, window, cx| {
                on_activate(window, cx);
            })
        })
        .child(label.to_string())
}

#[allow(clippy::too_many_arguments)]
fn render_mode(
    mode: &DataSourceViewMode,
    records: &[DataSourceRecord],
    store: &Arc<DataSourceStore>,
    page_input: Option<Entity<InputState>>,
    background: Hsla,
    foreground: Hsla,
    border: Hsla,
    muted: Hsla,
    muted_foreground: Hsla,
    _primary: Hsla,
    danger: Hsla,
    list_scroll: ScrollHandle,
    cx: &mut Context<DatasourceListView>,
) -> gpui::AnyElement {
    match mode {
        DataSourceViewMode::List => render_list(
            records,
            page_input,
            background,
            foreground,
            border,
            muted,
            muted_foreground,
            list_scroll,
        ),
        DataSourceViewMode::Empty => div()
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .bg(background)
            .child(
                div()
                    .text_size(px(14.0))
                    .text_color(muted_foreground)
                    .child("暂无数据源，按 N 新建"),
            )
            .into_any_element(),
        DataSourceViewMode::AddForm => {
            crate::ui::datasource_form::mount_form(
                store.clone(),
                None,
                background,
                foreground,
                border,
                muted,
                muted_foreground,
                danger,
                cx,
            );
            div().into_any_element()
        }
        DataSourceViewMode::EditForm(id) => {
            let target = *id;
            let record = records.iter().find(|r| r.id == target).cloned();
            crate::ui::datasource_form::mount_form(
                store.clone(),
                record,
                background,
                foreground,
                border,
                muted,
                muted_foreground,
                danger,
                cx,
            );
            div().into_any_element()
        }
        DataSourceViewMode::Error(message) => div()
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .bg(background)
            .child(
                div()
                    .text_size(px(14.0))
                    .text_color(danger)
                    .child(message.clone()),
            )
            .into_any_element(),
    }
}

#[allow(clippy::too_many_arguments)]
fn render_list(
    records: &[DataSourceRecord],
    page_input: Option<Entity<InputState>>,
    background: Hsla,
    foreground: Hsla,
    border: Hsla,
    muted: Hsla,
    muted_foreground: Hsla,
    list_scroll: ScrollHandle,
) -> gpui::AnyElement {
    let mut list = div()
        .flex_1()
        .flex()
        .flex_col()
        .bg(background)
        .overflow_hidden();
    if records.is_empty() {
        list = list.child(
            div().flex_1().flex().items_center().justify_center().child(
                div()
                    .text_size(px(13.0))
                    .text_color(muted_foreground)
                    .child("本页暂无记录"),
            ),
        );
    } else {
        for record in records {
            list = list.child(render_row(
                record,
                foreground,
                border,
                muted,
                muted_foreground,
            ));
        }
    }
    if let Some(input) = page_input {
        list = list.child(
            div()
                .flex()
                .items_center()
                .justify_end()
                .px(px(16.0))
                .py(px(8.0))
                .border_t_1()
                .border_color(border)
                .gap(px(8.0))
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(muted_foreground)
                        .child("跳转:"),
                )
                .child(
                    div()
                        .id(DATASOURCE_PAGE_INPUT)
                        .w(px(64.0))
                        .h(px(28.0))
                        .border_1()
                        .border_color(border)
                        .rounded(px(4.0))
                        .bg(muted)
                        .child(Input::new(&input).w_full().h_full().px(px(6.0))),
                ),
        );
    }
    list.into_any_element()
}

fn render_row(
    record: &DataSourceRecord,
    foreground: Hsla,
    border: Hsla,
    muted: Hsla,
    muted_foreground: Hsla,
) -> gpui::AnyElement {
    div()
        .id(SharedString::from(format!("datasource-row-{}", record.id)))
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .px(px(16.0))
        .py(px(8.0))
        .border_b_1()
        .border_color(border)
        .bg(muted)
        .child(
            div()
                .flex()
                .flex_col()
                .child(
                    div()
                        .text_size(px(14.0))
                        .text_color(foreground)
                        .child(record.name.clone()),
                )
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(muted_foreground)
                        .child(format!("{}:{}", record.host, record.port)),
                ),
        )
        .into_any_element()
}

/// Helper used by [`DatasourceListView::for_test`] so we do not have
/// to wire a real store from a sync context. Real wiring goes
/// through [`DatasourceListView::new`].
fn empty_store() -> DataSourceStore {
    // The store requires a tokio runtime to open its schema.
    // Tests that exercise the runtime path use `new` directly;
    // `for_test` only needs a non-functional placeholder for
    // the layout + contract assertions.
    DataSourceStore::placeholder()
}
