use crate::datasource::{Store, entity_store::Category};
use crate::ui::management_style::{
    ActionRole, ActionSize, ManagementStyle, action_button, list_actions, list_cell,
    list_container, list_header, list_header_cell, list_row,
};
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::ActiveTheme as _;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::scroll::ScrollableElement;

pub struct CategoryView {
    store: Entity<Store>,
    categories: Vec<Category>,
    loading: bool,
    search_text: String,
    current_page: i64,
    page_size: i64,
    total_count: i64,
    show_form: bool,
    editing_category: Option<Category>,
    form_name: String,
    form_slug: String,
    form_description: String,
    form_parent_id: Option<i64>,
    error_message: Option<String>,
    confirm_delete_id: Option<i64>,
    name_input: Option<Entity<InputState>>,
    slug_input: Option<Entity<InputState>>,
    description_input: Option<Entity<InputState>>,
    search_input: Option<Entity<InputState>>,
}

impl CategoryView {
    pub fn new(store: Entity<Store>, cx: &mut Context<Self>) -> Self {
        let mut view = Self {
            store: store.clone(),
            categories: Vec::new(),
            loading: false,
            search_text: String::new(),
            current_page: 0,
            page_size: 20,
            total_count: 0,
            show_form: false,
            editing_category: None,
            form_name: String::new(),
            form_slug: String::new(),
            form_description: String::new(),
            form_parent_id: None,
            error_message: None,
            confirm_delete_id: None,
            name_input: None,
            slug_input: None,
            description_input: None,
            search_input: None,
        };
        view.load_categories(cx);
        view
    }

    fn load_categories(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        let store = self.store.read(cx).clone();
        let search = if self.search_text.is_empty() {
            None
        } else {
            Some(self.search_text.clone())
        };
        let offset = self.current_page * self.page_size;

        cx.spawn(async move |this, cx| {
            let categories = Category::list(store.pool(), search.clone(), 20, offset).await?;
            let count = Category::count(store.pool(), search).await?;

            this.update(cx, |view, cx| {
                view.categories = categories;
                view.total_count = count;
                view.loading = false;
                cx.notify();
            })
        })
        .detach();
    }

    fn next_page(&mut self, cx: &mut Context<Self>) {
        if (self.current_page + 1) * self.page_size < self.total_count {
            self.current_page += 1;
            self.load_categories(cx);
        }
    }
    fn prev_page(&mut self, cx: &mut Context<Self>) {
        if self.current_page > 0 {
            self.current_page -= 1;
            self.load_categories(cx);
        }
    }

    fn show_add_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_form = true;
        self.editing_category = None;
        self.form_name = String::new();
        self.form_slug = String::new();
        self.form_description = String::new();
        self.form_parent_id = None;
        self.error_message = None;

        self.name_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("输入分类名称")
                .default_value("")
        }));
        self.slug_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("输入 slug")
                .default_value("")
        }));
        self.description_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("输入描述（可选）")
                .default_value("")
        }));

        cx.notify();
    }

    fn show_edit_form(&mut self, window: &mut Window, category: Category, cx: &mut Context<Self>) {
        self.show_form = true;
        self.editing_category = Some(category.clone());
        self.form_name = category.name.clone();
        self.form_slug = category.slug.clone();
        self.form_description = category.description.clone().unwrap_or_default();
        self.form_parent_id = category.parent_id;
        self.error_message = None;

        self.name_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("输入分类名称")
                .default_value(&category.name)
        }));
        self.slug_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("输入 slug")
                .default_value(&category.slug)
        }));
        self.description_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("输入描述（可选）")
                .default_value(&category.description.unwrap_or_default())
        }));

        cx.notify();
    }

    fn hide_form(&mut self, cx: &mut Context<Self>) {
        self.show_form = false;
        self.editing_category = None;
        self.form_name = String::new();
        self.form_slug = String::new();
        self.form_description = String::new();
        self.form_parent_id = None;
        self.error_message = None;
        self.name_input = None;
        self.slug_input = None;
        self.description_input = None;
        cx.notify();
    }

    fn render_table(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let style = ManagementStyle::current(cx);
        let col_widths = [px(60.0), px(150.0), px(120.0), px(200.0), px(120.0)];

        list_container(style)
            // Header row
            .child(
                list_header(style)
                    .child(list_header_cell(Some(col_widths[0]), style).child("ID"))
                    .child(list_header_cell(Some(col_widths[1]), style).child("名称"))
                    .child(list_header_cell(Some(col_widths[2]), style).child("Slug"))
                    .child(list_header_cell(Some(col_widths[3]), style).child("描述"))
                    .child(list_header_cell(Some(col_widths[4]), style).child("操作")),
            )
            // Data rows
            .children(self.categories.iter().map(|category| {
                let category_id = category.id;
                let category_clone = category.clone();
                list_row(style)
                    .child(list_cell(Some(col_widths[0]), style).child(format!("{}", category.id)))
                    .child(
                        list_cell(Some(col_widths[1]), style)
                            .text_size(px(13.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(style.list.foreground)
                            .child(category.name.clone()),
                    )
                    .child(list_cell(Some(col_widths[2]), style).child(category.slug.clone()))
                    .child(
                        list_cell(Some(col_widths[3]), style)
                            .truncate()
                            .child(category.description.clone().unwrap_or_default()),
                    )
                    .child(
                        list_actions(Some(col_widths[4]), style)
                            .child(
                                action_button(
                                    ("edit", category_id as u64),
                                    "编辑",
                                    ActionRole::Edit,
                                    ActionSize::Row,
                                    style,
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    {
                                        let this = cx.weak_entity();
                                        move |_, window, cx| {
                                            this.update(cx, |view, cx| {
                                                view.show_edit_form(
                                                    window,
                                                    category_clone.clone(),
                                                    cx,
                                                );
                                            })
                                            .ok();
                                        }
                                    },
                                ),
                            )
                            .child(
                                action_button(
                                    ("delete", category_id as u64),
                                    "删除",
                                    ActionRole::Delete,
                                    ActionSize::Row,
                                    style,
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    {
                                        let this = cx.weak_entity();
                                        move |_, _, cx| {
                                            this.update(cx, |view, cx| {
                                                view.confirm_delete_id = Some(category_id);
                                                cx.notify();
                                            })
                                            .ok();
                                        }
                                    },
                                ),
                            ),
                    )
            }))
    }

    fn save_category(&mut self, cx: &mut Context<Self>) {
        // Sync input state to form fields
        if let Some(ref inp) = self.name_input {
            self.form_name = inp.read(cx).value().to_string();
        }
        if let Some(ref inp) = self.slug_input {
            self.form_slug = inp.read(cx).value().to_string();
        }
        if let Some(ref inp) = self.description_input {
            self.form_description = inp.read(cx).value().to_string();
        }

        if self.form_name.trim().is_empty() || self.form_slug.trim().is_empty() {
            self.error_message = Some("名称和 slug 不能为空".to_string());
            cx.notify();
            return;
        }

        let store = self.store.read(cx).clone();
        let name = self.form_name.clone();
        let slug = self.form_slug.clone();
        let description = if self.form_description.is_empty() {
            None
        } else {
            Some(self.form_description.clone())
        };
        let parent_id = self.form_parent_id;

        if let Some(editing) = &self.editing_category {
            let id = editing.id;
            cx.spawn(async move |this, cx| {
                match Category::update(store.pool(), id, parent_id, name, slug, description).await {
                    Ok(_) => {
                        this.update(cx, |view, cx| {
                            view.hide_form(cx);
                            view.load_categories(cx);
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
            cx.spawn(async move |this, cx| {
                match Category::create(store.pool(), parent_id, name, slug, description).await {
                    Ok(_) => {
                        this.update(cx, |view, cx| {
                            view.hide_form(cx);
                            view.load_categories(cx);
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
                }
            })
            .detach();
        }
    }

    fn delete_category(&mut self, id: i64, cx: &mut Context<Self>) {
        let store = self.store.read(cx).clone();
        cx.spawn(
            async move |this, cx| match Category::delete(store.pool(), id).await {
                Ok(_) => {
                    this.update(cx, |view, cx| {
                        view.load_categories(cx);
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
}

impl Render for CategoryView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let style = ManagementStyle::current(cx);
        let (
            background,
            overlay,
            popover,
            popover_foreground,
            border,
            input,
            warning,
            warning_foreground,
            foreground,
            muted_foreground,
        ) = {
            let theme = cx.theme();
            (
                theme.background,
                theme.overlay,
                theme.popover,
                theme.popover_foreground,
                theme.border,
                theme.input,
                theme.warning,
                theme.warning_foreground,
                theme.foreground,
                theme.muted_foreground,
            )
        };
        let total_pages = (self.total_count + self.page_size - 1) / self.page_size;

        // Initialize inputs if form is shown but inputs are None
        if self.show_form && self.name_input.is_none() {
            self.name_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("输入分类名称")
                    .default_value(&self.form_name)
            }));
            self.slug_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("输入 slug")
                    .default_value(&self.form_slug)
            }));
            self.description_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("输入描述（可选）")
                    .default_value(&self.form_description)
            }));
        }

        // Initialize search input
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
                        this.load_categories(cx);
                    }
                })
                .detach();
            }
        }

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(background)
            .text_color(foreground)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .p(px(16.0))
                    .border_b_1()
                    .border_color(border)
                    .child(
                        div()
                            .text_size(px(18.0))
                            .font_weight(FontWeight::BOLD)
                            .child("分类管理"),
                    )
                    .child(
                        action_button(
                            "add-category-btn",
                            "+ 添加分类",
                            ActionRole::Main,
                            ActionSize::Page,
                            style,
                        )
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
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .p(px(12.0))
                    .border_b_1()
                    .border_color(border)
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
                    .child(if self.categories.is_empty() {
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .h_full()
                            .text_color(muted_foreground)
                            .text_size(px(14.0))
                            .child("暂无数据")
                            .into_any_element()
                    } else {
                        self.render_table(cx).into_any_element()
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
                    .border_color(border)
                    .child(
                        div()
                            .text_size(px(13.0))
                            .text_color(muted_foreground)
                            .child(format!("共 {} 条", self.total_count)),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child({
                                let has_prev = self.current_page > 0;
                                action_button(
                                    "prev-page",
                                    "上一页",
                                    if has_prev {
                                        ActionRole::Main
                                    } else {
                                        ActionRole::Disabled
                                    },
                                    ActionSize::Compact,
                                    style,
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    {
                                        let this = cx.weak_entity();
                                        move |_, _, cx| {
                                            this.update(cx, |view, cx| {
                                                view.prev_page(cx);
                                            })
                                            .ok();
                                        }
                                    },
                                )
                            })
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .text_color(muted_foreground)
                                    .child(format!(
                                        "第 {} / {} 页",
                                        self.current_page + 1,
                                        total_pages.max(1)
                                    )),
                            )
                            .child({
                                let has_next =
                                    (self.current_page + 1) * self.page_size < self.total_count;
                                action_button(
                                    "next-page",
                                    "下一页",
                                    if has_next {
                                        ActionRole::Main
                                    } else {
                                        ActionRole::Disabled
                                    },
                                    ActionSize::Compact,
                                    style,
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    {
                                        let this = cx.weak_entity();
                                        move |_, _, cx| {
                                            this.update(cx, |view, cx| {
                                                view.next_page(cx);
                                            })
                                            .ok();
                                        }
                                    },
                                )
                            }),
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
                        .bg(overlay)
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
                        .bg(popover)
                        .text_color(popover_foreground)
                        .rounded(px(12.0))
                        .shadow_lg()
                        .border_1()
                        .border_color(border)
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
                                        .child(if self.editing_category.is_some() {
                                            "编辑分类"
                                        } else {
                                            "添加分类"
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
                                                .border_color(input)
                                                .rounded(px(4.0)),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(px(8.0))
                                        .child(div().text_size(px(13.0)).child("Slug *"))
                                        .child(
                                            Input::new(self.slug_input.as_ref().unwrap())
                                                .w_full()
                                                .h(px(32.0))
                                                .px(px(8.0))
                                                .border_1()
                                                .border_color(input)
                                                .rounded(px(4.0)),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(px(8.0))
                                        .child(div().text_size(px(13.0)).child("描述"))
                                        .child(
                                            Input::new(self.description_input.as_ref().unwrap())
                                                .w_full()
                                                .h(px(60.0))
                                                .px(px(8.0))
                                                .py(px(4.0))
                                                .border_1()
                                                .border_color(input)
                                                .rounded(px(4.0)),
                                        ),
                                )
                                .when_some(self.error_message.as_ref(), |this, err| {
                                    this.child(
                                        div()
                                            .p(px(8.0))
                                            .bg(warning)
                                            .border_1()
                                            .border_color(warning)
                                            .rounded(px(4.0))
                                            .text_size(px(12.0))
                                            .text_color(warning_foreground)
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
                                                "cancel-form",
                                                "取消",
                                                ActionRole::Neutral,
                                                ActionSize::Dialog,
                                                style,
                                            )
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
                                            action_button(
                                                "save-form",
                                                "保存",
                                                ActionRole::Main,
                                                ActionSize::Dialog,
                                                style,
                                            )
                                            .on_mouse_down(MouseButton::Left, {
                                                let this = cx.weak_entity();
                                                move |_, _, cx| {
                                                    this.update(cx, |view, cx| {
                                                        view.save_category(cx);
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
                this.child(
                    div()
                        .absolute()
                        .top(px(0.0))
                        .left(px(0.0))
                        .right(px(0.0))
                        .bottom(px(0.0))
                        .bg(overlay)
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
                                .bg(popover)
                                .text_color(popover_foreground)
                                .rounded(px(8.0))
                                .shadow_lg()
                                .border_1()
                                .border_color(border)
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
                                                .text_color(foreground)
                                                .child("确认删除"),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(14.0))
                                                .text_color(muted_foreground)
                                                .child("确定要删除这个分类吗？此操作不可恢复。"),
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
                                                        ActionSize::Dialog,
                                                        style,
                                                    )
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
                                                    action_button(
                                                        "confirm-delete",
                                                        "确认删除",
                                                        ActionRole::Delete,
                                                        ActionSize::Dialog,
                                                        style,
                                                    )
                                                    .on_mouse_down(MouseButton::Left, {
                                                        let this = cx.weak_entity();
                                                        move |_, _, cx| {
                                                            this.update(cx, |view, cx| {
                                                                if let Some(delete_id) =
                                                                    view.confirm_delete_id
                                                                {
                                                                    view.delete_category(
                                                                        delete_id, cx,
                                                                    );
                                                                    view.confirm_delete_id = None;
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
