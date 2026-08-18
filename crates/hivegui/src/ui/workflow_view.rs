use crate::datasource::{Store, entity_store::Workflow};
use crate::ui::dag_editor_view::DagEditorView;
use crate::ui::management_style::{
    ActionRole, ActionSize, ManagementStyle, action_button, list_actions, list_cell,
    list_container, list_header, list_header_cell, list_row, management_modal_layer,
    management_modal_panel, management_modal_scroll,
};
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::ActiveTheme as _;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::scroll::ScrollableElement;

pub struct WorkflowView {
    store: Entity<Store>,
    items: Vec<Workflow>,
    loading: bool,
    search_text: String,
    current_page: i64,
    page_size: i64,
    total_count: i64,
    show_form: bool,
    form_scroll: ScrollHandle,
    editing_id: Option<i64>,
    form_identifier: String,
    form_name: String,
    form_description: String,
    form_timeout_ms: String,
    error_message: Option<String>,
    confirm_delete_id: Option<i64>,
    identifier_input: Option<Entity<InputState>>,
    name_input: Option<Entity<InputState>>,
    description_input: Option<Entity<InputState>>,
    timeout_input: Option<Entity<InputState>>,
    search_input: Option<Entity<InputState>>,
    // DAG 编辑器状态
    show_dag_editor: bool,
    dag_editor_workflow_id: Option<i64>,
    dag_editor_view: Option<Entity<DagEditorView>>,
    form_focus: FocusHandle,
}

impl WorkflowView {
    pub fn new(store: Entity<Store>, cx: &mut Context<Self>) -> Self {
        let mut v = Self {
            store,
            items: Vec::new(),
            loading: false,
            search_text: String::new(),
            current_page: 0,
            page_size: 20,
            total_count: 0,
            show_form: false,
            form_scroll: ScrollHandle::default(),
            editing_id: None,
            form_identifier: String::new(),
            form_name: String::new(),
            form_description: String::new(),
            form_timeout_ms: "30000".into(),
            error_message: None,
            confirm_delete_id: None,
            identifier_input: None,
            name_input: None,
            description_input: None,
            timeout_input: None,
            search_input: None,
            show_dag_editor: false,
            dag_editor_workflow_id: None,
            dag_editor_view: None,
            form_focus: cx.focus_handle(),
        };
        v.load(cx);
        v
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        let store = self.store.read(cx).clone();
        let search = if self.search_text.is_empty() {
            None
        } else {
            Some(self.search_text.clone())
        };
        let offset = self.current_page * self.page_size;
        cx.spawn(async move |this, cx| {
            let items = Workflow::list(store.pool(), search.clone(), 20, offset).await?;
            let count = Workflow::count(store.pool(), search).await?;
            this.update(cx, |v, cx| {
                v.items = items;
                v.total_count = count;
                v.loading = false;
                cx.notify();
            })
        })
        .detach();
    }

    fn show_add_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_form = true;
        self.editing_id = None;
        self.form_identifier.clear();
        self.form_name.clear();
        self.form_description.clear();
        self.form_timeout_ms = "30000".into();
        self.error_message = None;

        // 初始化输入框
        self.identifier_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("唯一标识符")
                .default_value("")
        }));
        self.name_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("工作流名称")
                .default_value("")
        }));
        self.description_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("描述（可选）")
                .default_value("")
        }));
        self.timeout_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("30000")
                .default_value("30000")
        }));

        cx.notify();
    }

    fn show_edit_form(&mut self, window: &mut Window, item: Workflow, cx: &mut Context<Self>) {
        self.show_form = true;
        self.editing_id = Some(item.id);
        self.form_identifier = item.identifier.clone();
        self.form_name = item.name.clone();
        self.form_description = item.description.clone().unwrap_or_default();
        self.form_timeout_ms = item.timeout_ms.to_string();
        self.error_message = None;

        // 初始化输入框
        self.identifier_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("唯一标识符")
                .default_value(&item.identifier)
        }));
        self.name_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("工作流名称")
                .default_value(&item.name)
        }));
        self.description_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("描述（可选）")
                .default_value(item.description.unwrap_or_default())
        }));
        self.timeout_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("30000")
                .default_value(item.timeout_ms.to_string())
        }));

        cx.notify();
    }

    /// Keyboard handler for the workflow form. Esc closes the form,
    /// Enter submits when the form is visible.
    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if !self.show_form {
            return;
        }
        match event.keystroke.key.as_str() {
            "escape" => self.hide_form(cx),
            "enter" => self.save(cx),
            _ => {}
        }
    }

    fn hide_form(&mut self, cx: &mut Context<Self>) {
        self.form_scroll.set_offset(point(px(0.0), px(0.0)));
        self.show_form = false;
        self.editing_id = None;
        self.error_message = None;
        cx.notify();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        // 同步输入框值
        if let Some(ref inp) = self.identifier_input {
            self.form_identifier = inp.read(cx).value().to_string();
        }
        if let Some(ref inp) = self.name_input {
            self.form_name = inp.read(cx).value().to_string();
        }
        if let Some(ref inp) = self.description_input {
            self.form_description = inp.read(cx).value().to_string();
        }
        if let Some(ref inp) = self.timeout_input {
            self.form_timeout_ms = inp.read(cx).value().to_string();
        }

        if self.form_identifier.trim().is_empty() || self.form_name.trim().is_empty() {
            self.error_message = Some("Identifier 和名称不能为空".into());
            cx.notify();
            return;
        }
        let timeout: i64 = self.form_timeout_ms.parse().unwrap_or(30000);
        let store = self.store.read(cx).clone();
        let idf = self.form_identifier.clone();
        let name = self.form_name.clone();
        let desc = if self.form_description.is_empty() {
            None
        } else {
            Some(self.form_description.clone())
        };

        if let Some(eid) = self.editing_id {
            cx.spawn(async move |this, cx| {
                match Workflow::update(
                    store.pool(),
                    eid,
                    idf,
                    name,
                    desc,
                    timeout,
                    None,
                    None,
                    None,
                    None,
                    None,
                )
                .await
                {
                    Ok(_) => {
                        this.update(cx, |v, cx| {
                            v.hide_form(cx);
                            v.load(cx);
                        })
                        .ok();
                    }
                    Err(e) => {
                        this.update(cx, |v, cx| {
                            v.error_message = Some(format!("更新失败: {}", e));
                            cx.notify();
                        })
                        .ok();
                    }
                }
            })
            .detach();
        } else {
            cx.spawn(async move |this, cx| {
                match Workflow::create(
                    store.pool(),
                    idf,
                    name,
                    desc,
                    timeout,
                    None,
                    None,
                    None,
                    None,
                    None,
                )
                .await
                {
                    Ok(_) => {
                        this.update(cx, |v, cx| {
                            v.hide_form(cx);
                            v.load(cx);
                        })
                        .ok();
                    }
                    Err(e) => {
                        this.update(cx, |v, cx| {
                            v.error_message = Some(format!("创建失败: {}", e));
                            cx.notify();
                        })
                        .ok();
                    }
                }
            })
            .detach();
        }
    }

    fn delete(&mut self, id: i64, cx: &mut Context<Self>) {
        let store = self.store.read(cx).clone();
        cx.spawn(
            async move |this, cx| match Workflow::delete(store.pool(), id).await {
                Ok(_) => {
                    this.update(cx, |v, cx| {
                        v.load(cx);
                    })
                    .ok();
                }
                Err(e) => {
                    this.update(cx, |v, cx| {
                        v.error_message = Some(format!("删除失败: {}", e));
                        cx.notify();
                    })
                    .ok();
                }
            },
        )
        .detach();
    }

    fn next_page(&mut self, cx: &mut Context<Self>) {
        if (self.current_page + 1) * self.page_size < self.total_count {
            self.current_page += 1;
            self.load(cx);
        }
    }
    fn prev_page(&mut self, cx: &mut Context<Self>) {
        if self.current_page > 0 {
            self.current_page -= 1;
            self.load(cx);
        }
    }

    fn show_dag_editor(&mut self, workflow_id: i64, cx: &mut Context<Self>) {
        self.show_dag_editor = true;
        self.dag_editor_workflow_id = Some(workflow_id);
        self.dag_editor_view =
            Some(cx.new(|cx| DagEditorView::new(self.store.clone(), workflow_id, cx)));
        cx.notify();
    }

    fn hide_dag_editor(&mut self, cx: &mut Context<Self>) {
        self.show_dag_editor = false;
        self.dag_editor_workflow_id = None;
        self.dag_editor_view = None;
        cx.notify();
    }
}

impl Render for WorkflowView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tp = (self.total_count + self.page_size - 1) / self.page_size;

        if self.search_input.is_none() {
            self.search_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("输入名称...")
                    .default_value(&self.search_text)
            }));
            if let Some(ref input) = self.search_input {
                cx.subscribe_in(input, window, |this, state, event, _window, cx| {
                    if let InputEvent::Change = event {
                        this.search_text = state.read(cx).value().to_string();
                        this.current_page = 0;
                        this.load(cx);
                    }
                })
                .detach();
            }
        }

        // 确保输入框已初始化
        if self.show_form && self.identifier_input.is_none() {
            self.identifier_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("唯一标识符")
                    .default_value(&self.form_identifier)
            }));
            self.name_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("工作流名称")
                    .default_value(&self.form_name)
            }));
            self.description_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("描述（可选）")
                    .default_value(&self.form_description)
            }));
            self.timeout_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("30000")
                    .default_value(&self.form_timeout_ms)
            }));
        }

        let style = ManagementStyle::current(cx);
        let theme = cx.theme();

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .p(px(16.0))
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .text_size(px(18.0))
                            .font_weight(FontWeight::BOLD)
                            .child("工作流管理"),
                    )
                    .child(
                        action_button(
                            "add-btn",
                            "+ 添加工作流",
                            ActionRole::Main,
                            ActionSize::Page,
                            style,
                        )
                        .on_mouse_down(MouseButton::Left, {
                            let t = cx.weak_entity();
                            move |_, window, cx| {
                                t.update(cx, |v, cx| v.show_add_form(window, cx)).ok();
                            }
                        }),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .p(px(12.0))
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(div().text_size(px(13.0)).child("搜索:"))
                            .child(
                                div()
                                    .w(px(200.0))
                                    .child(Input::new(&self.search_input.clone().unwrap())),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .p(px(16.0))
                    .child(if self.items.is_empty() {
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .h_full()
                            .text_color(style.list.muted_foreground)
                            .text_size(px(14.0))
                            .child("暂无数据")
                    } else {
                        let col_widths = [px(60.0), px(120.0), px(100.0), px(120.0), px(120.0)];
                        list_container(style)
                            .child(
                                list_header(style)
                                    .child(list_header_cell(Some(col_widths[0]), style).child("ID"))
                                    .child(
                                        list_header_cell(Some(col_widths[1]), style).child("名称"),
                                    )
                                    .child(
                                        list_header_cell(Some(col_widths[2]), style)
                                            .child("Identifier"),
                                    )
                                    .child(
                                        list_header_cell(Some(col_widths[3]), style)
                                            .child("Timeout"),
                                    )
                                    .child(
                                        list_header_cell(Some(col_widths[4]), style).child("操作"),
                                    ),
                            )
                            .children(self.items.iter().map(|item| {
                                let id = item.id;
                                let ic = item.clone();
                                list_row(style)
                                    .child(
                                        list_cell(Some(col_widths[0]), style)
                                            .child(format!("{}", item.id)),
                                    )
                                    .child(
                                        list_cell(Some(col_widths[1]), style)
                                            .child(item.name.clone()),
                                    )
                                    .child(
                                        list_cell(Some(col_widths[2]), style)
                                            .child(item.identifier.clone()),
                                    )
                                    .child(
                                        list_cell(Some(col_widths[3]), style)
                                            .child(format!("{}ms", item.timeout_ms)),
                                    )
                                    .child(
                                        list_actions(None, style)
                                            .child(
                                                action_button(
                                                    ("dag", id as u64),
                                                    "编辑DAG",
                                                    ActionRole::Edit,
                                                    ActionSize::Row,
                                                    style,
                                                )
                                                .on_mouse_down(MouseButton::Left, {
                                                    let t = cx.weak_entity();
                                                    move |_, _, cx| {
                                                        t.update(cx, |v, cx| {
                                                            v.show_dag_editor(id, cx);
                                                        })
                                                        .ok();
                                                    }
                                                }),
                                            )
                                            .child(
                                                action_button(
                                                    ("edit", id as u64),
                                                    "编辑",
                                                    ActionRole::Edit,
                                                    ActionSize::Row,
                                                    style,
                                                )
                                                .on_mouse_down(MouseButton::Left, {
                                                    let t = cx.weak_entity();
                                                    move |_, window, cx| {
                                                        t.update(cx, |v, cx| {
                                                            v.show_edit_form(window, ic.clone(), cx)
                                                        })
                                                        .ok();
                                                    }
                                                }),
                                            )
                                            .child(
                                                action_button(
                                                    ("del", id as u64),
                                                    "删除",
                                                    ActionRole::Delete,
                                                    ActionSize::Row,
                                                    style,
                                                )
                                                .on_mouse_down(MouseButton::Left, {
                                                    let t = cx.weak_entity();
                                                    move |_, _, cx| {
                                                        t.update(cx, |v, cx| {
                                                            v.confirm_delete_id = Some(id);
                                                            cx.notify();
                                                        })
                                                        .ok();
                                                    }
                                                }),
                                            ),
                                    )
                            }))
                    }),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_between()
                    .p(px(12.0))
                    .border_t_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .text_size(px(13.0))
                            .text_color(style.list.muted_foreground)
                            .child(format!("共 {} 条", self.total_count)),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(8.0))
                            .items_center()
                            .child(
                                action_button(
                                    "prev",
                                    "上一页",
                                    if self.current_page > 0 {
                                        ActionRole::Neutral
                                    } else {
                                        ActionRole::Disabled
                                    },
                                    ActionSize::Page,
                                    style,
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    {
                                        let t = cx.weak_entity();
                                        move |_, _, cx| {
                                            t.update(cx, |v, cx| v.prev_page(cx)).ok();
                                        }
                                    },
                                ),
                            )
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .text_color(style.list.muted_foreground)
                                    .child(format!(
                                        "第 {} / {} 页",
                                        self.current_page + 1,
                                        tp.max(1)
                                    )),
                            )
                            .child(
                                action_button(
                                    "next",
                                    "下一页",
                                    if (self.current_page + 1) * self.page_size < self.total_count {
                                        ActionRole::Neutral
                                    } else {
                                        ActionRole::Disabled
                                    },
                                    ActionSize::Page,
                                    style,
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    {
                                        let t = cx.weak_entity();
                                        move |_, _, cx| {
                                            t.update(cx, |v, cx| v.next_page(cx)).ok();
                                        }
                                    },
                                ),
                            ),
                    ),
            )
            .when(self.show_form, |this| {
                let identifier_input = self.identifier_input.clone().unwrap();
                let name_input = self.name_input.clone().unwrap();
                let description_input = self.description_input.clone().unwrap();
                let timeout_input = self.timeout_input.clone().unwrap();

                this.child(
                    div()
                        .absolute()
                        .top(px(0.0))
                        .left(px(0.0))
                        .right(px(0.0))
                        .bottom(px(0.0))
                        .bg(theme.overlay)
                        .opacity(0.3)
                        .cursor(CursorStyle::PointingHand)
                        .on_mouse_down(MouseButton::Left, {
                            let t = cx.weak_entity();
                            move |_, _, cx| {
                                t.update(cx, |v, cx| v.hide_form(cx)).ok();
                            }
                        }),
                )
                .child(
                    management_modal_panel(
                        management_modal_layer(px(500.0)),
                        theme.popover,
                        theme.foreground,
                        theme.border,
                    )
                    .track_focus(&self.form_focus)
                    .on_key_down(cx.listener(|v, event: &KeyDownEvent, window, cx| {
                        v.on_key_down(event, window, cx);
                    }))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .child(
                        management_modal_scroll("workflow-form-scroll", &self.form_scroll)
                            .gap(px(12.0))
                            .child(
                                div()
                                    .text_size(px(18.0))
                                    .font_weight(FontWeight::BOLD)
                                    .child(if self.editing_id.is_some() {
                                        "编辑工作流"
                                    } else {
                                        "添加工作流"
                                    }),
                            )
                            .child(form_field("Identifier *", identifier_input, theme))
                            .child(form_field("名称 *", name_input, theme))
                            .child(form_field("描述", description_input, theme))
                            .child(form_field("Timeout (ms)", timeout_input, theme))
                            .when_some(self.error_message.as_ref(), |this, err| {
                                this.child(
                                    div()
                                        .p(px(8.0))
                                        .bg(theme.warning.opacity(0.1))
                                        .rounded(px(4.0))
                                        .text_size(px(12.0))
                                        .text_color(theme.warning)
                                        .child(err.clone()),
                                )
                            })
                            .child(
                                div()
                                    .flex()
                                    .justify_end()
                                    .gap(px(8.0))
                                    .child(
                                        action_button(
                                            "cancel",
                                            "取消",
                                            ActionRole::Neutral,
                                            ActionSize::Page,
                                            style,
                                        )
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            {
                                                let t = cx.weak_entity();
                                                move |_, _, cx| {
                                                    t.update(cx, |v, cx| v.hide_form(cx)).ok();
                                                }
                                            },
                                        ),
                                    )
                                    .child(
                                        action_button(
                                            "save",
                                            "保存",
                                            ActionRole::Main,
                                            ActionSize::Page,
                                            style,
                                        )
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            {
                                                let t = cx.weak_entity();
                                                move |_, _, cx| {
                                                    t.update(cx, |v, cx| v.save(cx)).ok();
                                                }
                                            },
                                        ),
                                    ),
                            ),
                    ),
                )
            })
            .when(self.confirm_delete_id.is_some(), |this| {
                let _id = self.confirm_delete_id.unwrap();
                this.child(
                    div()
                        .absolute()
                        .top(px(0.0))
                        .left(px(0.0))
                        .right(px(0.0))
                        .bottom(px(0.0))
                        .bg(theme.overlay)
                        .opacity(0.3)
                        .on_mouse_down(MouseButton::Left, {
                            let t = cx.weak_entity();
                            move |_, _, cx| {
                                t.update(cx, |v, cx| {
                                    v.confirm_delete_id = None;
                                    cx.notify();
                                })
                                .ok();
                            }
                        }),
                )
                .child(
                    div()
                        .absolute()
                        .top(px(0.0))
                        .left(px(0.0))
                        .right(px(0.0))
                        .bottom(px(0.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .w(px(400.0))
                                .bg(theme.popover)
                                .rounded(px(8.0))
                                .shadow_lg()
                                .border_1()
                                .border_color(theme.border)
                                .p(px(24.0))
                                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                    cx.stop_propagation();
                                })
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(px(16.0))
                                        .child(
                                            div()
                                                .text_size(px(18.0))
                                                .font_weight(FontWeight::BOLD)
                                                .text_color(theme.foreground)
                                                .child("确认删除"),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(14.0))
                                                .text_color(style.list.muted_foreground)
                                                .child("确定要删除这个工作流吗？此操作不可恢复。"),
                                        )
                                        .child(
                                            div()
                                                .flex()
                                                .justify_end()
                                                .gap(px(8.0))
                                                .child(
                                                    action_button(
                                                        "cancel-delete",
                                                        "取消",
                                                        ActionRole::Neutral,
                                                        ActionSize::Page,
                                                        style,
                                                    )
                                                    .on_mouse_down(MouseButton::Left, {
                                                        let t = cx.weak_entity();
                                                        move |_, _, cx| {
                                                            t.update(cx, |v, cx| {
                                                                v.confirm_delete_id = None;
                                                                cx.notify();
                                                            })
                                                            .ok();
                                                        }
                                                    }),
                                                )
                                                .child(
                                                    action_button(
                                                        "confirm-delete",
                                                        "确认删除",
                                                        ActionRole::Delete,
                                                        ActionSize::Page,
                                                        style,
                                                    )
                                                    .on_mouse_down(MouseButton::Left, {
                                                        let t = cx.weak_entity();
                                                        move |_, _, cx| {
                                                            t.update(cx, |v, cx| {
                                                                if let Some(did) =
                                                                    v.confirm_delete_id
                                                                {
                                                                    v.delete(did, cx);
                                                                    v.confirm_delete_id = None;
                                                                }
                                                            })
                                                            .ok();
                                                        }
                                                    }),
                                                ),
                                        ),
                                ),
                        ),
                )
            })
            .when(self.show_dag_editor, |this| {
                this.child(
                    div()
                        .absolute()
                        .top(px(0.0))
                        .left(px(0.0))
                        .right(px(0.0))
                        .bottom(px(0.0))
                        .bg(theme.overlay)
                        .opacity(0.3)
                        .on_mouse_down(MouseButton::Left, {
                            let t = cx.weak_entity();
                            move |_, _, cx| {
                                t.update(cx, |v, cx| v.hide_dag_editor(cx)).ok();
                            }
                        }),
                )
                .child(
                    div()
                        .absolute()
                        .top(px(24.0))
                        .bottom(px(24.0))
                        .left(px(24.0))
                        .right(px(24.0))
                        .bg(theme.popover)
                        .rounded(px(12.0))
                        .shadow_lg()
                        .border_1()
                        .border_color(theme.border)
                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                            cx.stop_propagation();
                        })
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .size_full()
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .p(px(16.0))
                                        .border_b_1()
                                        .border_color(theme.border)
                                        .child(
                                            div()
                                                .text_size(px(18.0))
                                                .font_weight(FontWeight::BOLD)
                                                .child("编辑 DAG"),
                                        )
                                        .child(
                                            action_button(
                                                "close-dag",
                                                "关闭",
                                                ActionRole::Neutral,
                                                ActionSize::Page,
                                                style,
                                            )
                                            .on_mouse_down(MouseButton::Left, {
                                                let t = cx.weak_entity();
                                                move |_, _, cx| {
                                                    t.update(cx, |v, cx| v.hide_dag_editor(cx))
                                                        .ok();
                                                }
                                            }),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_1()
                                        .min_h_0()
                                        .overflow_hidden()
                                        .when_some(self.dag_editor_view.clone(), |this, editor| {
                                            this.child(editor)
                                        }),
                                ),
                        ),
                )
            })
    }
}

fn form_field(
    label: &'static str,
    input: Entity<InputState>,
    theme: &gpui_component::theme::Theme,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(4.0))
        .child(div().text_size(px(13.0)).child(label))
        .child(
            Input::new(&input)
                .w_full()
                .h(px(32.0))
                .px(px(8.0))
                .border_1()
                .border_color(theme.border)
                .rounded(px(4.0)),
        )
}
