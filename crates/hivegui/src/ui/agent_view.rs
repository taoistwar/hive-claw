use crate::datasource::{Store, entity_store::Agent};
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::ActiveTheme as _;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::scroll::ScrollableElement;

pub struct AgentView {
    store: Entity<Store>,
    items: Vec<Agent>,
    loading: bool,
    search_text: String,
    current_page: i64,
    page_size: i64,
    total_count: i64,
    show_form: bool,
    editing_id: Option<i64>,
    form_identifier: String,
    form_name: String,
    form_description: String,
    form_system_prompt: String,
    form_depth: String,
    form_model_preset: String,
    error_message: Option<String>,
    confirm_delete_id: Option<i64>,
    identifier_input: Option<Entity<InputState>>,
    name_input: Option<Entity<InputState>>,
    description_input: Option<Entity<InputState>>,
    system_prompt_input: Option<Entity<InputState>>,
    depth_input: Option<Entity<InputState>>,
    model_preset_input: Option<Entity<InputState>>,
    search_input: Option<Entity<InputState>>,
}

impl AgentView {
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
            editing_id: None,
            form_identifier: String::new(),
            form_name: String::new(),
            form_description: String::new(),
            form_system_prompt: String::new(),
            form_depth: "0".into(),
            form_model_preset: String::new(),
            error_message: None,
            confirm_delete_id: None,
            identifier_input: None,
            name_input: None,
            description_input: None,
            system_prompt_input: None,
            depth_input: None,
            model_preset_input: None,
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
            let items = Agent::list(store.pool(), search.clone(), 20, offset).await?;
            let count = Agent::count(store.pool(), search).await?;
            this.update(cx, |v, cx| {
                v.items = items;
                v.total_count = count;
                v.loading = false;
                cx.notify();
            })
        })
        .detach();
    }

    fn init_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.identifier_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("唯一标识符")
                .default_value(&self.form_identifier)
        }));
        self.name_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Agent 名称")
                .default_value(&self.form_name)
        }));
        self.description_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("描述（可选）")
                .default_value(&self.form_description)
        }));
        self.system_prompt_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("系统提示词")
                .default_value(&self.form_system_prompt)
        }));
        self.depth_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("0")
                .default_value(&self.form_depth)
        }));
        self.model_preset_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("模型预设（可选）")
                .default_value(&self.form_model_preset)
        }));
    }

    fn show_add_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_form = true;
        self.editing_id = None;
        self.form_identifier.clear();
        self.form_name.clear();
        self.form_description.clear();
        self.form_system_prompt.clear();
        self.form_depth = "0".into();
        self.form_model_preset.clear();
        self.error_message = None;
        self.init_inputs(window, cx);
    }

    fn show_edit_form(&mut self, window: &mut Window, item: Agent, cx: &mut Context<Self>) {
        self.show_form = true;
        self.editing_id = Some(item.id);
        self.form_identifier = item.identifier.clone();
        self.form_name = item.name.clone();
        self.form_description = item.description.clone().unwrap_or_default();
        self.form_system_prompt = item.system_prompt.clone();
        self.form_depth = item.depth.to_string();
        self.form_model_preset = item.model_preset.clone().unwrap_or_default();
        self.error_message = None;
        self.init_inputs(window, cx);
    }

    fn hide_form(&mut self, cx: &mut Context<Self>) {
        self.show_form = false;
        self.editing_id = None;
        self.error_message = None;
        self.identifier_input = None;
        self.name_input = None;
        self.description_input = None;
        self.system_prompt_input = None;
        self.depth_input = None;
        self.model_preset_input = None;
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
        if let Some(ref inp) = self.system_prompt_input {
            self.form_system_prompt = inp.read(cx).value().to_string();
        }
        if let Some(ref inp) = self.depth_input {
            self.form_depth = inp.read(cx).value().to_string();
        }
        if let Some(ref inp) = self.model_preset_input {
            self.form_model_preset = inp.read(cx).value().to_string();
        }
        if self.form_identifier.trim().is_empty() || self.form_name.trim().is_empty() {
            self.error_message = Some("Identifier 和名称不能为空".into());
            cx.notify();
            return;
        }
        let depth: i64 = self.form_depth.parse().unwrap_or(0);
        let store = self.store.read(cx).clone();
        let idf = self.form_identifier.clone();
        let name = self.form_name.clone();
        let desc = if self.form_description.is_empty() {
            None
        } else {
            Some(self.form_description.clone())
        };
        let sp = self.form_system_prompt.clone();
        let mp = if self.form_model_preset.is_empty() {
            None
        } else {
            Some(self.form_model_preset.clone())
        };

        if let Some(eid) = self.editing_id {
            cx.spawn(async move |this, cx| {
                match Agent::update(store.pool(), eid, idf, name, desc, sp, None, depth, mp).await {
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
                match Agent::create(store.pool(), idf, name, desc, sp, None, depth, mp).await {
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
            async move |this, cx| match Agent::delete(store.pool(), id).await {
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

impl Render for AgentView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tp = (self.total_count + self.page_size - 1) / self.page_size;
        if self.show_form && self.identifier_input.is_none() {
            self.init_inputs(window, cx);
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
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .p(px(16.0))
                    .border_b_1()
                    .border_color(rgb(0xe0e0e0))
                    .child(
                        div()
                            .text_size(px(18.0))
                            .font_weight(FontWeight::BOLD)
                            .child("Agent 管理"),
                    )
                    .child(
                        div()
                            .id("add-btn")
                            .px(px(12.0))
                            .py(px(6.0))
                            .rounded(px(4.0))
                            .bg(rgb(0x4a90d9))
                            .text_color(rgb(0xffffff))
                            .text_size(px(13.0))
                            .cursor(CursorStyle::PointingHand)
                            .child("+ 添加 Agent")
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
                    .border_color(rgb(0xe0e0e0))
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
                    .overflow_y_scrollbar()
                    .p(px(16.0))
                    .child(if self.items.is_empty() {
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .h_full()
                            .text_color(rgb(0x999999))
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
                        div()
                            .flex()
                            .flex_col()
                            .border_1()
                            .border_color(rgb(0xe0e0e0))
                            .rounded(px(4.0))
                            .overflow_hidden()
                            .child(
                                div()
                                    .flex()
                                    .bg(rgb(0xf5f5f5))
                                    .border_b_1()
                                    .border_color(rgb(0xe0e0e0))
                                    .child(
                                        div()
                                            .w(col_widths[0])
                                            .px(px(8.0))
                                            .py(px(8.0))
                                            .text_size(px(12.0))
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(rgb(0x333333))
                                            .child("ID"),
                                    )
                                    .child(
                                        div()
                                            .w(col_widths[1])
                                            .px(px(8.0))
                                            .py(px(8.0))
                                            .text_size(px(12.0))
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(rgb(0x333333))
                                            .child("名称"),
                                    )
                                    .child(
                                        div()
                                            .w(col_widths[2])
                                            .px(px(8.0))
                                            .py(px(8.0))
                                            .text_size(px(12.0))
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(rgb(0x333333))
                                            .child("Identifier"),
                                    )
                                    .child(
                                        div()
                                            .w(col_widths[3])
                                            .px(px(8.0))
                                            .py(px(8.0))
                                            .text_size(px(12.0))
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(rgb(0x333333))
                                            .child("Depth"),
                                    )
                                    .child(
                                        div()
                                            .w(col_widths[4])
                                            .px(px(8.0))
                                            .py(px(8.0))
                                            .text_size(px(12.0))
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(rgb(0x333333))
                                            .child("描述"),
                                    )
                                    .child(
                                        div()
                                            .w(col_widths[5])
                                            .px(px(8.0))
                                            .py(px(8.0))
                                            .text_size(px(12.0))
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(rgb(0x333333))
                                            .child("操作"),
                                    ),
                            )
                            .children(self.items.iter().map(|item| {
                                let id = item.id;
                                let ic = item.clone();
                                div()
                                    .flex()
                                    .border_b_1()
                                    .border_color(rgb(0xf0f0f0))
                                    .bg(rgb(0xffffff))
                                    .child(
                                        div()
                                            .w(col_widths[0])
                                            .px(px(8.0))
                                            .py(px(6.0))
                                            .text_size(px(12.0))
                                            .text_color(rgb(0x666666))
                                            .child(format!("{}", item.id)),
                                    )
                                    .child(
                                        div()
                                            .w(col_widths[1])
                                            .px(px(8.0))
                                            .py(px(6.0))
                                            .text_size(px(13.0))
                                            .font_weight(FontWeight::MEDIUM)
                                            .child(item.name.clone()),
                                    )
                                    .child(
                                        div()
                                            .w(col_widths[2])
                                            .px(px(8.0))
                                            .py(px(6.0))
                                            .text_size(px(12.0))
                                            .text_color(rgb(0x666666))
                                            .truncate()
                                            .child(item.identifier.clone()),
                                    )
                                    .child(
                                        div()
                                            .w(col_widths[3])
                                            .px(px(8.0))
                                            .py(px(6.0))
                                            .text_size(px(12.0))
                                            .text_color(rgb(0x666666))
                                            .child(format!("{}", item.depth)),
                                    )
                                    .child(
                                        div()
                                            .w(col_widths[4])
                                            .px(px(8.0))
                                            .py(px(6.0))
                                            .text_size(px(12.0))
                                            .text_color(rgb(0x666666))
                                            .truncate()
                                            .child(if let Some(ref d) = item.description {
                                                d.clone()
                                            } else {
                                                String::new()
                                            }),
                                    )
                                    .child(
                                        div()
                                            .w(col_widths[5])
                                            .px(px(8.0))
                                            .py(px(6.0))
                                            .flex()
                                            .gap(px(4.0))
                                            .child(
                                                div()
                                                    .id(("edit", id as u64))
                                                    .px(px(8.0))
                                                    .py(px(3.0))
                                                    .rounded(px(3.0))
                                                    .bg(rgb(0x5cb85c))
                                                    .text_color(rgb(0xffffff))
                                                    .text_size(px(11.0))
                                                    .cursor(CursorStyle::PointingHand)
                                                    .child("编辑")
                                                    .on_mouse_down(MouseButton::Left, {
                                                        let t = cx.weak_entity();
                                                        move |_, window, cx| {
                                                            t.update(cx, |v, cx| {
                                                                v.show_edit_form(
                                                                    window,
                                                                    ic.clone(),
                                                                    cx,
                                                                )
                                                            })
                                                            .ok();
                                                        }
                                                    }),
                                            )
                                            .child(
                                                div()
                                                    .id(("del", id as u64))
                                                    .px(px(8.0))
                                                    .py(px(3.0))
                                                    .rounded(px(3.0))
                                                    .bg(rgb(0xd9534f))
                                                    .text_color(rgb(0xffffff))
                                                    .text_size(px(11.0))
                                                    .cursor(CursorStyle::PointingHand)
                                                    .child("删除")
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
                    .border_color(rgb(0xe0e0e0))
                    .child(
                        div()
                            .text_size(px(13.0))
                            .text_color(rgb(0x666666))
                            .child(format!("共 {} 条", self.total_count)),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .id("prev")
                                    .px(px(12.0))
                                    .py(px(6.0))
                                    .rounded(px(4.0))
                                    .bg(if self.current_page > 0 {
                                        rgb(0x4a90d9)
                                    } else {
                                        rgb(0xcccccc)
                                    })
                                    .text_color(rgb(0xffffff))
                                    .text_size(px(13.0))
                                    .cursor(if self.current_page > 0 {
                                        CursorStyle::PointingHand
                                    } else {
                                        CursorStyle::Arrow
                                    })
                                    .child("上一页")
                                    .on_mouse_down(MouseButton::Left, {
                                        let t = cx.weak_entity();
                                        move |_, _, cx| {
                                            t.update(cx, |v, cx| v.prev_page(cx)).ok();
                                        }
                                    }),
                            )
                            .child(div().text_size(px(13.0)).child(format!(
                                "第 {} / {} 页",
                                self.current_page + 1,
                                tp.max(1)
                            )))
                            .child(
                                div()
                                    .id("next")
                                    .px(px(12.0))
                                    .py(px(6.0))
                                    .rounded(px(4.0))
                                    .bg(
                                        if (self.current_page + 1) * self.page_size
                                            < self.total_count
                                        {
                                            rgb(0x4a90d9)
                                        } else {
                                            rgb(0xcccccc)
                                        },
                                    )
                                    .text_color(rgb(0xffffff))
                                    .text_size(px(13.0))
                                    .cursor(
                                        if (self.current_page + 1) * self.page_size
                                            < self.total_count
                                        {
                                            CursorStyle::PointingHand
                                        } else {
                                            CursorStyle::Arrow
                                        },
                                    )
                                    .child("下一页")
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
                let system_prompt_input = self.system_prompt_input.clone().unwrap();
                let depth_input = self.depth_input.clone().unwrap();
                let model_preset_input = self.model_preset_input.clone().unwrap();
                this.child(
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
                            let t = cx.weak_entity();
                            move |_, _, cx| {
                                t.update(cx, |v, cx| v.hide_form(cx)).ok();
                            }
                        }),
                )
                .child(
                    div()
                        .absolute()
                        .top(px(80.0))
                        .left(px(50.0))
                        .right(px(50.0))
                        .max_w(px(550.0))
                        .max_h(px(600.0))
                        .bg(rgb(0xffffff))
                        .rounded(px(12.0))
                        .shadow_lg()
                        .border_1()
                        .border_color(rgb(0xdddddd))
                        .p(px(24.0))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                            cx.stop_propagation();
                        })
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(12.0))
                                .overflow_y_scrollbar()
                                .child(
                                    div()
                                        .text_size(px(18.0))
                                        .font_weight(FontWeight::BOLD)
                                        .child(if self.editing_id.is_some() {
                                            "编辑 Agent"
                                        } else {
                                            "添加 Agent"
                                        }),
                                )
                                .child(form_field("Identifier *", identifier_input))
                                .child(form_field("名称 *", name_input))
                                .child(form_field("描述", description_input))
                                .child(form_field("System Prompt", system_prompt_input))
                                .child(form_field("Depth", depth_input))
                                .child(form_field("Model Preset", model_preset_input))
                                .when_some(self.error_message.as_ref(), |this, err| {
                                    this.child(
                                        div()
                                            .p(px(8.0))
                                            .bg(rgb(0xfff3cd))
                                            .rounded(px(4.0))
                                            .text_size(px(12.0))
                                            .text_color(rgb(0x856404))
                                            .child(err.clone()),
                                    )
                                })
                                .child(
                                    div()
                                        .flex()
                                        .justify_end()
                                        .gap(px(8.0))
                                        .child(
                                            div()
                                                .id("cancel")
                                                .px(px(16.0))
                                                .py(px(8.0))
                                                .rounded(px(6.0))
                                                .bg(rgb(0x6c757d))
                                                .text_color(rgb(0xffffff))
                                                .text_size(px(13.0))
                                                .cursor(CursorStyle::PointingHand)
                                                .child("取消")
                                                .on_mouse_down(MouseButton::Left, {
                                                    let t = cx.weak_entity();
                                                    move |_, _, cx| {
                                                        t.update(cx, |v, cx| v.hide_form(cx)).ok();
                                                    }
                                                }),
                                        )
                                        .child(
                                            div()
                                                .id("save")
                                                .px(px(16.0))
                                                .py(px(8.0))
                                                .rounded(px(6.0))
                                                .bg(rgb(0x4a90d9))
                                                .text_color(rgb(0xffffff))
                                                .text_size(px(13.0))
                                                .cursor(CursorStyle::PointingHand)
                                                .child("保存")
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
            .when_some(self.confirm_delete_id, |this, id| {
                this.child(
                    div()
                        .absolute()
                        .top(px(0.0))
                        .left(px(0.0))
                        .right(px(0.0))
                        .bottom(px(0.0))
                        .bg(rgb(0x000000))
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
                                .bg(rgb(0xffffff))
                                .rounded(px(8.0))
                                .shadow_lg()
                                .border_1()
                                .border_color(rgb(0xdddddd))
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
                                                .text_color(rgb(0x333333))
                                                .child("确认删除"),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(14.0))
                                                .text_color(rgb(0x666666))
                                                .child("确定要删除这个 Agent 吗？此操作不可恢复。"),
                                        )
                                        .child(
                                            div()
                                                .flex()
                                                .justify_end()
                                                .gap(px(8.0))
                                                .child(
                                                    div()
                                                        .id("cancel-delete")
                                                        .px(px(16.0))
                                                        .py(px(8.0))
                                                        .rounded(px(6.0))
                                                        .bg(rgb(0x6c757d))
                                                        .text_color(rgb(0xffffff))
                                                        .text_size(px(13.0))
                                                        .cursor(CursorStyle::PointingHand)
                                                        .child("取消")
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
                                                        .id("confirm-delete")
                                                        .px(px(16.0))
                                                        .py(px(8.0))
                                                        .rounded(px(6.0))
                                                        .bg(rgb(0xd9534f))
                                                        .text_color(rgb(0xffffff))
                                                        .text_size(px(13.0))
                                                        .cursor(CursorStyle::PointingHand)
                                                        .child("确认删除")
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

fn form_field(label: &'static str, input: Entity<InputState>) -> impl IntoElement {
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
                .border_color(rgb(0xcccccc))
                .rounded(px(4.0)),
        )
}
