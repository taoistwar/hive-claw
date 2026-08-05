use gpui::{
    AnyElement, Context, CursorStyle, Entity, MouseButton, ScrollHandle, SharedString, Window, div,
    prelude::*, px, rgb,
};

use crate::datasource::{MysqlClient, Store, TableInfo};

fn icon_caret_right() -> AnyElement {
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
                .child(
                    div()
                        .w(px(2.0))
                        .h(px(1.5))
                        .bg(rgb(0x555555))
                        .rounded(px(0.75)),
                )
                .child(
                    div()
                        .w(px(4.0))
                        .h(px(1.5))
                        .bg(rgb(0x555555))
                        .rounded(px(0.75)),
                )
                .child(
                    div()
                        .w(px(6.0))
                        .h(px(1.5))
                        .bg(rgb(0x555555))
                        .rounded(px(0.75)),
                ),
        )
        .into_any()
}

fn icon_caret_down() -> AnyElement {
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
                .child(
                    div()
                        .w(px(1.5))
                        .h(px(2.0))
                        .bg(rgb(0x555555))
                        .rounded(px(0.75)),
                )
                .child(
                    div()
                        .w(px(1.5))
                        .h(px(4.0))
                        .bg(rgb(0x555555))
                        .rounded(px(0.75)),
                )
                .child(
                    div()
                        .w(px(1.5))
                        .h(px(6.0))
                        .bg(rgb(0x555555))
                        .rounded(px(0.75)),
                ),
        )
        .into_any()
}

fn icon_database() -> AnyElement {
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
                .border_color(rgb(0x555555))
                .flex()
                .flex_col()
                .items_center()
                .justify_between()
                .py(px(1.0))
                .child(
                    div()
                        .w(px(10.0))
                        .h(px(1.5))
                        .bg(rgb(0x555555))
                        .rounded(px(0.75)),
                )
                .child(
                    div()
                        .w(px(10.0))
                        .h(px(1.5))
                        .bg(rgb(0x555555))
                        .rounded(px(0.75)),
                )
                .child(
                    div()
                        .w(px(10.0))
                        .h(px(1.5))
                        .bg(rgb(0x555555))
                        .rounded(px(0.75)),
                ),
        )
        .into_any()
}

fn icon_data_source() -> AnyElement {
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
                .bg(rgb(0x4a90d9))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .w(px(8.0))
                        .h(px(8.0))
                        .rounded(px(1.0))
                        .border_1()
                        .border_color(rgb(0xffffff))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .w(px(4.0))
                                .h(px(4.0))
                                .rounded(px(2.0))
                                .bg(rgb(0xffffff)),
                        ),
                ),
        )
        .into_any()
}

fn icon_table() -> AnyElement {
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
                .border_color(rgb(0x555555))
                .flex()
                .flex_col()
                .child(
                    div()
                        .w_full()
                        .h(px(3.0))
                        .border_b_1()
                        .border_color(rgb(0x555555))
                        .flex()
                        .child(
                            div()
                                .w(px(6.0))
                                .h_full()
                                .border_r_1()
                                .border_color(rgb(0x555555)),
                        ),
                )
                .child(
                    div()
                        .w_full()
                        .h(px(3.0))
                        .border_b_1()
                        .border_color(rgb(0x555555))
                        .flex()
                        .child(
                            div()
                                .w(px(6.0))
                                .h_full()
                                .border_r_1()
                                .border_color(rgb(0x555555)),
                        ),
                )
                .child(
                    div().w_full().h(px(3.0)).flex().child(
                        div()
                            .w(px(6.0))
                            .h_full()
                            .border_r_1()
                            .border_color(rgb(0x555555)),
                    ),
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
                            && let Some(db) = databases.get_mut(db_index)
                        {
                            db.tables = tables;
                            db.expanded = true;
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
        let mut col = div().flex().flex_col().size_full().bg(rgb(0xfafafa)).child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .px(px(12.0))
                .py(px(8.0))
                .border_b_1()
                .border_color(rgb(0xe0e0e0))
                .child(div().text_size(px(14.0)).child("数据源"))
                .child(
                    div()
                        .id("btn-add")
                        .px(px(8.0))
                        .py(px(4.0))
                        .rounded(px(4.0))
                        .bg(rgb(0x4a90d9))
                        .text_size(px(12.0))
                        .text_color(rgb(0xffffff))
                        .cursor(CursorStyle::PointingHand)
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
                        })
                        .child("+ 添加"),
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
                    .text_color(rgb(0x888888))
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
                    .text_color(rgb(0x888888))
                    .child("暂无数据源"),
            );
        } else {
            let mut list = div()
                .id("tree-list")
                .flex()
                .flex_col()
                .flex_grow()
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
                            rgb(0xd0e0f0)
                        } else {
                            rgb(0xfafafa)
                        };

                        let ds_item = div()
                            .id(format!("ds-{id}"))
                            .flex()
                            .items_center()
                            .px(px(12.0))
                            .py(px(6.0))
                            .bg(bg)
                            .cursor(CursorStyle::PointingHand)
                            .child(div().w(px(16.0)).child(if *expanded {
                                icon_caret_down()
                            } else {
                                icon_caret_right()
                            }))
                            .child(icon_data_source())
                            .child(
                                div()
                                    .flex_grow()
                                    .pl(px(6.0))
                                    .flex()
                                    .flex_col()
                                    .child(div().text_size(px(13.0)).child(name.clone()))
                                    .child(
                                        div()
                                            .text_size(px(11.0))
                                            .text_color(rgb(0x888888))
                                            .child(addr),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(4.0))
                                    .child(
                                        div()
                                            .id(format!("ds-edit-{id}"))
                                            .px(px(6.0))
                                            .py(px(2.0))
                                            .rounded(px(3.0))
                                            .bg(rgb(0xffaa00))
                                            .text_size(px(11.0))
                                            .text_color(rgb(0xffffff))
                                            .cursor(CursorStyle::PointingHand)
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
                                            })
                                            .child("编辑"),
                                    )
                                    .child(
                                        div()
                                            .id(format!("ds-delete-{id}"))
                                            .px(px(6.0))
                                            .py(px(2.0))
                                            .rounded(px(3.0))
                                            .bg(rgb(0xdd4444))
                                            .text_size(px(11.0))
                                            .text_color(rgb(0xffffff))
                                            .cursor(CursorStyle::PointingHand)
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
                                            })
                                            .child("删除"),
                                    ),
                            )
                            .on_mouse_down(MouseButton::Left, {
                                let this_for_ds = this.clone();
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
                                    rgb(0xd0e0f0)
                                } else {
                                    rgb(0xfafafa)
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
                                    .border_color(rgb(0xf0f0f0))
                                    .cursor(CursorStyle::PointingHand)
                                    .child(div().w(px(16.0)).child(if db.expanded {
                                        icon_caret_down()
                                    } else {
                                        icon_caret_right()
                                    }))
                                    .child(icon_database())
                                    .child(
                                        div()
                                            .flex_grow()
                                            .pl(px(6.0))
                                            .text_size(px(13.0))
                                            .child(db_name_owned.clone()),
                                    )
                                    .on_mouse_down(MouseButton::Left, {
                                        let this_for_db = this.clone();
                                        let source_idx = idx;
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
                                            rgb(0xd0e0f0)
                                        } else {
                                            rgb(0xffffff)
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
                                            .border_color(rgb(0xf0f0f0))
                                            .cursor(CursorStyle::PointingHand)
                                            .child(icon_table())
                                            .child(
                                                div()
                                                    .flex_grow()
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
