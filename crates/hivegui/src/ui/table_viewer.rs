use std::collections::HashMap;
use std::time::Instant;

use gpui::{
    Context, CursorStyle, Entity, MouseButton, ScrollHandle, SharedString, Window, div, prelude::*,
    px, rgb,
};
use tracing::info;

use crate::datasource::{ColumnInfo, DataSource, MysqlClient, Store, TableData, TableDataRequest};

struct ErrorModal {
    title: String,
    message: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TableTab {
    Columns,
    Ddl,
    Data,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum RowNumberColumn {
    #[default]
    Normal,
    Hovered,
    Selected,
}

#[derive(Clone)]
struct ColumnResize {
    col_index: usize,
    start_x: f32,
    start_width: f32,
}

impl gpui::Global for ColumnResize {}

#[derive(Clone)]
struct PageCache {
    data: TableData,
    loaded_at: Instant,
}

struct OpenTable {
    name: String,
    database: String,
    column_widths: Vec<f32>,
    active_tab: TableTab,
    columns: Vec<ColumnInfo>,
    ddl: String,
    table_data: Option<TableData>,
    loading: bool,
    error: Option<SharedString>,
    error_modal: Option<ErrorModal>,
    where_clause: String,
    order_by: String,
    current_offset: i64,
    page_size: i64,
    total_count: i64,
    scroll_handle: ScrollHandle,
    page_cache: HashMap<i64, PageCache>,
    selected_row: Option<usize>,
    show_value_panel: bool,
    selected_cell_col: Option<usize>,
    context_menu_visible: bool,
    context_menu_x: f32,
    context_menu_y: f32,
    hovered_row: Option<usize>,
    #[expect(
        dead_code,
        reason = "retained for the pending per-cell hover interaction"
    )]
    hovered_row_col: Option<usize>,
    context_menu_row: Option<usize>,
    context_menu_col: Option<usize>,
}

const DEFAULT_COLUMN_WIDTHS: [f32; 5] = [120.0, 100.0, 60.0, 60.0, 200.0];
const PAGE_CACHE_TTL_SECS: u64 = 30;

impl OpenTable {
    fn new(name: String, database: String) -> Self {
        Self {
            name,
            database,
            column_widths: DEFAULT_COLUMN_WIDTHS.to_vec(),
            active_tab: TableTab::Columns,
            columns: Vec::new(),
            ddl: String::new(),
            table_data: None,
            loading: false,
            error: None,
            error_modal: None,
            where_clause: String::new(),
            order_by: String::new(),
            current_offset: 0,
            page_size: 100,
            total_count: 0,
            scroll_handle: ScrollHandle::new(),
            page_cache: HashMap::new(),
            selected_row: None,
            show_value_panel: false,
            selected_cell_col: None,
            context_menu_visible: false,
            context_menu_x: 0.0,
            context_menu_y: 0.0,
            hovered_row: None,
            hovered_row_col: None,
            context_menu_row: None,
            context_menu_col: None,
        }
    }

    fn current_page(&self) -> i64 {
        self.current_offset / self.page_size
    }

    fn total_pages(&self) -> i64 {
        if self.total_count <= 0 {
            1
        } else {
            (self.total_count + self.page_size - 1) / self.page_size
        }
    }

    fn get_cached_page(&self, page: i64) -> Option<&PageCache> {
        self.page_cache
            .get(&page)
            .filter(|&cache| cache.loaded_at.elapsed().as_secs() < PAGE_CACHE_TTL_SECS)
    }

    fn show_error(&mut self, title: String, message: String) {
        self.error_modal = Some(ErrorModal { title, message });
    }

    fn dismiss_error(&mut self) {
        self.error_modal = None;
    }
}

pub struct TableViewer {
    data_source: Option<DataSource>,
    store: Option<Entity<Store>>,
    open_tables: Vec<OpenTable>,
    active_table_index: usize,
}

impl TableViewer {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self {
            data_source: None,
            store: None,
            open_tables: Vec::new(),
            active_table_index: 0,
        }
    }

    pub fn set_table(
        &mut self,
        ds: DataSource,
        store: Entity<Store>,
        _source_id: i64,
        database: String,
        table: String,
        cx: &mut Context<Self>,
    ) {
        let existing = self
            .open_tables
            .iter()
            .position(|t| t.database == database && t.name == table);
        if let Some(idx) = existing {
            self.active_table_index = idx;
            cx.notify();
            return;
        }

        let ds_clone = ds.clone();
        let store_clone = store.read(cx).clone();

        self.data_source = Some(ds);
        self.store = Some(store);
        let new_idx = self.open_tables.len();
        self.open_tables
            .push(OpenTable::new(table.clone(), database.clone()));
        self.active_table_index = new_idx;
        cx.notify();

        self.open_tables[new_idx].loading = true;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let password = match store_clone.decrypt_password(&ds_clone.encrypted_password) {
                Ok(p) => p,
                Err(e) => {
                    this.update(cx, |v, cx| {
                        if let Some(t) = v.open_tables.get_mut(new_idx) {
                            t.loading = false;
                            t.show_error("解密失败".to_string(), format!("{}", e));
                        }
                        cx.notify();
                    })
                    .ok();
                    return;
                }
            };

            let result = MysqlClient::query_columns(
                &ds_clone.host,
                ds_clone.port,
                &ds_clone.username,
                &password,
                &database,
                &table,
            )
            .await;
            match result {
                Ok(cols) => {
                    this.update(cx, |v, cx| {
                        if let Some(t) = v.open_tables.get_mut(new_idx) {
                            t.loading = false;
                            t.columns = cols;
                            let needed = DEFAULT_COLUMN_WIDTHS.len().max(t.columns.len() + 1);
                            while t.column_widths.len() < needed {
                                t.column_widths.push(200.0);
                            }
                        }
                        cx.notify();
                    })
                    .ok();
                }
                Err(e) => {
                    this.update(cx, |v, cx| {
                        if let Some(t) = v.open_tables.get_mut(new_idx) {
                            t.loading = false;
                            t.show_error("加载列失败".to_string(), format!("{}", e));
                        }
                        cx.notify();
                    })
                    .ok();
                }
            }
        })
        .detach();
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.data_source = None;
        self.store = None;
        self.open_tables.clear();
        self.active_table_index = 0;
        cx.notify();
    }

    fn switch_table_tab(&mut self, tab: TableTab, cx: &mut Context<Self>) {
        let Some(t) = self.open_tables.get_mut(self.active_table_index) else {
            return;
        };
        let Some(ds) = self.data_source.as_ref() else {
            return;
        };
        let Some(store) = self.store.as_ref() else {
            return;
        };
        let store_ref = store.read(cx).clone();
        let idx = self.active_table_index;
        t.active_tab = tab.clone();
        match tab {
            TableTab::Columns => t.load_columns_async(ds, &store_ref, idx, cx),
            TableTab::Ddl => t.load_ddl_async(ds, &store_ref, idx, cx),
            TableTab::Data => t.load_data_async(ds, &store_ref, idx, cx, false),
        }
    }

    fn close_table(&mut self, idx: usize, cx: &mut Context<Self>) {
        if idx >= self.open_tables.len() {
            return;
        }
        self.open_tables.remove(idx);
        if self.open_tables.is_empty() {
            self.active_table_index = 0;
        } else if idx <= self.active_table_index {
            self.active_table_index = self
                .active_table_index
                .saturating_sub(1)
                .min(self.open_tables.len() - 1);
        }
        cx.notify();
    }

    fn update_column_width(&mut self, col_index: usize, new_width: f32, cx: &mut Context<Self>) {
        let Some(t) = self.open_tables.get_mut(self.active_table_index) else {
            return;
        };
        if col_index >= t.column_widths.len() {
            return;
        }
        t.column_widths[col_index] = new_width.max(40.0);
        cx.notify();
    }

    fn goto_page(&mut self, page: i64, cx: &mut Context<Self>) {
        let Some(ds) = self.data_source.clone() else {
            return;
        };
        let Some(store) = self.store.clone() else {
            return;
        };
        let idx = self.active_table_index;
        let store_ref = store.read(cx).clone();

        if let Some(t) = self.open_tables.get_mut(idx) {
            let offset = page * t.page_size;
            if offset < 0 || offset > t.total_count {
                return;
            }
            t.current_offset = offset;
            t.selected_row = None;

            if let Some(_cached) = t.get_cached_page(page) {
                let cached_data = t.page_cache.get(&page).unwrap().data.clone();
                t.table_data = Some(cached_data);
                cx.notify();
                return;
            }

            t.load_data_async(&ds, &store_ref, idx, cx, false);
        }
    }

    fn force_refresh(&mut self, cx: &mut Context<Self>) {
        let Some(ds) = self.data_source.clone() else {
            return;
        };
        let Some(store) = self.store.clone() else {
            return;
        };
        let idx = self.active_table_index;
        let store_ref = store.read(cx).clone();

        if let Some(t) = self.open_tables.get_mut(idx) {
            let page = t.current_page();
            t.page_cache.remove(&page);
            t.load_data_async(&ds, &store_ref, idx, cx, true);
        }
    }

    #[expect(
        dead_code,
        reason = "retained until inline row handlers are consolidated"
    )]
    fn select_row(&mut self, row_idx: usize, cx: &mut Context<Self>) {
        if let Some(t) = self.open_tables.get_mut(self.active_table_index) {
            t.selected_row = Some(row_idx);
            cx.notify();
        }
    }

    #[expect(
        dead_code,
        reason = "retained until inline row handlers are consolidated"
    )]
    fn deselect_row(&mut self, cx: &mut Context<Self>) {
        if let Some(t) = self.open_tables.get_mut(self.active_table_index) {
            t.selected_row = None;
            cx.notify();
        }
    }

    #[expect(
        dead_code,
        reason = "retained until inline context-menu handlers are consolidated"
    )]
    fn dismiss_context_menu(&mut self, cx: &mut Context<Self>) {
        if let Some(t) = self.open_tables.get_mut(self.active_table_index) {
            t.context_menu_visible = false;
            t.context_menu_row = None;
            t.context_menu_col = None;
            cx.notify();
        }
    }

    fn open_value_panel_from_context_menu(&mut self, cx: &mut Context<Self>) {
        if let Some(t) = self.open_tables.get_mut(self.active_table_index) {
            if let (Some(row), Some(col)) = (t.context_menu_row, t.context_menu_col) {
                t.selected_row = Some(row);
                t.selected_cell_col = Some(col);
                t.show_value_panel = true;
            }
            t.context_menu_visible = false;
            t.context_menu_row = None;
            t.context_menu_col = None;
            cx.notify();
        }
    }
}

impl OpenTable {
    fn load_columns_async(
        &mut self,
        ds: &DataSource,
        store: &Store,
        table_idx: usize,
        cx: &mut Context<TableViewer>,
    ) {
        let ds = ds.clone();
        let store = store.clone();
        let db = self.database.clone();
        let tbl = self.name.clone();

        self.loading = true;
        self.columns.clear();
        self.error = None;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let password = match store.decrypt_password(&ds.encrypted_password) {
                Ok(p) => p,
                Err(e) => {
                    this.update(cx, |v, cx| {
                        if let Some(t) = v.open_tables.get_mut(table_idx) {
                            t.loading = false;
                            t.show_error("解密失败".to_string(), format!("{}", e));
                        }
                        cx.notify();
                    })
                    .ok();
                    return;
                }
            };

            let result =
                MysqlClient::query_columns(&ds.host, ds.port, &ds.username, &password, &db, &tbl)
                    .await;
            match result {
                Ok(cols) => {
                    this.update(cx, |v, cx| {
                        if let Some(t) = v.open_tables.get_mut(table_idx) {
                            t.loading = false;
                            t.columns = cols;
                            let needed = DEFAULT_COLUMN_WIDTHS.len().max(t.columns.len() + 1);
                            while t.column_widths.len() < needed {
                                t.column_widths.push(200.0);
                            }
                        }
                        cx.notify();
                    })
                    .ok();
                }
                Err(e) => {
                    this.update(cx, |v, cx| {
                        if let Some(t) = v.open_tables.get_mut(table_idx) {
                            t.loading = false;
                            t.error = Some(SharedString::from(format!("加载列失败: {}", e)));
                        }
                        cx.notify();
                    })
                    .ok();
                }
            }
        })
        .detach();
    }

    fn load_ddl_async(
        &mut self,
        ds: &DataSource,
        store: &Store,
        table_idx: usize,
        cx: &mut Context<TableViewer>,
    ) {
        let ds = ds.clone();
        let store = store.clone();
        let db = self.database.clone();
        let tbl = self.name.clone();

        self.loading = true;
        self.ddl.clear();
        self.error = None;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let password = match store.decrypt_password(&ds.encrypted_password) {
                Ok(p) => p,
                Err(e) => {
                    this.update(cx, |v, cx| {
                        if let Some(t) = v.open_tables.get_mut(table_idx) {
                            t.loading = false;
                            t.error = Some(SharedString::from(format!("解密失败: {}", e)));
                        }
                        cx.notify();
                    })
                    .ok();
                    return;
                }
            };

            let result =
                MysqlClient::query_ddl(&ds.host, ds.port, &ds.username, &password, &db, &tbl).await;
            match result {
                Ok(ddl) => {
                    this.update(cx, |v, cx| {
                        if let Some(t) = v.open_tables.get_mut(table_idx) {
                            t.loading = false;
                            t.ddl = ddl;
                        }
                        cx.notify();
                    })
                    .ok();
                }
                Err(e) => {
                    this.update(cx, |v, cx| {
                        if let Some(t) = v.open_tables.get_mut(table_idx) {
                            t.loading = false;
                            t.error = Some(SharedString::from(format!("加载DDL失败: {}", e)));
                        }
                        cx.notify();
                    })
                    .ok();
                }
            }
        })
        .detach();
    }

    fn load_data_async(
        &mut self,
        ds: &DataSource,
        store: &Store,
        table_idx: usize,
        cx: &mut Context<TableViewer>,
        force: bool,
    ) {
        let page = self.current_offset / self.page_size;

        if !force && let Some(_cached) = self.get_cached_page(page) {
            let cached_data = self.page_cache.get(&page).unwrap().data.clone();
            self.table_data = Some(cached_data);
            cx.notify();
            return;
        }

        let ds = ds.clone();
        let store = store.clone();
        let db = self.database.clone();
        let tbl = self.name.clone();
        let where_clause = self.where_clause.clone();
        let order_by = self.order_by.clone();
        let offset = self.current_offset;
        let limit = self.page_size;

        self.loading = true;
        self.table_data = None;
        self.error = None;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let password = match store.decrypt_password(&ds.encrypted_password) {
                Ok(p) => p,
                Err(e) => {
                    this.update(cx, |v, cx| {
                        if let Some(t) = v.open_tables.get_mut(table_idx) {
                            t.loading = false;
                            t.error = Some(SharedString::from(format!("解密失败: {}", e)));
                        }
                        cx.notify();
                    })
                    .ok();
                    return;
                }
            };

            let req = TableDataRequest {
                where_clause: if where_clause.is_empty() {
                    None
                } else {
                    Some(where_clause)
                },
                order_by: if order_by.is_empty() {
                    None
                } else {
                    Some(order_by)
                },
                offset,
                limit,
            };

            let result = MysqlClient::query_table_data(
                &ds.host,
                ds.port,
                &ds.username,
                &password,
                &db,
                &tbl,
                &req,
            )
            .await;
            match result {
                Ok(data) => {
                    let total = data.total_count;
                    let page = offset / limit;
                    this.update(cx, |v, cx| {
                        if let Some(t) = v.open_tables.get_mut(table_idx) {
                            t.loading = false;
                            t.total_count = total;
                            t.page_cache.insert(
                                page,
                                PageCache {
                                    data: data.clone(),
                                    loaded_at: Instant::now(),
                                },
                            );
                            t.table_data = Some(data);
                        }
                        cx.notify();
                    })
                    .ok();
                }
                Err(e) => {
                    this.update(cx, |v, cx| {
                        if let Some(t) = v.open_tables.get_mut(table_idx) {
                            t.loading = false;
                            t.error = Some(SharedString::from(format!("加载数据失败: {}", e)));
                        }
                        cx.notify();
                    })
                    .ok();
                }
            }
        })
        .detach();
    }
}

impl Render for TableViewer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.weak_entity();

        if self.open_tables.is_empty() {
            return div().flex().flex_col().size_full().bg(rgb(0xffffff)).child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .flex_grow()
                    .text_size(px(13.0))
                    .text_color(rgb(0x888888))
                    .child("请选择一个表"),
            );
        }

        let mut col = div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(0xffffff))
            .relative();

        col = col.child(self.render_table_tabs(cx));

        let Some(t) = self.open_tables.get(self.active_table_index) else {
            return col.child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .flex_grow()
                    .text_size(px(13.0))
                    .text_color(rgb(0x888888))
                    .child("请选择一个表"),
            );
        };

        let active_tab = t.active_tab.clone();

        col = col.child(self.render_sub_tabs(&active_tab, this.clone()));

        if t.loading {
            col = col.child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .h(px(60.0))
                    .text_size(px(13.0))
                    .text_color(rgb(0x888888))
                    .child("加载中..."),
            );
        } else {
            match t.active_tab {
                TableTab::Columns => {
                    col = col.child(self.render_columns(t, this.clone(), cx));
                }
                TableTab::Ddl => {
                    col = col.child(self.render_ddl(t));
                }
                TableTab::Data => {
                    col = col.child(self.render_data_tab(t, this.clone(), cx));
                }
            }
        }

        if t.active_tab == TableTab::Data
            && t.context_menu_visible
            && let (Some(row), Some(col_idx)) = (t.context_menu_row, t.context_menu_col)
        {
            let num_cols = t.columns.len();
            let widths: Vec<f32> = (0..num_cols)
                .map(|i| t.column_widths.get(i).copied().unwrap_or(120.0).max(80.0))
                .collect();

            let row_number_width = 50.0;
            let row_height = 11.0 + 4.0 + 1.0;
            let header_height = 11.0 + 8.0 + 1.0;

            let mut menu_x = row_number_width + 16.0;
            for ci in 0..col_idx {
                menu_x += widths.get(ci).copied().unwrap_or(120.0).max(80.0) + 16.0;
            }
            let menu_y = header_height + (row as f32) * row_height;

            col = col.child(
                div()
                    .absolute()
                    .left(px(menu_x))
                    .top(px(menu_y))
                    .w(px(160.0))
                    .bg(rgb(0xffffff))
                    .border_1()
                    .border_color(rgb(0xcccccc))
                    .rounded(px(4.0))
                    .shadow_lg()
                    .cursor(CursorStyle::PointingHand)
                    .child(
                        div()
                            .id("context-menu-item")
                            .px(px(12.0))
                            .py(px(6.0))
                            .text_size(px(12.0))
                            .text_color(rgb(0x333333))
                            .hover(|s| s.bg(rgb(0xe8f0fe)))
                            .cursor(CursorStyle::PointingHand)
                            .child("查看完整值")
                            .on_mouse_down(MouseButton::Left, {
                                let this = this.clone();
                                move |_, _, cx| {
                                    this.update(cx, |v, cx| {
                                        v.open_value_panel_from_context_menu(cx);
                                    })
                                    .ok();
                                }
                            }),
                    ),
            );
        }

        // Render error modal if present
        if let Some(ref error_modal) = t.error_modal {
            let title = error_modal.title.clone();
            let message = error_modal.message.clone();
            let table_idx = self.active_table_index;
            let this_for_modal = this.clone();
            col = col.child(
                div()
                    .absolute()
                    .top(px(0.0))
                    .left(px(0.0))
                    .right(px(0.0))
                    .bottom(px(0.0))
                    .bg(rgb(0x000000))
                    .opacity(0.3)
                    .cursor(CursorStyle::PointingHand)
                    .on_mouse_down(MouseButton::Left, {
                        let this_for_bg = this.clone();
                        move |_, _, cx| {
                            this_for_bg
                                .update(cx, |v, cx| {
                                    if let Some(t) = v.open_tables.get_mut(table_idx) {
                                        t.dismiss_error();
                                    }
                                    cx.notify();
                                })
                                .ok();
                        }
                    })
                    .child(
                        div()
                            .absolute()
                            .top(px(100.0))
                            .left(px(50.0))
                            .right(px(50.0))
                            .max_w(px(500.0))
                            .bg(rgb(0xffffff))
                            .rounded(px(12.0))
                            .shadow_lg()
                            .border_1()
                            .border_color(rgb(0xdddddd))
                            .p(px(24.0))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(16.0))
                                    .child(
                                        div()
                                            .text_size(px(18.0))
                                            .text_color(rgb(0xcc0000))
                                            .child(title),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(14.0))
                                            .text_color(rgb(0x333333))
                                            .child(message),
                                    )
                                    .child(
                                        div().flex().justify_end().child(
                                            div()
                                                .id("close-error-modal")
                                                .px(px(16.0))
                                                .py(px(8.0))
                                                .rounded(px(6.0))
                                                .bg(rgb(0x4a90d9))
                                                .text_color(rgb(0xffffff))
                                                .text_size(px(13.0))
                                                .cursor(CursorStyle::PointingHand)
                                                .child("关闭")
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    move |_, _, cx| {
                                                        this_for_modal
                                                            .update(cx, |v, cx| {
                                                                if let Some(t) =
                                                                    v.open_tables.get_mut(table_idx)
                                                                {
                                                                    t.dismiss_error();
                                                                }
                                                                cx.notify();
                                                            })
                                                            .ok();
                                                    },
                                                ),
                                        ),
                                    ),
                            ),
                    ),
            );
        }

        col
    }
}

impl TableViewer {
    fn render_table_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut row = div()
            .flex()
            .items_center()
            .gap(px(2.0))
            .px(px(12.0))
            .py(px(4.0))
            .border_b_1()
            .border_color(rgb(0xe0e0e0));

        for (idx, tab) in self.open_tables.iter().enumerate() {
            let active = self.active_table_index == idx;
            let tab_label = SharedString::from(tab.name.clone());
            let this = cx.weak_entity();

            let tab_div = div()
                .id(format!("table-tab-{idx}"))
                .flex()
                .items_center()
                .gap(px(6.0))
                .px(px(12.0))
                .py(px(4.0))
                .rounded_t(px(4.0))
                .bg(if active { rgb(0xffffff) } else { rgb(0xf0f0f0) })
                .border_b_1()
                .border_color(if active { rgb(0xffffff) } else { rgb(0xe0e0e0) })
                .text_size(px(12.0))
                .text_color(if active { rgb(0x333333) } else { rgb(0x666666) })
                .cursor(CursorStyle::PointingHand)
                .on_mouse_down(MouseButton::Left, {
                    let this = this.clone();
                    move |_, _, cx| {
                        this.update(cx, |v, cx| {
                            v.active_table_index = idx;
                            cx.notify();
                        })
                        .ok();
                    }
                })
                .child(tab_label);

            let close_btn = div()
                .id(format!("close-tab-{idx}"))
                .w(px(14.0))
                .h(px(14.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(2.0))
                .text_size(px(10.0))
                .text_color(rgb(0x999999))
                .hover(|s| s.bg(rgb(0xe0e0e0)))
                .cursor(CursorStyle::PointingHand)
                .child("x")
                .on_mouse_down(MouseButton::Left, {
                    move |_, _, cx| {
                        this.update(cx, |v, cx| {
                            v.close_table(idx, cx);
                        })
                        .ok();
                    }
                });

            row = row.child(tab_div.child(close_btn));
        }

        row
    }

    fn render_sub_tabs(
        &self,
        active_tab: &TableTab,
        this: gpui::WeakEntity<Self>,
    ) -> impl IntoElement {
        div()
            .flex()
            .gap(px(2.0))
            .px(px(16.0))
            .py(px(4.0))
            .border_b_1()
            .border_color(rgb(0xe0e0e0))
            .child(self.render_sub_tab(
                SharedString::from("columns-tab"),
                SharedString::from("列"),
                active_tab == &TableTab::Columns,
                TableTab::Columns,
                this.clone(),
            ))
            .child(self.render_sub_tab(
                SharedString::from("ddl-tab"),
                SharedString::from("DDL"),
                active_tab == &TableTab::Ddl,
                TableTab::Ddl,
                this.clone(),
            ))
            .child(self.render_sub_tab(
                SharedString::from("data-tab"),
                SharedString::from("数据"),
                active_tab == &TableTab::Data,
                TableTab::Data,
                this,
            ))
    }

    fn render_sub_tab(
        &self,
        id: SharedString,
        label: SharedString,
        active: bool,
        tab: TableTab,
        this: gpui::WeakEntity<Self>,
    ) -> impl IntoElement {
        div()
            .id(id)
            .px(px(12.0))
            .py(px(4.0))
            .rounded(px(4.0))
            .bg(if active { rgb(0x4a90d9) } else { rgb(0xf5f5f5) })
            .text_size(px(12.0))
            .text_color(if active { rgb(0xffffff) } else { rgb(0x666666) })
            .cursor(CursorStyle::PointingHand)
            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                this.update(cx, |v, cx| {
                    v.switch_table_tab(tab.clone(), cx);
                })
                .ok();
            })
            .child(label)
    }

    fn render_columns(
        &self,
        t: &OpenTable,
        this: gpui::WeakEntity<Self>,
        _cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let column_labels = ["列名", "类型", "可空", "主键", "注释"];
        let column_data: Vec<_> = t
            .columns
            .iter()
            .map(|c| {
                vec![
                    SharedString::from(c.name.clone()),
                    SharedString::from(c.data_type.clone()),
                    SharedString::from(if c.is_nullable { "YES" } else { "NO" }),
                    SharedString::from(if c.is_primary_key { "PK" } else { "" }),
                    SharedString::from(c.comment.clone().unwrap_or_default()),
                ]
            })
            .collect();

        let num_cols = column_labels.len().max(t.columns.len().max(1));
        let widths: Vec<f32> = (0..num_cols)
            .map(|i| t.column_widths.get(i).copied().unwrap_or(200.0))
            .collect();

        let mut scroll = div()
            .id("columns-scroll")
            .flex()
            .flex_col()
            .flex_grow()
            .overflow_x_scroll()
            .overflow_y_scroll()
            .track_scroll(&t.scroll_handle);

        if t.columns.is_empty() && !t.loading {
            scroll = scroll.child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .h(px(60.0))
                    .text_size(px(13.0))
                    .text_color(rgb(0x888888))
                    .child("该表暂无列信息"),
            );
        } else {
            let header = div()
                .flex()
                .border_b_1()
                .border_color(rgb(0xe0e0e0))
                .text_size(px(12.0))
                .text_color(rgb(0x333333));

            let header = column_labels
                .iter()
                .enumerate()
                .fold(header, |acc, (i, label)| {
                    let w = widths[i];
                    let col_div = div()
                        .id(format!("col-header-{i}"))
                        .relative()
                        .w(px(w))
                        .flex_shrink_0()
                        .px(px(12.0))
                        .py(px(6.0))
                        .child(label.to_string());

                    let resize_handle = self.render_resize_handle(i, w, this.clone());

                    acc.child(col_div.child(resize_handle))
                });

            scroll = scroll.child(header);

            for row_data in column_data {
                let row = div()
                    .flex()
                    .border_b_1()
                    .border_color(rgb(0xf5f5f5))
                    .text_size(px(12.0));

                let row = row_data
                    .into_iter()
                    .enumerate()
                    .fold(row, |acc, (i, text)| {
                        let w = widths.get(i).copied().unwrap_or(200.0);
                        acc.child(
                            div()
                                .w(px(w))
                                .flex_shrink_0()
                                .px(px(12.0))
                                .py(px(5.0))
                                .child(text),
                        )
                    });

                scroll = scroll.child(row);
            }
        }

        scroll
    }

    fn render_ddl(&self, t: &OpenTable) -> impl IntoElement {
        let ddl = t.ddl.clone();
        div()
            .id("ddl-scroll")
            .flex()
            .flex_grow()
            .overflow_y_scroll()
            .track_scroll(&t.scroll_handle)
            .child(
                div()
                    .p(px(16.0))
                    .text_size(px(12.0))
                    .font_family("monospace")
                    .bg(rgb(0xf9f9f9))
                    .text_color(rgb(0x333333))
                    .child(ddl),
            )
    }

    fn render_data_tab(
        &self,
        t: &OpenTable,
        this: gpui::WeakEntity<Self>,
        _cx: &mut Context<Self>,
    ) -> impl IntoElement {
        info!("[DataTab] render_data_tab called, table: {}", t.name);

        let columns: Option<Vec<SharedString>> = t.table_data.as_ref().map(|data| {
            data.columns
                .iter()
                .map(|c| SharedString::from(c.clone()))
                .collect()
        });
        let rows: Option<Vec<Vec<Option<SharedString>>>> = t.table_data.as_ref().map(|data| {
            data.rows
                .iter()
                .map(|row| {
                    row.iter()
                        .map(|val| val.as_ref().map(|v| SharedString::from(v.clone())))
                        .collect()
                })
                .collect()
        });

        info!(
            "[DataTab] columns count: {:?}, rows count: {:?}",
            columns.as_ref().map(|c| c.len()),
            rows.as_ref().map(|r| r.len())
        );

        let mut container = div()
            .id("data-tab-container")
            .flex()
            .flex_col()
            .flex_grow()
            .size_full()
            .overflow_hidden();

        info!("[DataTab] container created with overflow_hidden and flex_grow");

        if let Some(ref cols) = columns {
            if let Some(ref row_data) = rows {
                if row_data.is_empty() && !t.loading {
                    container = container.child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .h(px(60.0))
                            .text_size(px(13.0))
                            .text_color(rgb(0x888888))
                            .child("该表暂无数据"),
                    );
                } else {
                    info!("[DataTab] rendering data_area with {} rows", row_data.len());

                    let grid_div = div()
                        .flex()
                        .flex_col()
                        .flex_grow()
                        .overflow_hidden()
                        .child(self.render_data_grid(cols, row_data, t, this.clone()));

                    let data_area = div()
                        .id("data-area")
                        .relative()
                        .flex()
                        .flex_row()
                        .flex_grow()
                        .size_full()
                        .overflow_hidden()
                        .child(grid_div)
                        .when(t.show_value_panel, |el| {
                            el.child(self.render_value_viewer(t, this.clone()))
                        });

                    container = container.child(data_area);
                    container = container.child(self.render_pagination(t, this));

                    info!("[DataTab] data_area and pagination added to container");
                }
            } else if !t.loading {
                container = container.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .h(px(60.0))
                        .text_size(px(13.0))
                        .text_color(rgb(0x888888))
                        .child("该表暂无数据"),
                );
            }
        } else if !t.loading {
            container = container.child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .h(px(60.0))
                    .text_size(px(13.0))
                    .text_color(rgb(0x888888))
                    .child("请选择一个表"),
            );
        }

        container
    }

    fn render_data_grid(
        &self,
        cols: &[SharedString],
        row_data: &[Vec<Option<SharedString>>],
        t: &OpenTable,
        this: gpui::WeakEntity<Self>,
    ) -> impl IntoElement {
        let num_cols = cols.len();
        let widths: Vec<f32> = (0..num_cols)
            .map(|i| t.column_widths.get(i).copied().unwrap_or(120.0).max(80.0))
            .collect();

        let row_number_width = 50.0;
        let total_width: f32 =
            widths.iter().sum::<f32>() + row_number_width + (num_cols as f32 * 16.0);

        info!(
            "[DataGrid] creating scroll container, rows: {}, cols: {}, total_width: {:.1}",
            row_data.len(),
            num_cols,
            total_width
        );

        let mut scroll = div()
            .id("data-scroll")
            .relative()
            .flex()
            .flex_col()
            .flex_grow()
            .size_full()
            .overflow_x_scroll()
            .overflow_y_scroll()
            .track_scroll(&t.scroll_handle);

        info!(
            "[DataGrid] scroll container created with flex_grow, w(total_width), overflow_x_scroll, overflow_y_scroll"
        );

        let mut content = div().flex().flex_col().w(px(total_width));

        let mut header = div()
            .flex()
            .flex_nowrap()
            .border_b_1()
            .border_color(rgb(0xe0e0e0))
            .text_size(px(11.0))
            .bg(rgb(0xf8f8f8));

        header = header.child(
            div()
                .w(px(row_number_width))
                .flex_shrink_0()
                .px(px(4.0))
                .py(px(4.0))
                .child("#"),
        );

        header = cols.iter().enumerate().fold(header, |acc, (i, col)| {
            let w = widths[i];
            let col_div = div()
                .id(format!("data-col-header-{i}"))
                .relative()
                .w(px(w))
                .flex_shrink_0()
                .px(px(8.0))
                .py(px(4.0))
                .child(col.clone());

            let resize_handle = self.render_resize_handle(i, w, this.clone());

            acc.child(col_div.child(resize_handle))
        });

        content = content.child(header);

        for (row_idx, row) in row_data.iter().enumerate() {
            let is_selected = t.selected_row == Some(row_idx);
            let is_hovered = t.hovered_row == Some(row_idx);
            let this_for_row = this.clone();
            let this_for_cell = this.clone();

            let mut row_div = div()
                .id(format!("data-row-{row_idx}"))
                .flex()
                .flex_nowrap()
                .border_b_1()
                .border_color(rgb(0xf0f0f0))
                .text_size(px(11.0))
                .bg(if is_selected {
                    rgb(0xd0e0f0)
                } else if is_hovered {
                    rgb(0xf5f8fc)
                } else {
                    rgb(0xffffff)
                })
                .cursor(CursorStyle::PointingHand)
                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    this_for_row
                        .update(cx, |v, cx| {
                            if let Some(t) = v.open_tables.get_mut(v.active_table_index) {
                                t.selected_row = Some(row_idx);
                            }
                            cx.notify();
                        })
                        .ok();
                })
                .on_mouse_move({
                    let this_for_cell = this_for_cell.clone();
                    move |_, _, cx| {
                        this_for_cell
                            .update(cx, |v, cx| {
                                if let Some(t) = v.open_tables.get_mut(v.active_table_index) {
                                    t.hovered_row = Some(row_idx);
                                }
                                cx.notify();
                            })
                            .ok();
                    }
                })
                .on_mouse_up(MouseButton::Left, {
                    let this_for_cell = this_for_cell.clone();
                    move |_, _, cx| {
                        this_for_cell
                            .update(cx, |v, cx| {
                                if let Some(t) = v.open_tables.get_mut(v.active_table_index) {
                                    t.hovered_row = None;
                                }
                                cx.notify();
                            })
                            .ok();
                    }
                });

            row_div = row_div.child(
                div()
                    .w(px(row_number_width))
                    .flex_shrink_0()
                    .px(px(6.0))
                    .py(px(2.0))
                    .text_color(rgb(0x999999))
                    .text_size(px(11.0))
                    .child(format!("{}", row_idx + 1)),
            );

            let row_div = row.iter().enumerate().fold(row_div, |acc, (i, val)| {
                let w = widths.get(i).copied().unwrap_or(120.0).max(80.0);
                let this_for_cell = this_for_cell.clone();
                acc.child(
                    div()
                        .relative()
                        .w(px(w))
                        .flex_shrink_0()
                        .px(px(8.0))
                        .py(px(2.0))
                        .text_color(rgb(0x333333))
                        .cursor(CursorStyle::IBeam)
                        .on_mouse_down(MouseButton::Left, {
                            let this_for_cell = this_for_cell.clone();
                            move |_, _, cx| {
                                this_for_cell
                                    .update(cx, |v, cx| {
                                        if let Some(t) = v.open_tables.get_mut(v.active_table_index)
                                        {
                                            t.selected_row = Some(row_idx);
                                            t.selected_cell_col = Some(i);
                                            t.show_value_panel = true;
                                        }
                                        cx.notify();
                                    })
                                    .ok();
                            }
                        })
                        .on_mouse_down(MouseButton::Right, {
                            let this_for_cell = this_for_cell.clone();
                            move |event, _, cx| {
                                let x: f32 = event.position.x.into();
                                let y: f32 = event.position.y.into();
                                this_for_cell
                                    .update(cx, |v, cx| {
                                        if let Some(t) = v.open_tables.get_mut(v.active_table_index)
                                        {
                                            t.selected_row = Some(row_idx);
                                            t.selected_cell_col = Some(i);
                                            t.context_menu_visible = true;
                                            t.context_menu_row = Some(row_idx);
                                            t.context_menu_col = Some(i);
                                            t.context_menu_x = x;
                                            t.context_menu_y = y;
                                        }
                                        cx.notify();
                                    })
                                    .ok();
                            }
                        })
                        .child(
                            div()
                                .truncate()
                                .child(val.clone().unwrap_or(SharedString::from("NULL"))),
                        ),
                )
            });

            content = content.child(row_div);
        }

        scroll = scroll.child(content);
        scroll
    }

    fn render_value_viewer(&self, t: &OpenTable, this: gpui::WeakEntity<Self>) -> impl IntoElement {
        let Some(ref data) = t.table_data else {
            return div();
        };
        let Some(row_idx) = t.selected_row else {
            return div();
        };
        let Some(row) = data.rows.get(row_idx) else {
            return div();
        };
        let Some(col_idx) = t.selected_cell_col else {
            return div();
        };

        let col_name = data
            .columns
            .get(col_idx)
            .map(|c| SharedString::from(c.clone()))
            .unwrap_or(SharedString::from("-"));
        let type_name = t
            .columns
            .iter()
            .find(|c| c.name == *col_name)
            .map(|c| SharedString::from(c.data_type.clone()))
            .unwrap_or(SharedString::from("-"));
        let value = row
            .get(col_idx)
            .and_then(|v| v.as_ref())
            .map(|v| SharedString::from(v.clone()))
            .unwrap_or(SharedString::from("NULL"));

        div()
            .w(px(360.0))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .border_l_1()
            .border_color(rgb(0xe0e0e0))
            .bg(rgb(0xfafafa))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px(px(12.0))
                    .py(px(6.0))
                    .border_b_1()
                    .border_color(rgb(0xe0e0e0))
                    .text_size(px(12.0))
                    .text_color(rgb(0x333333))
                    .child("数值查看器")
                    .child(
                        div()
                            .w(px(16.0))
                            .h(px(16.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(2.0))
                            .text_size(px(10.0))
                            .text_color(rgb(0x999999))
                            .hover(|s| s.bg(rgb(0xe0e0e0)))
                            .cursor(CursorStyle::PointingHand)
                            .child("x")
                            .on_mouse_down(MouseButton::Left, {
                                let this = this.clone();
                                move |_, _, cx| {
                                    this.update(cx, |v, cx| {
                                        if let Some(t) = v.open_tables.get_mut(v.active_table_index)
                                        {
                                            t.show_value_panel = false;
                                            t.selected_cell_col = None;
                                        }
                                        cx.notify();
                                    })
                                    .ok();
                                }
                            }),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap(px(8.0))
                    .px(px(12.0))
                    .py(px(6.0))
                    .border_b_1()
                    .border_color(rgb(0xe0e0e0))
                    .text_size(px(11.0))
                    .child(
                        div()
                            .flex()
                            .gap(px(4.0))
                            .child(div().text_color(rgb(0x999999)).child("字段:"))
                            .child(div().text_color(rgb(0x333333)).child(col_name)),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(4.0))
                            .child(div().text_color(rgb(0x999999)).child("类型:"))
                            .child(div().text_color(rgb(0x333333)).child(type_name)),
                    ),
            )
            .child(
                div()
                    .id("value-viewer-scroll")
                    .flex()
                    .flex_grow()
                    .px(px(12.0))
                    .py(px(8.0))
                    .text_size(px(11.0))
                    .font_family("monospace")
                    .text_color(rgb(0x333333))
                    .overflow_y_scroll()
                    .child(value),
            )
    }

    fn render_pagination(&self, t: &OpenTable, this: gpui::WeakEntity<Self>) -> impl IntoElement {
        let current_page = t.current_page();
        let total_pages = t.total_pages();
        let page_size = t.page_size;
        let has_prev = current_page > 0;
        let has_next = current_page + 1 < total_pages;

        let mut visited_pages: Vec<i64> = t.page_cache.keys().copied().collect();
        visited_pages.sort();
        if !visited_pages.contains(&current_page) {
            visited_pages.push(current_page);
            visited_pages.sort();
        }

        let mut pager = div()
            .flex()
            .items_center()
            .justify_between()
            .px(px(12.0))
            .py(px(6.0))
            .border_t_1()
            .border_color(rgb(0xe0e0e0))
            .text_size(px(11.0))
            .text_color(rgb(0x666666));

        pager = pager.child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child("每页".to_string())
                .child(
                    div()
                        .flex()
                        .gap(px(2.0))
                        .child(self.render_page_size_option(50, page_size, this.clone()))
                        .child(self.render_page_size_option(100, page_size, this.clone()))
                        .child(self.render_page_size_option(200, page_size, this.clone()))
                        .child(self.render_page_size_option(500, page_size, this.clone())),
                ),
        );

        let mut btn_row = div().flex().items_center().gap(px(4.0));

        btn_row = btn_row.child(
            div()
                .id("prev-page")
                .px(px(8.0))
                .py(px(2.0))
                .rounded(px(3.0))
                .bg(if has_prev {
                    rgb(0xe0e0e0)
                } else {
                    rgb(0xf5f5f5)
                })
                .text_size(px(11.0))
                .cursor(if has_prev {
                    CursorStyle::PointingHand
                } else {
                    CursorStyle::Arrow
                })
                .child("上一页")
                .when(has_prev, |el| {
                    let this = this.clone();
                    el.on_mouse_down(MouseButton::Left, move |_, _, cx| {
                        let page = current_page - 1;
                        this.update(cx, |v, cx| {
                            v.goto_page(page, cx);
                        })
                        .ok();
                    })
                }),
        );

        for &page_num in &visited_pages {
            let is_current = page_num == current_page;
            btn_row = btn_row.child(
                div()
                    .id(format!("page-{page_num}"))
                    .px(px(6.0))
                    .py(px(2.0))
                    .rounded(px(3.0))
                    .bg(if is_current {
                        rgb(0x4a90d9)
                    } else {
                        rgb(0xf0f0f0)
                    })
                    .text_color(if is_current {
                        rgb(0xffffff)
                    } else {
                        rgb(0x333333)
                    })
                    .text_size(px(11.0))
                    .cursor(if is_current {
                        CursorStyle::Arrow
                    } else {
                        CursorStyle::PointingHand
                    })
                    .child(format!("{}", page_num + 1))
                    .when(!is_current, |el| {
                        let this = this.clone();
                        el.on_mouse_down(MouseButton::Left, move |_, _, cx| {
                            this.update(cx, |v, cx| {
                                v.goto_page(page_num, cx);
                            })
                            .ok();
                        })
                    }),
            );
        }

        btn_row = btn_row.child(
            div()
                .id("next-page")
                .px(px(8.0))
                .py(px(2.0))
                .rounded(px(3.0))
                .bg(if has_next {
                    rgb(0xe0e0e0)
                } else {
                    rgb(0xf5f5f5)
                })
                .text_size(px(11.0))
                .cursor(if has_next {
                    CursorStyle::PointingHand
                } else {
                    CursorStyle::Arrow
                })
                .child("下一页")
                .when(has_next, |el| {
                    let this = this.clone();
                    el.on_mouse_down(MouseButton::Left, move |_, _, cx| {
                        let page = current_page + 1;
                        this.update(cx, |v, cx| {
                            v.goto_page(page, cx);
                        })
                        .ok();
                    })
                }),
        );

        btn_row = btn_row.child(
            div()
                .id("force-refresh")
                .px(px(8.0))
                .py(px(2.0))
                .rounded(px(3.0))
                .bg(rgb(0xe0e0e0))
                .text_size(px(11.0))
                .cursor(CursorStyle::PointingHand)
                .child("刷新")
                .on_mouse_down(MouseButton::Left, {
                    let this = this.clone();
                    move |_, _, cx| {
                        this.update(cx, |v, cx| {
                            v.force_refresh(cx);
                        })
                        .ok();
                    }
                }),
        );

        pager = pager.child(btn_row);
        pager
    }

    fn render_page_size_option(
        &self,
        size: i64,
        current_size: i64,
        this: gpui::WeakEntity<Self>,
    ) -> impl IntoElement {
        let is_selected = size == current_size;
        div()
            .id(format!("page-size-{size}"))
            .px(px(6.0))
            .py(px(2.0))
            .rounded(px(3.0))
            .bg(if is_selected {
                rgb(0x4a90d9)
            } else {
                rgb(0xf0f0f0)
            })
            .text_color(if is_selected {
                rgb(0xffffff)
            } else {
                rgb(0x333333)
            })
            .text_size(px(11.0))
            .cursor(CursorStyle::PointingHand)
            .child(format!("{}", size))
            .when(!is_selected, |el| {
                el.on_mouse_down(MouseButton::Left, {
                    let this = this.clone();
                    move |_, _, cx| {
                        this.update(cx, |v, cx| {
                            if let Some(t) = v.open_tables.get_mut(v.active_table_index) {
                                t.page_size = size;
                                t.current_offset = 0;
                                t.page_cache.clear();
                                t.selected_row = None;
                                t.load_data_async(
                                    &v.data_source.clone().unwrap(),
                                    &v.store.as_ref().unwrap().read(cx).clone(),
                                    v.active_table_index,
                                    cx,
                                    true,
                                );
                            }
                        })
                        .ok();
                    }
                })
            })
    }

    fn render_resize_handle(
        &self,
        col_idx: usize,
        start_width: f32,
        this: gpui::WeakEntity<Self>,
    ) -> impl IntoElement {
        div()
            .id(format!("resize-handle-{col_idx}"))
            .absolute()
            .right(px(0.0))
            .top(px(0.0))
            .bottom(px(0.0))
            .w(px(4.0))
            .cursor(CursorStyle::PointingHand)
            .on_mouse_down(MouseButton::Left, {
                move |event: &gpui::MouseDownEvent, _window, cx| {
                    cx.set_global(ColumnResize {
                        col_index: col_idx,
                        start_x: event.position.x.into(),
                        start_width,
                    });
                }
            })
            .on_mouse_move({
                let this = this.clone();
                move |event: &gpui::MouseMoveEvent, _window, cx| {
                    if cx.has_global::<ColumnResize>() {
                        let resize = cx.global::<ColumnResize>();
                        if resize.col_index == col_idx {
                            let delta: f32 = (event.position.x - gpui::px(resize.start_x)).into();
                            let new_width = resize.start_width + delta;
                            this.update(cx, |v, cx| {
                                v.update_column_width(col_idx, new_width, cx);
                            })
                            .ok();
                        }
                    }
                }
            })
            .on_mouse_up(MouseButton::Left, {
                move |_event, _window, cx| {
                    cx.remove_global::<ColumnResize>();
                }
            })
    }
}
