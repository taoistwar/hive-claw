use crate::datasource::{Store, entity_store::Capability};
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::ActiveTheme as _;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::scroll::ScrollableElement;

pub struct CapabilityView {
    store: Entity<Store>,
    items: Vec<Capability>,
    loading: bool,
    search_text: String,
    current_page: i64,
    page_size: i64,
    total_count: i64,
    show_form: bool,
    editing_name: Option<String>,
    form_name: String,
    form_description: String,
    form_is_dangerous: bool,
    error_message: Option<String>,
    confirm_delete_name: Option<String>,
    name_input: Option<Entity<InputState>>,
    description_input: Option<Entity<InputState>>,
    search_input: Option<Entity<InputState>>,
}

impl CapabilityView {
    pub fn new(store: Entity<Store>, cx: &mut Context<Self>) -> Self {
        let mut view = Self {
            store,
            items: Vec::new(),
            loading: false,
            search_text: String::new(),
            current_page: 0,
            page_size: 20,
            total_count: 0,
            show_form: false,
            editing_name: None,
            form_name: String::new(),
            form_description: String::new(),
            form_is_dangerous: false,
            error_message: None,
            confirm_delete_name: None,
            name_input: None,
            description_input: None,
            search_input: None,
        };
        view.load(cx);
        view
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
            match async {
                let items = Capability::list(store.pool(), search.clone(), 20, offset).await?;
                let count = Capability::count(store.pool(), search).await?;
                Ok::<(Vec<Capability>, i64), anyhow::Error>((items, count))
            }
            .await
            {
                Ok((items, count)) => {
                    this.update(cx, |v, cx| {
                        v.items = items;
                        v.total_count = count;
                        v.loading = false;
                        cx.notify();
                    })
                    .ok();
                }
                Err(e) => {
                    this.update(cx, |v, cx| {
                        v.error_message = Some(format!("加载失败: {}", e));
                        v.loading = false;
                        cx.notify();
                    })
                    .ok();
                }
            }
        })
        .detach();
    }

    fn show_add_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_form = true;
        self.editing_name = None;
        self.form_name = String::new();
        self.form_description = String::new();
        self.form_is_dangerous = false;
        self.error_message = None;

        self.name_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("输入能力名称")
                .default_value("")
        }));
        self.description_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("输入描述")
                .default_value("")
        }));

        cx.notify();
    }

    fn show_edit_form(&mut self, window: &mut Window, item: Capability, cx: &mut Context<Self>) {
        self.show_form = true;
        self.editing_name = Some(item.name.clone());
        self.form_name = item.name.clone();
        self.form_description = item.description.clone();
        self.form_is_dangerous = item.is_dangerous;
        self.error_message = None;

        self.name_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("输入能力名称")
                .default_value(&item.name)
        }));
        self.description_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("输入描述")
                .default_value(&item.description)
        }));

        cx.notify();
    }

    fn hide_form(&mut self, cx: &mut Context<Self>) {
        self.show_form = false;
        self.editing_name = None;
        self.form_name.clear();
        self.form_description.clear();
        self.form_is_dangerous = false;
        self.error_message = None;
        self.name_input = None;
        self.description_input = None;
        cx.notify();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        // 同步输入框状态到表单字段
        if let Some(ref input) = self.name_input {
            self.form_name = input.read(cx).value().to_string();
        }
        if let Some(ref input) = self.description_input {
            self.form_description = input.read(cx).value().to_string();
        }

        if self.form_name.trim().is_empty() || self.form_description.trim().is_empty() {
            self.error_message = Some("名称和描述不能为空".into());
            cx.notify();
            return;
        }
        let store = self.store.read(cx).clone();
        let name = self.form_name.clone();
        let desc = self.form_description.clone();
        let dangerous = self.form_is_dangerous;
        if let Some(old_name) = &self.editing_name {
            let old = old_name.clone();
            cx.spawn(async move |this, cx| {
                match Capability::update(store.pool(), &old, name, desc, dangerous, None).await {
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
                match Capability::create(store.pool(), name, desc, dangerous, None).await {
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

    fn delete(&mut self, name: String, cx: &mut Context<Self>) {
        let store = self.store.read(cx).clone();
        cx.spawn(
            async move |this, cx| match Capability::delete(store.pool(), &name).await {
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

impl Render for CapabilityView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let total_pages = (self.total_count + self.page_size - 1) / self.page_size;

        // 如果表单显示但输入框未初始化，则初始化
        if self.show_form && self.name_input.is_none() {
            self.name_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("输入能力名称")
                    .default_value(&self.form_name)
            }));
            self.description_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("输入描述")
                    .default_value(&self.form_description)
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
                            .child("能力管理"),
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
                            .child("+ 添加能力")
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
                        let col_widths = [px(150.0), px(200.0), px(80.0), px(120.0)];
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
                                            .child("名称"),
                                    )
                                    .child(
                                        div()
                                            .w(col_widths[1])
                                            .px(px(8.0))
                                            .py(px(8.0))
                                            .text_size(px(12.0))
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(rgb(0x333333))
                                            .child("描述"),
                                    )
                                    .child(
                                        div()
                                            .w(col_widths[2])
                                            .px(px(8.0))
                                            .py(px(8.0))
                                            .text_size(px(12.0))
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(rgb(0x333333))
                                            .child("危险"),
                                    )
                                    .child(
                                        div()
                                            .w(col_widths[3])
                                            .px(px(8.0))
                                            .py(px(8.0))
                                            .text_size(px(12.0))
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(rgb(0x333333))
                                            .child("操作"),
                                    ),
                            )
                            .children(self.items.iter().map(|item| {
                                let item_clone = item.clone();
                                let del_name = item.name.clone();
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
                                            .text_size(px(13.0))
                                            .font_weight(FontWeight::MEDIUM)
                                            .child(item.name.clone()),
                                    )
                                    .child(
                                        div()
                                            .w(col_widths[1])
                                            .px(px(8.0))
                                            .py(px(6.0))
                                            .text_size(px(12.0))
                                            .text_color(rgb(0x666666))
                                            .truncate()
                                            .child(item.description.clone()),
                                    )
                                    .child(
                                        div()
                                            .w(col_widths[2])
                                            .px(px(8.0))
                                            .py(px(6.0))
                                            .text_size(px(12.0))
                                            .child(if item.is_dangerous {
                                                div()
                                                    .px(px(6.0))
                                                    .py(px(2.0))
                                                    .rounded(px(3.0))
                                                    .bg(rgb(0xd9534f))
                                                    .text_color(rgb(0xffffff))
                                                    .text_size(px(11.0))
                                                    .child("危险")
                                            } else {
                                                div().child("否").text_color(rgb(0x666666))
                                            }),
                                    )
                                    .child(
                                        div()
                                            .w(col_widths[3])
                                            .px(px(8.0))
                                            .py(px(6.0))
                                            .flex()
                                            .gap(px(4.0))
                                            .child(
                                                div()
                                                    .id(format!("edit-{}", item.name))
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
                                                        let item_clone = item.clone();
                                                        move |_, window, cx| {
                                                            t.update(cx, |v, cx| {
                                                                v.show_edit_form(
                                                                    window,
                                                                    item_clone.clone(),
                                                                    cx,
                                                                )
                                                            })
                                                            .ok();
                                                        }
                                                    }),
                                            )
                                            .child(
                                                div()
                                                    .id(format!("del-{}", del_name))
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
                                                        let name = del_name.clone();
                                                        move |_, _, cx| {
                                                            t.update(cx, |v, cx| {
                                                                v.confirm_delete_name =
                                                                    Some(name.clone());
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
                                total_pages.max(1)
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
                        .top(px(100.0))
                        .left(px(50.0))
                        .right(px(50.0))
                        .max_w(px(500.0))
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
                                .gap(px(16.0))
                                .overflow_y_scrollbar()
                                .child(
                                    div()
                                        .text_size(px(18.0))
                                        .font_weight(FontWeight::BOLD)
                                        .child(if self.editing_name.is_some() {
                                            "编辑能力"
                                        } else {
                                            "添加能力"
                                        }),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(px(8.0))
                                        .child(div().text_size(px(13.0)).child("名称 *"))
                                        .child(
                                            Input::new(self.name_input.as_ref().unwrap())
                                                .w_full()
                                                .h(px(32.0))
                                                .px(px(8.0))
                                                .border_1()
                                                .border_color(rgb(0xcccccc))
                                                .rounded(px(4.0)),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(px(8.0))
                                        .child(div().text_size(px(13.0)).child("描述 *"))
                                        .child(
                                            Input::new(self.description_input.as_ref().unwrap())
                                                .w_full()
                                                .h(px(60.0))
                                                .px(px(8.0))
                                                .py(px(4.0))
                                                .border_1()
                                                .border_color(rgb(0xcccccc))
                                                .rounded(px(4.0)),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(8.0))
                                        .child(div().text_size(px(13.0)).child("危险能力"))
                                        .child(if self.form_is_dangerous {
                                            div()
                                                .id("toggle-danger")
                                                .px(px(8.0))
                                                .py(px(4.0))
                                                .rounded(px(3.0))
                                                .bg(rgb(0xd9534f))
                                                .text_color(rgb(0xffffff))
                                                .text_size(px(12.0))
                                                .cursor(CursorStyle::PointingHand)
                                                .child("是")
                                                .on_mouse_down(MouseButton::Left, {
                                                    let t = cx.weak_entity();
                                                    move |_, _, cx| {
                                                        t.update(cx, |v, cx| {
                                                            v.form_is_dangerous = false;
                                                            cx.notify();
                                                        })
                                                        .ok();
                                                    }
                                                })
                                        } else {
                                            div()
                                                .id("toggle-danger")
                                                .px(px(8.0))
                                                .py(px(4.0))
                                                .rounded(px(3.0))
                                                .bg(rgb(0xcccccc))
                                                .text_size(px(12.0))
                                                .cursor(CursorStyle::PointingHand)
                                                .child("否")
                                                .on_mouse_down(MouseButton::Left, {
                                                    let t = cx.weak_entity();
                                                    move |_, _, cx| {
                                                        t.update(cx, |v, cx| {
                                                            v.form_is_dangerous = true;
                                                            cx.notify();
                                                        })
                                                        .ok();
                                                    }
                                                })
                                        }),
                                )
                                .when_some(self.error_message.as_ref(), |this, err| {
                                    this.child(
                                        div()
                                            .p(px(8.0))
                                            .bg(rgb(0xfff3cd))
                                            .border_1()
                                            .border_color(rgb(0xffeaa7))
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
            .when(self.confirm_delete_name.is_some(), |this| {
                let name = self.confirm_delete_name.clone().unwrap();
                let name_clone = name.clone();
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
                            let this = cx.weak_entity();
                            move |_, _, cx| {
                                this.update(cx, |view, cx| {
                                    view.confirm_delete_name = None;
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
                                                .child(format!(
                                                    "确定要删除能力 '{}' 吗？此操作不可恢复。",
                                                    name_clone
                                                )),
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
                                                                    v.confirm_delete_name = None;
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
                                                            let name = name.clone();
                                                            move |_, _, cx| {
                                                                t.update(cx, |v, cx| {
                                                                    let name = name.clone();
                                                                    v.delete(name, cx);
                                                                    v.confirm_delete_name = None;
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
