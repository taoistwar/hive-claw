//! Legacy `DataSourceView` (uppercase S) wrapper, kept for
//! `utility_view.rs` and other call sites that have not yet been
//! migrated to the new T038/T039 store. New code MUST use
//! [`crate::ui::datasource_view::DatasourceView`] (lowercase `s`).

#![allow(dead_code)]

use gpui::{Context, CursorStyle, Entity, MouseButton, Window, div, prelude::*, px};
use gpui_component::ActiveTheme as _;

use crate::datasource::Store;
use crate::ui::{
    datasource_form::{DataSourceForm, FormMode},
    management_style::{
        ActionRole, ActionSize, ManagementStyle, action_button, management_modal_layer,
        management_modal_panel,
    },
    table_viewer::TableViewer,
    tree_nav::{ErrorModal, PendingAction, TreeNav, TreeSelection},
};

/// Legacy view used by the `Tools` tab. The new T039 keyboard
/// surface lives in `t039_view::DatasourceView`.
pub struct DataSourceView {
    pub store: Entity<Store>,
    pub tree: Entity<TreeNav>,
    pub viewer: Entity<TableViewer>,
    pub prev_tree_selection: Option<TreeSelection>,
    pub form: Option<Entity<DataSourceForm>>,
    pub left_width: f32,
    pub mid_width: f32,
    pub error_modal: Option<ErrorModal>,
}

#[derive(Clone)]
pub struct SplitterDrag {
    pub splitter_index: usize,
    pub start_x: f32,
    pub start_widths: (f32, f32),
}

impl gpui::Global for SplitterDrag {}

const MIN_WIDTH: f32 = 100.0;
const DEFAULT_LEFT_WIDTH: f32 = 280.0;

impl DataSourceView {
    pub fn new(store: Entity<Store>, cx: &mut Context<Self>) -> Self {
        let tree = cx.new(|cx| {
            let mut t = TreeNav::new(cx);
            t.set_store(store.clone(), cx);
            t
        });
        let viewer = cx.new(TableViewer::new);
        Self {
            store,
            tree,
            viewer,
            prev_tree_selection: None,
            form: None,
            left_width: DEFAULT_LEFT_WIDTH,
            mid_width: 200.0,
            error_modal: None,
        }
    }
}

impl gpui::Render for DataSourceView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync(window, cx);
        let style = ManagementStyle::current(cx);
        let (overlay, popover, popover_foreground, border, danger, splitter_hover) = {
            let theme = cx.theme();
            (
                theme.overlay,
                theme.popover,
                theme.popover_foreground,
                theme.border,
                theme.danger,
                theme.list_hover,
            )
        };
        let form = self.form.clone();
        let left_width = self.left_width;
        let this = cx.weak_entity();
        let splitter_0 = div()
            .id("legacy-splitter-0")
            .w(px(4.0))
            .flex_shrink_0()
            .cursor(CursorStyle::ResizeColumn)
            .hover(move |el| el.bg(splitter_hover))
            .on_mouse_down(MouseButton::Left, {
                let current_left = left_width;
                move |event: &gpui::MouseDownEvent, _window, cx| {
                    cx.set_global(SplitterDrag {
                        splitter_index: 0,
                        start_x: event.position.x.into(),
                        start_widths: (current_left, 0.0),
                    });
                }
            });
        let mut root = div()
            .id("legacy-datasource-view-root")
            .flex()
            .flex_row()
            .size_full()
            .child(
                div()
                    .w(px(left_width))
                    .h_full()
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .border_r_1()
                    .border_color(border)
                    .child(self.tree.clone()),
            )
            .child(splitter_0)
            .child(
                div()
                    .h_full()
                    .flex_1()
                    .min_h_0()
                    .child(self.viewer.clone())
                    .when_some(form, |this, form_entity| this.child(form_entity)),
            );
        if let Some(ref error_modal) = self.error_modal {
            let title = error_modal.title.clone();
            let message = error_modal.message.clone();
            let this_for_modal = this.clone();
            root = root.child(
                div()
                    .absolute()
                    .top(px(0.0))
                    .left(px(0.0))
                    .right(px(0.0))
                    .bottom(px(0.0))
                    .bg(overlay)
                    .cursor(CursorStyle::PointingHand)
                    .on_mouse_down(MouseButton::Left, {
                        let this_for_bg = this.clone();
                        move |_, _, cx| {
                            this_for_bg
                                .update(cx, |v, cx| {
                                    v.error_modal = None;
                                    cx.notify();
                                })
                                .ok();
                        }
                    })
                    .child(
                        management_modal_panel(
                            div()
                                .absolute()
                                .top(px(24.0))
                                .left(px(0.0))
                                .right(px(0.0))
                                .mx_auto()
                                .w_full()
                                .max_w(px(500.0)),
                            popover,
                            popover_foreground,
                            border,
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(16.0))
                                .child(div().text_size(px(18.0)).text_color(danger).child(title))
                                .child(
                                    div()
                                        .text_size(px(14.0))
                                        .text_color(popover_foreground)
                                        .child(message),
                                )
                                .child(
                                    div().flex().justify_end().child(
                                        action_button(
                                            "close-error-modal",
                                            "关闭",
                                            ActionRole::Main,
                                            ActionSize::Dialog,
                                            style,
                                        )
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            move |_, _, cx| {
                                                this_for_modal
                                                    .update(cx, |v, cx| {
                                                        v.error_modal = None;
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
        root
    }
}

impl DataSourceView {
    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(action) = self.tree.read(cx).pending_action() {
            match action {
                PendingAction::Add => {
                    self.form = Some(cx.new(|cx| {
                        DataSourceForm::new(FormMode::Add, self.store.clone(), None, window, cx)
                    }));
                }
                PendingAction::Edit(id) => {
                    let node = self.tree.read(cx).nodes.iter().find(|n| {
                        matches!(n, crate::ui::tree_nav::TreeNode::DataSource { id: ds_id, .. } if *ds_id == id)
                    }).cloned();
                    if let Some(crate::ui::tree_nav::TreeNode::DataSource {
                        id,
                        name,
                        host,
                        port,
                        username,
                        encrypted_password,
                        ..
                    }) = node
                    {
                        use chrono::Utc;
                        let ds = crate::datasource::models::DataSource {
                            id,
                            name,
                            host,
                            port,
                            username,
                            encrypted_password,
                            created_at: Utc::now(),
                            updated_at: Utc::now(),
                        };
                        self.form = Some(cx.new(|cx| {
                            DataSourceForm::new(
                                FormMode::Edit(id),
                                self.store.clone(),
                                Some(&ds),
                                window,
                                cx,
                            )
                        }));
                    }
                }
                PendingAction::Delete(id) => {
                    let store = self.store.read(cx).clone();
                    let _this = cx.weak_entity();
                    cx.spawn(async move |this, cx| {
                        let _ = store.delete(id).await;
                        this.update(cx, |view, cx| {
                            view.tree.update(cx, |t, cx| {
                                t.refresh(cx);
                            });
                            view.viewer.update(cx, |v, cx| {
                                v.clear(cx);
                            });
                        })
                        .ok();
                    })
                    .detach();
                }
            }
            self.tree.update(cx, |t, cx| {
                t.clear_pending_action(cx);
            });
        }
        if let Some(ref form_entity) = self.form {
            if form_entity.read(cx).is_done() {
                self.form = None;
                self.tree.update(cx, |t, cx| {
                    t.refresh(cx);
                });
            }
        }
        if let Some(modal) = self.tree.update(cx, |t, _| t.take_error_modal()) {
            self.error_modal = Some(modal);
        }
        let current_tree_sel = self.tree.read(cx).selection().cloned();
        if current_tree_sel != self.prev_tree_selection {
            if let Some(ref selection) = current_tree_sel {
                match selection {
                    TreeSelection::Table {
                        source_id,
                        database,
                        table,
                    } => {
                        let node = self.tree.read(cx).nodes.iter().find(|n| {
                            matches!(n, crate::ui::tree_nav::TreeNode::DataSource { id: ds_id, .. } if *ds_id == *source_id)
                        }).cloned();
                        if let Some(crate::ui::tree_nav::TreeNode::DataSource {
                            id,
                            name,
                            host,
                            port,
                            username,
                            encrypted_password,
                            ..
                        }) = node
                        {
                            use chrono::Utc;
                            let ds = crate::datasource::models::DataSource {
                                id,
                                name,
                                host,
                                port,
                                username,
                                encrypted_password,
                                created_at: Utc::now(),
                                updated_at: Utc::now(),
                            };
                            let store = self.store.clone();
                            self.viewer.update(cx, |viewer, cx| {
                                viewer.set_table(
                                    ds,
                                    store,
                                    *source_id,
                                    database.clone(),
                                    table.clone(),
                                    cx,
                                );
                            });
                        }
                    }
                    TreeSelection::DataSource(_) | TreeSelection::Database { .. } => {
                        self.viewer.update(cx, |viewer, cx| {
                            viewer.clear(cx);
                        });
                    }
                }
            }
            self.prev_tree_selection = current_tree_sel;
        }
    }
}
