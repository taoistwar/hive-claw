use gpui::{
    AnyElement, Context, CursorStyle, Entity, Hsla, MouseButton, ScrollHandle, SharedString,
    Window, div, prelude::*, px,
};
use gpui_component::ActiveTheme as _;

use crate::datasource::{MysqlClient, Store, TableInfo};
use crate::ui::management_style::{ActionRole, ActionSize, ManagementStyle, action_button};

fn icon_caret_right(color: Hsla) -> AnyElement {
    div()
        .w(px(16.0))
        .h(px(16.0))
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .w(px(8.0))
                .h(px(8.0))
                .flex()
                .flex_col()
                .justify_between()
                .child(div().w(px(2.0)).h(px(1.5)).bg(color).rounded(px(0.75)))
                .child(div().w(px(4.0)).h(px(1.5)).bg(color).rounded(px(0.75)))
                .child(div().w(px(6.0)).h(px(1.5)).bg(color).rounded(px(0.75))),
        )
        .into_any()
}

fn icon_caret_down(color: Hsla) -> AnyElement {
    div()
        .w(px(16.0))
        .h(px(16.0))
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .w(px(8.0))
                .h(px(8.0))
                .flex()
                .flex_row()
                .justify_between()
                .child(div().w(px(1.5)).h(px(2.0)).bg(color).rounded(px(0.75)))
                .child(div().w(px(1.5)).h(px(4.0)).bg(color).rounded(px(0.75)))
                .child(div().w(px(1.5)).h(px(6.0)).bg(color).rounded(px(0.75))),
        )
        .into_any()
}

fn icon_database(color: Hsla) -> AnyElement {
    div()
        .w(px(16.0))
        .h(px(16.0))
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .w(px(12.0))
                .h(px(12.0))
                .rounded(px(6.0))
                .border_1()
                .border_color(color)
                .flex()
                .flex_col()
                .items_center()
                .justify_between()
                .py(px(1.0))
                .child(div().w(px(10.0)).h(px(1.5)).bg(color).rounded(px(0.75)))
                .child(div().w(px(10.0)).h(px(1.5)).bg(color).rounded(px(0.75)))
                .child(div().w(px(10.0)).h(px(1.5)).bg(color).rounded(px(0.75))),
        )
        .into_any()
}

fn icon_data_source(background: Hsla, foreground: Hsla) -> AnyElement {
    div()
        .w(px(16.0))
        .h(px(16.0))
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .w(px(12.0))
                .h(px(12.0))
                .rounded(px(2.0))
                .bg(background)
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .w(px(8.0))
                        .h(px(8.0))
                        .rounded(px(1.0))
                        .border_1()
                        .border_color(foreground)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(div().w(px(4.0)).h(px(4.0)).rounded(px(2.0)).bg(foreground)),
                ),
        )
        .into_any()
}

fn icon_table(color: Hsla) -> AnyElement {
    div()
        .w(px(16.0))
        .h(px(16.0))
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .w(px(12.0))
                .h(px(12.0))
                .rounded(px(2.0))
                .border_1()
                .border_color(color)
                .flex()
                .flex_col()
                .child(
                    div()
                        .w_full()
                        .h(px(3.0))
                        .border_b_1()
                        .border_color(color)
                        .flex()
                        .child(div().w(px(6.0)).h_full().border_r_1().border_color(color)),
                )
                .child(
                    div()
                        .w_full()
                        .h(px(3.0))
                        .border_b_1()
                        .border_color(color)
                        .flex()
                        .child(div().w(px(6.0)).h_full().border_r_1().border_color(color)),
                )
                .child(
                    div()
                        .w_full()
                        .h(px(3.0))
                        .flex()
                        .child(div().w(px(6.0)).h_full().border_r_1().border_color(color)),
                ),
        )
        .into_any()
}

#[derive(Debug, Clone, PartialEq)]
pub enum TreeNode {
    DataSource {
        id: i64,
        name: String,
        host: String,
        port: u16,
        username: String,
        encrypted_password: Vec<u8>,
        expanded: bool,
        databases: Vec<DatabaseNode>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct DatabaseNode {
    pub name: String,
    pub expanded: bool,
    pub tables: Vec<TableInfo>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TreeSelection {
    DataSource(i64),
    Database {
        source_id: i64,
        database: String,
    },
    Table {
        source_id: i64,
        database: String,
        table: String,
    },
}

pub struct TreeNav {
    pub store: Option<Entity<Store>>,
    pub nodes: Vec<TreeNode>,
    pub loading: bool,
    pub error: Option<SharedString>,
    pub selected: Option<TreeSelection>,
    pub pending_action: Option<PendingAction>,
    scroll_handle: ScrollHandle,
    pub error_modal: Option<ErrorModal>,
}

pub struct ErrorModal {
    pub title: String,
    pub message: String,
}

impl TreeNav {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self {
            store: None,
            nodes: Vec::new(),
            loading: false,
            error: None,
            selected: None,
            pending_action: None,
            scroll_handle: ScrollHandle::new(),
            error_modal: None,
        }
    }

    pub fn set_store(&mut self, store: Entity<Store>, cx: &mut Context<Self>) {
        self.store = Some(store.clone());
        self.load_data_sources(store, cx);
    }

    fn load_data_sources(&mut self, store: Entity<Store>, cx: &mut Context<Self>) {
        let store_clone = store.read(cx).clone();
        let _this = cx.weak_entity();

        self.loading = true;
        self.nodes.clear();
        self.error = None;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = store_clone.list().await;
            match result {
                Ok(sources) => {
                    let nodes: Vec<TreeNode> = sources
                        .into_iter()
                        .map(|ds| TreeNode::DataSource {
                            id: ds.id,
                            name: ds.name,
                            host: ds.host,
                            port: ds.port,
                            username: ds.username,
                            encrypted_password: ds.encrypted_password,
                            expanded: false,
                            databases: Vec::new(),
                        })
                        .collect();

                    this.update(cx, |tree, cx| {
                        tree.loading = false;
                        tree.nodes = nodes;
                        tree.error = None;
                        cx.notify();
                    })
                    .ok();
                }
                Err(e) => {
                    this.update(cx, |tree, cx| {
                        tree.loading = false;
                        tree.show_error("加载数据源失败".to_string(), format!("{}", e), cx);
                    })
                    .ok();
                }
            }
        })
        .detach();
    }

    pub fn toggle_data_source(&mut self, index: usize, cx: &mut Context<Self>) {
        let node = self.nodes[index].clone();
        let TreeNode::DataSource {
            expanded,
            databases,
            host,
            port,
            username,
            encrypted_password,
            ..
        } = node;

        if expanded {
            if let Some(TreeNode::DataSource { expanded, .. }) = self.nodes.get_mut(index) {
                *expanded = false;
            }
            cx.notify();
            return;
        }

        if !databases.is_empty() {
            if let Some(TreeNode::DataSource { expanded, .. }) = self.nodes.get_mut(index) {
                *expanded = true;
            }
            cx.notify();
            return;
        }

        let Some(ref store) = self.store else { return };
        let store = store.read(cx).clone();
        let _this = cx.weak_entity();

        self.loading = true;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let password = match store.decrypt_password(&encrypted_password) {
                Ok(p) => p,
                Err(e) => {
                    this.update(cx, |tree, cx| {
                        tree.loading = false;
                        tree.show_error("解密失败".to_string(), format!("{}", e), cx);
                    })
                    .ok();
                    return;
                }
            };

            let result = MysqlClient::query_databases(&host, port, &username, &password).await;

            match result {
                Ok(dbs) => {
                    let db_nodes: Vec<DatabaseNode> = dbs
                        .into_iter()
                        .map(|db| DatabaseNode {
                            name: db.name,
                            expanded: false,
                            tables: Vec::new(),
                        })
                        .collect();

                    this.update(cx, |tree, cx| {
                        tree.loading = false;
                        if let Some(TreeNode::DataSource {
                            databases,
                            expanded,
                            ..
                        }) = tree.nodes.get_mut(index)
                        {
                            *databases = db_nodes;
                            *expanded = true;
                        }
                        cx.notify();
                    })
                    .ok();
                }
                Err(e) => {
                    this.update(cx, |tree, cx| {
                        tree.loading = false;
                        tree.show_error("加载数据库失败".to_string(), format!("{}", e), cx);
                    })
                    .ok();
                }
            }
        })
        .detach();
    }

    pub fn toggle_database(
        &mut self,
        source_index: usize,
        db_index: usize,
        cx: &mut Context<Self>,
    ) {
        let node = self.nodes[source_index].clone();
        let TreeNode::DataSource {
            id: _source_id,
            host,
            port,
            username,
            encrypted_password,
            databases,
            ..
        } = node;
        let db_node = databases[db_index].clone();

        if db_node.expanded {
            if let Some(TreeNode::DataSource { databases, .. }) = self.nodes.get_mut(source_index) {
                databases[db_index].expanded = false;
            }
            cx.notify();
            return;
        }

        if !db_node.tables.is_empty() {
            if let Some(TreeNode::DataSource { databases, .. }) = self.nodes.get_mut(source_index) {
                databases[db_index].expanded = true;
            }
            cx.notify();
            return;
        }

        let Some(ref store) = self.store else { return };
        let store = store.read(cx).clone();
        let _this = cx.weak_entity();

        self.loading = true;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let password = match store.decrypt_password(&encrypted_password) {
                Ok(p) => p,
                Err(e) => {
                    this.update(cx, |tree, cx| {
                        tree.loading = false;
                        tree.show_error("解密失败".to_string(), format!("{}", e), cx);
                    })
                    .ok();
                    return;
                }
            };

            let result =
                MysqlClient::query_tables(&host, port, &username, &password, &db_node.name).await;

            match result {
                Ok(tables) => {
                    this.update(cx, |tree, cx| {
                        tree.loading = false;
                        if let Some(TreeNode::DataSource { databases, .. }) =
                            tree.nodes.get_mut(source_index)
                        {
                            if let Some(db) = databases.get_mut(db_index) {
                                db.tables = tables;
                                db.expanded = true;
                            }
                        }
                        cx.notify();
                    })
                    .ok();
                }
                Err(e) => {
                    this.update(cx, |tree, cx| {
                        tree.loading = false;
                        tree.show_error("加载表失败".to_string(), format!("{}", e), cx);
                    })
                    .ok();
                }
            }
        })
        .detach();
    }

    pub fn select_item(&mut self, selection: TreeSelection, cx: &mut Context<Self>) {
        self.selected = Some(selection);
        cx.notify();
    }

    pub fn selection(&self) -> Option<&TreeSelection> {
        self.selected.as_ref()
    }

    pub fn pending_action(&self) -> Option<PendingAction> {
        self.pending_action.clone()
    }

    pub fn clear_pending_action(&mut self, cx: &mut Context<Self>) {
        self.pending_action = None;
        cx.notify();
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if let Some(store) = self.store.clone() {
            self.load_data_sources(store, cx);
        }
    }

    pub fn show_error(&mut self, title: String, message: String, cx: &mut Context<Self>) {
        self.error_modal = Some(ErrorModal { title, message });
        cx.notify();
    }

    pub fn dismiss_error(&mut self, cx: &mut Context<Self>) {
        self.error_modal = None;
        cx.notify();
    }

    pub fn take_error_modal(&mut self) -> Option<ErrorModal> {
        self.error_modal.take()
    }
}

#[derive(Debug, Clone)]
pub enum PendingAction {
    Add,
    Edit(i64),
    Delete(i64),
}

impl Render for TreeNav {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.weak_entity();
        let style = ManagementStyle::current(cx);
        let theme = cx.theme();
        let foreground = theme.foreground;
        let muted_foreground = theme.muted_foreground;
        let border = theme.border;
        let list_background = theme.colors.list;
        let list_hover = theme.list_hover;
        let list_active = theme.list_active;
        let main = style.action(ActionRole::Main);
        let mut col = div()
            .flex()
            .flex_col()
            .size_full()
            .bg(list_background)
            .text_color(foreground)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px(px(12.0))
                    .py(px(8.0))
                    .border_b_1()
                    .border_color(border)
                    .child(div().text_size(px(14.0)).child("数据源"))
                    .child(
                        action_button(
                            "btn-add",
                            "+ 添加",
                            ActionRole::Main,
                            ActionSize::Compact,
                            style,
                        )
                        .on_mouse_down(MouseButton::Left, {
                            let panel = this.clone();
                            move |_event, _window, cx| {
                                panel
                                    .update(cx, |p, cx| {
                                        p.pending_action = Some(PendingAction::Add);
                                        cx.notify();
                                    })
                                    .ok();
                            }
                        }),
                    ),
            );

        if self.loading && self.nodes.is_empty() {
            col = col.child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .h(px(40.0))
                    .text_size(px(13.0))
                    .text_color(muted_foreground)
                    .child("加载中..."),
            );
        } else if self.nodes.is_empty() {
            col = col.child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .h(px(40.0))
                    .text_size(px(13.0))
                    .text_color(muted_foreground)
                    .child("暂无数据源"),
            );
        } else {
            let mut list = div()
                .id("tree-list")
                .flex()
                .flex_col()
                .overflow_y_scroll()
                .track_scroll(&self.scroll_handle)
                .gap(px(2.0));

            for (idx, node) in self.nodes.iter().enumerate() {
                match node {
                    TreeNode::DataSource {
                        id,
                        name,
                        host,
                        port,
                        expanded,
                        databases,
                        ..
                    } => {
                        let addr = format!("{}:{}", host, port);
                        let is_selected = matches!(
                            self.selected,
                            Some(TreeSelection::DataSource(ref s)) if s == id
                        );
                        let bg = if is_selected {
                            list_active
                        } else {
                            list_background
                        };

                        let ds_item =
                            div()
                                .id(format!("ds-{id}"))
                                .flex()
                                .items_center()
                                .px(px(12.0))
                                .py(px(6.0))
                                .bg(bg)
                                .hover(move |item| item.bg(list_hover))
                                .cursor(CursorStyle::PointingHand)
                                .child(div().w(px(16.0)).child(if *expanded {
                                    icon_caret_down(foreground)
                                } else {
                                    icon_caret_right(foreground)
                                }))
                                .child(icon_data_source(main.background, main.foreground))
                                .child(
                                    div()
                                        .pl(px(6.0))
                                        .flex()
                                        .flex_col()
                                        .child(div().text_size(px(13.0)).child(name.clone()))
                                        .child(
                                            div()
                                                .text_size(px(11.0))
                                                .text_color(muted_foreground)
                                                .child(addr),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(px(4.0))
                                        .child(
                                            action_button(
                                                format!("ds-edit-{id}"),
                                                "编辑",
                                                ActionRole::Edit,
                                                ActionSize::Row,
                                                style,
                                            )
                                            .on_mouse_down(MouseButton::Left, {
                                                let this_for_edit = this.clone();
                                                let id = *id;
                                                move |_, _, cx| {
                                                    cx.stop_propagation();
                                                    this_for_edit
                                                        .update(cx, |p, cx| {
                                                            p.pending_action =
                                                                Some(PendingAction::Edit(id));
                                                            cx.notify();
                                                        })
                                                        .ok();
                                                }
                                            }),
                                        )
                                        .child(
                                            action_button(
                                                format!("ds-delete-{id}"),
                                                "删除",
                                                ActionRole::Delete,
                                                ActionSize::Row,
                                                style,
                                            )
                                            .on_mouse_down(MouseButton::Left, {
                                                let this_for_delete = this.clone();
                                                let id = *id;
                                                move |_, _, cx| {
                                                    cx.stop_propagation();
                                                    this_for_delete
                                                        .update(cx, |p, cx| {
                                                            p.pending_action =
                                                                Some(PendingAction::Delete(id));
                                                            cx.notify();
                                                        })
                                                        .ok();
                                                }
                                            }),
                                        ),
                                )
                                .on_mouse_down(MouseButton::Left, {
                                    let this_for_ds = this.clone();
                                    let idx = idx;
                                    let id = *id;
                                    move |_, _, cx| {
                                        this_for_ds
                                            .update(cx, |tree, cx| {
                                                tree.toggle_data_source(idx, cx);
                                                tree.select_item(TreeSelection::DataSource(id), cx);
                                            })
                                            .ok();
                                    }
                                });

                        list = list.child(ds_item);

                        if *expanded {
                            for (db_idx, db) in databases.iter().enumerate() {
                                let db_name_owned = db.name.clone();
                                let is_db_selected = matches!(
                                    &self.selected,
                                    Some(TreeSelection::Database { source_id: s, database: d })
                                        if s == id && d == &db.name
                                );
                                let db_bg = if is_db_selected {
                                    list_active
                                } else {
                                    list_background
                                };

                                let db_item = div()
                                    .id(format!("db-{id}-{db_name_owned}"))
                                    .flex()
                                    .items_center()
                                    .pl(px(20.0))
                                    .pr(px(12.0))
                                    .py(px(5.0))
                                    .bg(db_bg)
                                    .border_t_1()
                                    .border_color(border)
                                    .hover(move |item| item.bg(list_hover))
                                    .cursor(CursorStyle::PointingHand)
                                    .child(div().w(px(16.0)).child(if db.expanded {
                                        icon_caret_down(foreground)
                                    } else {
                                        icon_caret_right(foreground)
                                    }))
                                    .child(icon_database(foreground))
                                    .child(
                                        div()
                                            .pl(px(6.0))
                                            .text_size(px(13.0))
                                            .child(db_name_owned.clone()),
                                    )
                                    .on_mouse_down(MouseButton::Left, {
                                        let this_for_db = this.clone();
                                        let source_idx = idx;
                                        let db_idx = db_idx;
                                        let source_id = *id;
                                        let db_name = db.name.clone();
                                        move |_, _, cx| {
                                            this_for_db
                                                .update(cx, |tree, cx| {
                                                    tree.toggle_database(source_idx, db_idx, cx);
                                                    tree.select_item(
                                                        TreeSelection::Database {
                                                            source_id,
                                                            database: db_name.clone(),
                                                        },
                                                        cx,
                                                    );
                                                })
                                                .ok();
                                        }
                                    });

                                list = list.child(db_item);

                                if db.expanded {
                                    for table in &db.tables {
                                        let table_name_owned = table.name.clone();
                                        let is_tbl_selected = matches!(
                                            &self.selected,
                                            Some(TreeSelection::Table { source_id: s, database: d, table: t })
                                                if s == id && d == &db.name && t == &table.name
                                        );
                                        let tbl_bg = if is_tbl_selected {
                                            list_active
                                        } else {
                                            list_background
                                        };

                                        let tbl_item = div()
                                            .id(format!(
                                                "tbl-{id}-{db_name_owned}-{table_name_owned}"
                                            ))
                                            .flex()
                                            .items_center()
                                            .pl(px(40.0))
                                            .pr(px(12.0))
                                            .py(px(5.0))
                                            .bg(tbl_bg)
                                            .border_t_1()
                                            .border_color(border)
                                            .hover(move |item| item.bg(list_hover))
                                            .cursor(CursorStyle::PointingHand)
                                            .child(icon_table(foreground))
                                            .child(
                                                div()
                                                    .pl(px(6.0))
                                                    .text_size(px(12.0))
                                                    .child(table_name_owned),
                                            )
                                            .on_mouse_down(MouseButton::Left, {
                                                let this_for_tbl = this.clone();
                                                let source_id = *id;
                                                let db_name = db.name.clone();
                                                let table_name = table.name.clone();
                                                move |_, _, cx| {
                                                    this_for_tbl
                                                        .update(cx, |tree, cx| {
                                                            tree.select_item(
                                                                TreeSelection::Table {
                                                                    source_id,
                                                                    database: db_name.clone(),
                                                                    table: table_name.clone(),
                                                                },
                                                                cx,
                                                            );
                                                        })
                                                        .ok();
                                                }
                                            });

                                        list = list.child(tbl_item);
                                    }
                                }
                            }
                        }
                    }
                }
            }

            col = col.child(list);
        }

        col
    }
}
