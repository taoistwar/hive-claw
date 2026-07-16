use crate::datasource::{Store, entity_store::Tag};
use crate::ui::table_viewer::TableViewer;
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::ActiveTheme as _;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::scroll::ScrollableElement;

pub struct TagView {
    store: Entity<Store>,
    viewer: Entity<TableViewer>,
    tags: Vec<Tag>,
    loading: bool,
    search_text: String,
    current_page: i64,
    page_size: i64,
    total_count: i64,
    show_form: bool,
    editing_tag: Option<Tag>,
    form_name: String,
    form_color: String,
    error_message: Option<String>,
    confirm_delete_id: Option<i64>,
    name_input: Option<Entity<InputState>>,
    color_input: Option<Entity<InputState>>,
    search_input: Option<Entity<InputState>>,
}

impl TagView {
    pub fn new(store: Entity<Store>, cx: &mut Context<Self>) -> Self {
        let viewer = cx.new(TableViewer::new);
        let mut view = Self {
            store: store.clone(),
            viewer,
            tags: Vec::new(),
            loading: false,
            search_text: String::new(),
            current_page: 0,
            page_size: 20,
            total_count: 0,
            show_form: false,
            editing_tag: None,
            form_name: String::new(),
            form_color: String::new(),
            error_message: None,
            confirm_delete_id: None,
            name_input: None,
            color_input: None,
            search_input: None,
        };
        view.load_tags(cx);
        view
    }

    fn load_tags(&mut self, cx: &mut Context<Self>) {
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
                let tags = Tag::list(store.pool(), search.clone(), 20, offset).await?;
                let count = Tag::count(store.pool(), search).await?;
                Ok::<(Vec<Tag>, i64), anyhow::Error>((tags, count))
            }
            .await
            {
                Ok((tags, count)) => {
                    this.update(cx, |view, cx| {
                        view.tags = tags;
                        view.total_count = count;
                        view.loading = false;
                        cx.notify();
                    })
                    .ok();
                }
                Err(e) => {
                    this.update(cx, |view, cx| {
                        view.error_message = Some(format!("加载失败: {}", e));
                        view.loading = false;
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
        self.editing_tag = None;
        self.form_name = String::new();
        self.form_color = String::new();
        self.error_message = None;

        self.name_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("输入标签名称")
                .default_value("")
        }));
        self.color_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("输入颜色代码（可选）")
                .default_value("")
        }));

        cx.notify();
    }

    fn show_edit_form(&mut self, window: &mut Window, tag: Tag, cx: &mut Context<Self>) {
        self.show_form = true;
        self.editing_tag = Some(tag.clone());
        self.form_name = tag.name.clone();
        self.form_color = tag.color.clone().unwrap_or_default();
        self.error_message = None;

        self.name_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("输入标签名称")
                .default_value(&tag.name)
        }));
        self.color_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("输入颜色代码（可选）")
                .default_value(&tag.color.unwrap_or_default())
        }));

        cx.notify();
    }

    fn hide_form(&mut self, cx: &mut Context<Self>) {
        self.show_form = false;
        self.editing_tag = None;
        self.form_name = String::new();
        self.form_color = String::new();
        self.error_message = None;
        self.name_input = None;
        self.color_input = None;
        cx.notify();
    }

    fn save_tag(&mut self, cx: &mut Context<Self>) {
        // Sync input state to form fields
        if let Some(ref inp) = self.name_input {
            self.form_name = inp.read(cx).value().to_string();
        }
        if let Some(ref inp) = self.color_input {
            self.form_color = inp.read(cx).value().to_string();
        }

        if self.form_name.trim().is_empty() {
            self.error_message = Some("名称不能为空".to_string());
            cx.notify();
            return;
        }

        let store = self.store.read(cx).clone();
        let name = self.form_name.clone();
        let color = if self.form_color.is_empty() {
            None
        } else {
            Some(self.form_color.clone())
        };

        if let Some(editing) = &self.editing_tag {
            let id = editing.id;
            cx.spawn(async move |this, cx| {
                match Tag::update(store.pool(), id, name, color).await {
                    Ok(_) => {
                        this.update(cx, |view, cx| {
                            view.hide_form(cx);
                            view.load_tags(cx);
                        })
                        .ok();
                    }
                    Err(e) => {
                        this.update(cx, |view, cx| {
                            view.error_message = Some(format!("更新失败: {}", e));
                            cx.notify();
                        })
                        .ok();
                    }
                }
            })
            .detach();
        } else {
            cx.spawn(
                async move |this, cx| match Tag::create(store.pool(), name, color).await {
                    Ok(_) => {
                        this.update(cx, |view, cx| {
                            view.hide_form(cx);
                            view.load_tags(cx);
                        })
                        .ok();
                    }
                    Err(e) => {
                        this.update(cx, |view, cx| {
                            view.error_message = Some(format!("创建失败: {}", e));
                            cx.notify();
                        })
                        .ok();
                    }
                },
            )
            .detach();
        }
    }

    fn delete_tag(&mut self, id: i64, cx: &mut Context<Self>) {
        let store = self.store.read(cx).clone();
        cx.spawn(
            async move |this, cx| match Tag::delete(store.pool(), id).await {
                Ok(_) => {
                    this.update(cx, |view, cx| {
                        view.load_tags(cx);
                    })
                    .ok();
                }
                Err(e) => {
                    this.update(cx, |view, cx| {
                        view.error_message = Some(format!("删除失败: {}", e));
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
            self.load_tags(cx);
        }
    }

    fn prev_page(&mut self, cx: &mut Context<Self>) {
        if self.current_page > 0 {
            self.current_page -= 1;
            self.load_tags(cx);
        }
    }
}

impl Render for TagView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let total_pages = (self.total_count + self.page_size - 1) / self.page_size;

        // Initialize inputs if form is shown but inputs are None
        if self.show_form && self.name_input.is_none() {
            self.name_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("输入标签名称")
                    .default_value(&self.form_name)
            }));
            self.color_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("输入颜色代码（可选）")
                    .default_value(&self.form_color)
            }));
        }

        // Initialize search input if None
        if self.search_input.is_none() {
            self.search_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("输入标签名称...")
                    .default_value(&self.search_text)
            }));
            if let Some(ref input) = self.search_input {
                cx.subscribe_in(input, window, |this, state, event, window, cx| {
                    if let InputEvent::Change = event {
                        this.search_text = state.read(cx).value().to_string();
                        this.current_page = 0;
                        this.load_tags(cx);
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
                            .child("标签管理"),
                    )
                    .child(
                        div().flex().gap(px(8.0)).child(
                            div()
                                .id("add-tag-btn")
                                .px(px(12.0))
                                .py(px(6.0))
                                .rounded(px(4.0))
                                .bg(rgb(0x4a90d9))
                                .text_color(rgb(0xffffff))
                                .text_size(px(13.0))
                                .cursor(CursorStyle::PointingHand)
                                .child("+ 添加标签")
                                .on_mouse_down(MouseButton::Left, {
                                    let this = cx.weak_entity();
                                    move |_, window, cx| {
                                        this.update(cx, |view, cx| {
                                            view.show_add_form(window, cx);
                                        })
                                        .ok();
                                    }
                                }),
                        ),
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
                    .child(if self.tags.is_empty() {
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .h_full()
                            .text_color(rgb(0x999999))
                            .text_size(px(14.0))
                            .child("暂无数据")
                    } else {
                        let col_widths = [px(60.0), px(180.0), px(120.0), px(120.0)];
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
                                            .child("颜色"),
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
                            .children(self.tags.iter().map(|tag| {
                                let tag_id = tag.id;
                                let tag_clone = tag.clone();
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
                                            .child(format!("{}", tag.id)),
                                    )
                                    .child(
                                        div()
                                            .w(col_widths[1])
                                            .px(px(8.0))
                                            .py(px(6.0))
                                            .text_size(px(13.0))
                                            .font_weight(FontWeight::MEDIUM)
                                            .child(tag.name.clone()),
                                    )
                                    .child(
                                        div()
                                            .w(col_widths[2])
                                            .px(px(8.0))
                                            .py(px(6.0))
                                            .text_size(px(12.0))
                                            .text_color(rgb(0x666666))
                                            .child(if let Some(ref color) = tag.color {
                                                color.clone()
                                            } else {
                                                "-".to_string()
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
                                                    .id(("edit", tag_id as u64))
                                                    .px(px(8.0))
                                                    .py(px(3.0))
                                                    .rounded(px(3.0))
                                                    .bg(rgb(0x5cb85c))
                                                    .text_color(rgb(0xffffff))
                                                    .text_size(px(11.0))
                                                    .cursor(CursorStyle::PointingHand)
                                                    .child("编辑")
                                                    .on_mouse_down(MouseButton::Left, {
                                                        let this = cx.weak_entity();
                                                        move |_, window, cx| {
                                                            this.update(cx, |view, cx| {
                                                                view.show_edit_form(
                                                                    window,
                                                                    tag_clone.clone(),
                                                                    cx,
                                                                );
                                                            })
                                                            .ok();
                                                        }
                                                    }),
                                            )
                                            .child(
                                                div()
                                                    .id(("delete", tag_id as u64))
                                                    .px(px(8.0))
                                                    .py(px(3.0))
                                                    .rounded(px(3.0))
                                                    .bg(rgb(0xd9534f))
                                                    .text_color(rgb(0xffffff))
                                                    .text_size(px(11.0))
                                                    .cursor(CursorStyle::PointingHand)
                                                    .child("删除")
                                                    .on_mouse_down(MouseButton::Left, {
                                                        let this = cx.weak_entity();
                                                        move |_, _, cx| {
                                                            this.update(cx, |view, cx| {
                                                                view.confirm_delete_id =
                                                                    Some(tag_id);
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
                            .child(format!("共 {} 条记录", self.total_count)),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .id("prev-page")
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
                                        let this = cx.weak_entity();
                                        move |_, _, cx| {
                                            this.update(cx, |view, cx| {
                                                view.prev_page(cx);
                                            })
                                            .ok();
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
                                    .id("next-page")
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
                                        let this = cx.weak_entity();
                                        move |_, _, cx| {
                                            this.update(cx, |view, cx| {
                                                view.next_page(cx);
                                            })
                                            .ok();
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
                            let this = cx.weak_entity();
                            move |_, _, cx| {
                                this.update(cx, |view, cx| {
                                    view.hide_form(cx);
                                })
                                .ok();
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
                                        .child(if self.editing_tag.is_some() {
                                            "编辑标签"
                                        } else {
                                            "添加标签"
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
                                        .child(
                                            div()
                                                .text_size(px(13.0))
                                                .child("颜色 (HEX，如 #FF5733)"),
                                        )
                                        .child(
                                            Input::new(self.color_input.as_ref().unwrap())
                                                .w_full()
                                                .h(px(32.0))
                                                .px(px(8.0))
                                                .border_1()
                                                .border_color(rgb(0xcccccc))
                                                .rounded(px(4.0)),
                                        ),
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
                                                .id("cancel-form")
                                                .px(px(16.0))
                                                .py(px(8.0))
                                                .rounded(px(6.0))
                                                .bg(rgb(0x6c757d))
                                                .text_color(rgb(0xffffff))
                                                .text_size(px(13.0))
                                                .cursor(CursorStyle::PointingHand)
                                                .child("取消")
                                                .on_mouse_down(MouseButton::Left, {
                                                    let this = cx.weak_entity();
                                                    move |_, _, cx| {
                                                        this.update(cx, |view, cx| {
                                                            view.hide_form(cx);
                                                        })
                                                        .ok();
                                                    }
                                                }),
                                        )
                                        .child(
                                            div()
                                                .id("save-form")
                                                .px(px(16.0))
                                                .py(px(8.0))
                                                .rounded(px(6.0))
                                                .bg(rgb(0x4a90d9))
                                                .text_color(rgb(0xffffff))
                                                .text_size(px(13.0))
                                                .cursor(CursorStyle::PointingHand)
                                                .child("保存")
                                                .on_mouse_down(MouseButton::Left, {
                                                    let this = cx.weak_entity();
                                                    move |_, _, cx| {
                                                        this.update(cx, |view, cx| {
                                                            view.save_tag(cx);
                                                        })
                                                        .ok();
                                                    }
                                                }),
                                        ),
                                ),
                        ),
                )
            })
            .when_some(self.confirm_delete_id, |this, id| {
                let id_clone = id;
                this.child(
                    div()
                        .absolute()
                        .top(px(0.0))
                        .left(px(0.0))
                        .right(px(0.0))
                        .bottom(px(0.0))
                        .bg(rgb(0x000000))
                        .opacity(0.5)
                        .cursor(CursorStyle::PointingHand)
                        .on_mouse_down(MouseButton::Left, {
                            let this = cx.weak_entity();
                            move |_, _, cx| {
                                this.update(cx, |view, cx| {
                                    view.confirm_delete_id = None;
                                    cx.notify();
                                })
                                .ok();
                            }
                        }),
                )
                .child(
                    div()
                        .absolute()
                        .top(px(200.0))
                        .left(px(0.0))
                        .right(px(0.0))
                        .flex()
                        .justify_center()
                        .child(
                            div()
                                .w(px(300.0))
                                .bg(rgb(0xffffff))
                                .rounded(px(8.0))
                                .shadow_lg()
                                .border_1()
                                .border_color(rgb(0xdddddd))
                                .p(px(20.0))
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
                                                .text_size(px(16.0))
                                                .font_weight(FontWeight::BOLD)
                                                .child("确认删除"),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(14.0))
                                                .text_color(rgb(0x666666))
                                                .child("确定要删除这个标签吗？此操作不可恢复。"),
                                        )
                                        .child(
                                            div()
                                                .flex()
                                                .justify_end()
                                                .gap(px(8.0))
                                                .child(
                                                    div()
                                                        .id("cancel-delete")
                                                        .px(px(12.0))
                                                        .py(px(6.0))
                                                        .rounded(px(4.0))
                                                        .bg(rgb(0x6c757d))
                                                        .text_color(rgb(0xffffff))
                                                        .text_size(px(13.0))
                                                        .cursor(CursorStyle::PointingHand)
                                                        .child("取消")
                                                        .on_mouse_down(MouseButton::Left, {
                                                            let this = cx.weak_entity();
                                                            move |_, _, cx| {
                                                                this.update(cx, |view, cx| {
                                                                    view.confirm_delete_id = None;
                                                                    cx.notify();
                                                                })
                                                                .ok();
                                                            }
                                                        }),
                                                )
                                                .child(
                                                    div()
                                                        .id("confirm-delete")
                                                        .px(px(12.0))
                                                        .py(px(6.0))
                                                        .rounded(px(4.0))
                                                        .bg(rgb(0xd9534f))
                                                        .text_color(rgb(0xffffff))
                                                        .text_size(px(13.0))
                                                        .cursor(CursorStyle::PointingHand)
                                                        .child("确认删除")
                                                        .on_mouse_down(MouseButton::Left, {
                                                            let this = cx.weak_entity();
                                                            move |_, _, cx| {
                                                                this.update(cx, |view, cx| {
                                                                    if let Some(delete_id) =
                                                                        view.confirm_delete_id
                                                                    {
                                                                        view.delete_tag(
                                                                            delete_id, cx,
                                                                        );
                                                                        view.confirm_delete_id =
                                                                            None;
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
