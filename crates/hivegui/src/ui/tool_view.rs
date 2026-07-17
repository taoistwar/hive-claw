use crate::datasource::{Store, entity_store::Tool};
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

pub struct ToolView {
    store: Entity<Store>,
    items: Vec<Tool>,
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
    form_kind: String,
    form_source: String,
    form_function_id: String,
    form_workflow_id: String,
    form_input_schema: String,
    form_output_schema: String,
    error_message: Option<String>,
    confirm_delete_id: Option<i64>,
    identifier_input: Option<Entity<InputState>>,
    name_input: Option<Entity<InputState>>,
    description_input: Option<Entity<InputState>>,
    kind_input: Option<Entity<InputState>>,
    source_input: Option<Entity<InputState>>,
    function_id_input: Option<Entity<InputState>>,
    workflow_id_input: Option<Entity<InputState>>,
    input_schema_input: Option<Entity<InputState>>,
    output_schema_input: Option<Entity<InputState>>,
    search_input: Option<Entity<InputState>>,
}

impl ToolView {
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
            form_kind: "1".into(),
            form_source: "workspace".into(),
            form_function_id: String::new(),
            form_workflow_id: String::new(),
            form_input_schema: "{}".into(),
            form_output_schema: "{}".into(),
            error_message: None,
            confirm_delete_id: None,
            identifier_input: None,
            name_input: None,
            description_input: None,
            kind_input: None,
            source_input: None,
            function_id_input: None,
            workflow_id_input: None,
            input_schema_input: None,
            output_schema_input: None,
            search_input: None,
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
            let items = Tool::list(store.pool(), search.clone(), 20, offset).await?;
            let count = Tool::count(store.pool(), search).await?;
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
        self.form_kind = "1".into();
        self.form_source = "workspace".into();
        self.form_function_id.clear();
        self.form_workflow_id.clear();
        self.form_input_schema = "{}".into();
        self.form_output_schema = "{}".into();
        self.error_message = None;
        self.identifier_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("唯一标识符")
                .default_value("")
        }));
        self.name_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("工具名称")
                .default_value("")
        }));
        self.description_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("工具描述")
                .default_value("")
        }));
        self.kind_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("1")
                .default_value("1")
        }));
        self.source_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("workspace")
                .default_value("workspace")
        }));
        self.function_id_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("函数 ID")
                .default_value("")
        }));
        self.workflow_id_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("工作流 ID")
                .default_value("")
        }));
        self.input_schema_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("{}")
                .default_value("{}")
        }));
        self.output_schema_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("{}")
                .default_value("{}")
        }));
        cx.notify();
    }

    fn show_edit_form(&mut self, window: &mut Window, item: Tool, cx: &mut Context<Self>) {
        self.show_form = true;
        self.editing_id = Some(item.id);
        self.form_identifier = item.identifier.clone();
        self.form_name = item.name.clone();
        self.form_description = item.description.clone();
        self.form_kind = item.kind.to_string();
        self.form_source = item.source.clone();
        self.form_function_id = item.function_id.map(|i| i.to_string()).unwrap_or_default();
        self.form_workflow_id = item.workflow_id.map(|i| i.to_string()).unwrap_or_default();
        self.form_input_schema = item.input_schema.clone();
        self.form_output_schema = item.output_schema.clone();
        self.error_message = None;
        self.identifier_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("唯一标识符")
                .default_value(&item.identifier)
        }));
        self.name_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("工具名称")
                .default_value(&item.name)
        }));
        self.description_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("工具描述")
                .default_value(&item.description)
        }));
        self.kind_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("1")
                .default_value(&item.kind.to_string())
        }));
        self.source_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("workspace")
                .default_value(&item.source)
        }));
        self.function_id_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("函数 ID")
                .default_value(&item.function_id.map(|i| i.to_string()).unwrap_or_default())
        }));
        self.workflow_id_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("工作流 ID")
                .default_value(&item.workflow_id.map(|i| i.to_string()).unwrap_or_default())
        }));
        self.input_schema_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("{}")
                .default_value(&item.input_schema)
        }));
        self.output_schema_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("{}")
                .default_value(&item.output_schema)
        }));
        cx.notify();
    }

    fn hide_form(&mut self, cx: &mut Context<Self>) {
        self.form_scroll
            .set_offset(point(px(0.0), px(0.0)));
        self.show_form = false;
        self.editing_id = None;
        self.error_message = None;
        cx.notify();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        if let Some(ref inp) = self.identifier_input {
            self.form_identifier = inp.read(cx).value().to_string();
        }
        if let Some(ref inp) = self.name_input {
            self.form_name = inp.read(cx).value().to_string();
        }
        if let Some(ref inp) = self.description_input {
            self.form_description = inp.read(cx).value().to_string();
        }
        if let Some(ref inp) = self.kind_input {
            self.form_kind = inp.read(cx).value().to_string();
        }
        if let Some(ref inp) = self.source_input {
            self.form_source = inp.read(cx).value().to_string();
        }
        if let Some(ref inp) = self.function_id_input {
            self.form_function_id = inp.read(cx).value().to_string();
        }
        if let Some(ref inp) = self.workflow_id_input {
            self.form_workflow_id = inp.read(cx).value().to_string();
        }
        if let Some(ref inp) = self.input_schema_input {
            self.form_input_schema = inp.read(cx).value().to_string();
        }
        if let Some(ref inp) = self.output_schema_input {
            self.form_output_schema = inp.read(cx).value().to_string();
        }
        if self.form_identifier.trim().is_empty()
            || self.form_name.trim().is_empty()
            || self.form_description.trim().is_empty()
        {
            self.error_message = Some("Identifier、名称和描述不能为空".into());
            cx.notify();
            return;
        }
        let kind: i64 = self.form_kind.parse().unwrap_or(1);
        let fid: Option<i64> = if self.form_function_id.is_empty() {
            None
        } else {
            self.form_function_id.parse().ok()
        };
        let wid: Option<i64> = if self.form_workflow_id.is_empty() {
            None
        } else {
            self.form_workflow_id.parse().ok()
        };
        let store = self.store.read(cx).clone();
        let idf = self.form_identifier.clone();
        let name = self.form_name.clone();
        let desc = self.form_description.clone();
        let src = self.form_source.clone();
        let is = self.form_input_schema.clone();
        let os = self.form_output_schema.clone();

        if let Some(eid) = self.editing_id {
            cx.spawn(async move |this, cx| {
                match Tool::update(
                    store.pool(),
                    eid,
                    idf,
                    name,
                    desc,
                    kind,
                    src,
                    false,
                    fid,
                    wid,
                    is,
                    os,
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
                match Tool::create(
                    store.pool(),
                    idf,
                    name,
                    desc,
                    kind,
                    src,
                    false,
                    fid,
                    wid,
                    is,
                    os,
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
            async move |this, cx| match Tool::delete(store.pool(), id).await {
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
}

impl Render for ToolView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tp = (self.total_count + self.page_size - 1) / self.page_size;
        if self.show_form && self.identifier_input.is_none() {
            self.identifier_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("唯一标识符")
                    .default_value(&self.form_identifier)
            }));
            self.name_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("工具名称")
                    .default_value(&self.form_name)
            }));
            self.description_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("工具描述")
                    .default_value(&self.form_description)
            }));
            self.kind_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("1")
                    .default_value(&self.form_kind)
            }));
            self.source_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("workspace")
                    .default_value(&self.form_source)
            }));
            self.function_id_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("函数 ID")
                    .default_value(&self.form_function_id)
            }));
            self.workflow_id_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("工作流 ID")
                    .default_value(&self.form_workflow_id)
            }));
            self.input_schema_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("{}")
                    .default_value(&self.form_input_schema)
            }));
            self.output_schema_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("{}")
                    .default_value(&self.form_output_schema)
            }));
        }

        if self.search_input.is_none() {
            self.search_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("输入名称...")
                    .default_value(&self.search_text)
            }));
            if let Some(ref input) = self.search_input {
                cx.subscribe_in(input, window, |this, state, event, window, cx| {
                    if let InputEvent::Change = event {
                        this.search_text = state.read(cx).value().to_string();
                        this.current_page = 0;
                        this.load(cx);
                    }
                })
                .detach();
            }
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
                            .child("工具管理"),
                    )
                    .child(
                        action_button(
                            "add-btn",
                            "+ 添加工具",
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
                        let col_widths = [
                            px(60.0),
                            px(120.0),
                            px(100.0),
                            px(80.0),
                            px(150.0),
                            px(120.0),
                        ];
                        list_container(style)
                            .child(
                                list_header(style)
                                    .child(list_header_cell(Some(col_widths[0]), style).child("ID"))
                                    .child(list_header_cell(Some(col_widths[1]), style).child("名称"))
                                    .child(list_header_cell(Some(col_widths[2]), style).child("Identifier"))
                                    .child(list_header_cell(Some(col_widths[3]), style).child("Kind"))
                                    .child(list_header_cell(Some(col_widths[4]), style).child("描述"))
                                    .child(list_header_cell(Some(col_widths[5]), style).child("操作")),
                            )
                            .children(self.items.iter().map(|item| {
                                let id = item.id;
                                let ic = item.clone();
                                list_row(style)
                                    .child(list_cell(Some(col_widths[0]), style).child(format!("{}", item.id)))
                                    .child(list_cell(Some(col_widths[1]), style).child(item.name.clone()))
                                    .child(list_cell(Some(col_widths[2]), style).child(item.identifier.clone()))
                                    .child(list_cell(Some(col_widths[3]), style).child(format!("{}", item.kind)))
                                    .child(list_cell(Some(col_widths[4]), style).child(item.description.clone()))
                                    .child(
                                        list_actions(None, style)
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
                                .on_mouse_down(MouseButton::Left, {
                                    let t = cx.weak_entity();
                                    move |_, _, cx| {
                                        t.update(cx, |v, cx| v.prev_page(cx)).ok();
                                    }
                                }),
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
                                .on_mouse_down(MouseButton::Left, {
                                    let t = cx.weak_entity();
                                    move |_, _, cx| {
                                        t.update(cx, |v, cx| v.next_page(cx)).ok();
                                    }
                                }),
                            ),
                    ),
            )
            .when(self.show_form, |this| {
                let identifier_input = self.identifier_input.clone().unwrap();
                let name_input = self.name_input.clone().unwrap();
                let description_input = self.description_input.clone().unwrap();
                let kind_input = self.kind_input.clone().unwrap();
                let source_input = self.source_input.clone().unwrap();
                let function_id_input = self.function_id_input.clone().unwrap();
                let workflow_id_input = self.workflow_id_input.clone().unwrap();
                let input_schema_input = self.input_schema_input.clone().unwrap();
                let output_schema_input = self.output_schema_input.clone().unwrap();
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
                        management_modal_layer(px(550.0)),
                        theme.popover,
                        theme.foreground,
                        theme.border,
                    )
                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                            cx.stop_propagation();
                        })
                        .child(
                            management_modal_scroll("tool-form-scroll", &self.form_scroll)
                                .gap(px(10.0))
                                .child(
                                    div()
                                        .text_size(px(18.0))
                                        .font_weight(FontWeight::BOLD)
                                        .child(if self.editing_id.is_some() {
                                            "编辑工具"
                                        } else {
                                            "添加工具"
                                        }),
                                )
                                .child(form_field("Identifier *", identifier_input, theme))
                                .child(form_field("名称 *", name_input, theme))
                                .child(form_field("描述 *", description_input, theme))
                                .child(form_field("Kind (1=function, 2=workflow)", kind_input, theme))
                                .child(form_field("Source", source_input, theme))
                                .child(form_field("Function ID (kind=1)", function_id_input, theme))
                                .child(form_field("Workflow ID (kind=2)", workflow_id_input, theme))
                                .child(form_field("Input Schema (JSON)", input_schema_input, theme))
                                .child(form_field("Output Schema (JSON)", output_schema_input, theme))
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
                                            .on_mouse_down(MouseButton::Left, {
                                                let t = cx.weak_entity();
                                                move |_, _, cx| {
                                                    t.update(cx, |v, cx| v.hide_form(cx)).ok();
                                                }
                                            }),
                                        )
                                        .child(
                                            action_button(
                                                "save",
                                                "保存",
                                                ActionRole::Main,
                                                ActionSize::Page,
                                                style,
                                            )
                                            .on_mouse_down(MouseButton::Left, {
                                                let t = cx.weak_entity();
                                                move |_, _, cx| {
                                                    t.update(cx, |v, cx| v.save(cx)).ok();
                                                }
                                            }),
                                        ),
                                ),
                        ),
                )
            })
            .when(self.confirm_delete_id.is_some(), |this| {
                let id = self.confirm_delete_id.unwrap();
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
                                                .child("确定要删除这个工具吗？此操作不可恢复。"),
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
    }
}

fn form_field(label: &'static str, input: Entity<InputState>, theme: &gpui_component::theme::Theme) -> impl IntoElement {
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
