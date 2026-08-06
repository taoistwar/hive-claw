use gpui::{Context, CursorStyle, Entity, MouseButton, Window, div, prelude::*, px, rgb};

use crate::datasource::{DataSource, Store};
use crate::ui::{
    datasource_form::{DataSourceForm, FormMode},
    table_viewer::TableViewer,
    tree_nav::{ErrorModal, PendingAction, TreeNav, TreeSelection},
};

#[derive(Clone)]
struct SplitterDrag {
    splitter_index: usize,
    start_x: f32,
    start_widths: (f32, f32),
}

impl gpui::Global for SplitterDrag {}

const MIN_WIDTH: f32 = 100.0;
const DEFAULT_LEFT_WIDTH: f32 = 280.0;
const DEFAULT_MID_WIDTH: f32 = 200.0;

pub struct DataSourceView {
    store: Entity<Store>,
    tree: Entity<TreeNav>,
    viewer: Entity<TableViewer>,
    prev_tree_selection: Option<TreeSelection>,
    form: Option<Entity<DataSourceForm>>,
    left_width: f32,
    #[expect(
        dead_code,
        reason = "retained for the pending second splitter in the datasource layout"
    )]
    mid_width: f32,
    error_modal: Option<ErrorModal>,
}

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
            mid_width: DEFAULT_MID_WIDTH,
            error_modal: None,
        }
    }

    fn sync(&mut self, cx: &mut Context<Self>) {
        if let Some(action) = self.tree.read(cx).pending_action() {
            match action {
                PendingAction::Add => {
                    self.form = Some(cx.new(|cx| {
                        DataSourceForm::new(FormMode::Add, self.store.clone(), None, cx)
                    }));
                }
                PendingAction::Edit(id) => {
                    let node = self.tree.read(cx).nodes.iter()
                        .find(|n| {
                            matches!(n, crate::ui::tree_nav::TreeNode::DataSource { id: ds_id, .. } if *ds_id == id)
                        })
                        .cloned();
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
                        let ds = DataSource {
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
                                cx,
                            )
                        }));
                    }
                }
                PendingAction::Delete(id) => {
                    let store = self.store.read(cx).clone();
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

        if let Some(ref form_entity) = self.form
            && form_entity.read(cx).is_done()
        {
            self.form = None;
            self.tree.update(cx, |t, cx| {
                t.refresh(cx);
            });
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
                        let node = self.tree.read(cx).nodes.iter()
                            .find(|n| {
                                matches!(n, crate::ui::tree_nav::TreeNode::DataSource { id: ds_id, .. } if *ds_id == *source_id)
                            })
                            .cloned();
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
                            let ds = DataSource {
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

impl Render for DataSourceView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync(cx);

        let form = self.form.clone();

        let left_width = self.left_width;
        let this = cx.weak_entity();

        let splitter_0 = div()
            .id("splitter-0")
            .w(px(4.0))
            .flex_shrink_0()
            .cursor(CursorStyle::ResizeColumn)
            .hover(|el| el.bg(rgb(0xc0c0c0)))
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
            .id("datasource-view-root")
            .flex()
            .flex_row()
            .size_full()
            .on_mouse_move({
                let this = this.clone();
                move |event: &gpui::MouseMoveEvent, _window, cx| {
                    if !cx.has_global::<SplitterDrag>() {
                        return;
                    }
                    let drag = cx.global::<SplitterDrag>().clone();
                    let delta: f32 = (event.position.x - gpui::px(drag.start_x)).into();
                    if drag.splitter_index == 0 {
                        let new_left = (drag.start_widths.0 + delta).max(MIN_WIDTH);
                        this.update(cx, |v, cx| {
                            v.left_width = new_left;
                            cx.notify();
                        })
                        .ok();
                    }
                }
            })
            .on_mouse_up(MouseButton::Left, {
                move |_event, _window, cx| {
                    if cx.has_global::<SplitterDrag>() {
                        cx.remove_global::<SplitterDrag>();
                    }
                }
            })
            .child(
                div()
                    .w(px(left_width))
                    .h_full()
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .border_r_1()
                    .border_color(rgb(0xe0e0e0))
                    .child(self.tree.clone()),
            )
            .child(splitter_0)
            .child(div().flex_grow().h_full().child(self.viewer.clone()));

        if let Some(form_entity) = form {
            root = root.child(form_entity);
        }

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
                    .bg(rgb(0x000000))
                    .opacity(0.3)
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
