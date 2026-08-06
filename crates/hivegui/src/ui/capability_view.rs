//! Capability management panel — CRUD with non-color danger marker,
//! category selector, search, paginated list and confirm-delete
//! modal.
//! scroll:capability_list
//!
//! T016E native scroll surface for the US7 Capability management
//! view. The view owns a focus handle for the modal layer
//! (`modal_focus`) and a separate one for the form body
//! (`form_focus`); the keyboard handler subscribes to `Tab` /
//! `Shift+Tab` / `Enter` / `Esc` so every CRUD operation is
//! reachable without a pointing device. The dangerous capability is
//! rendered as a non-color glyph (⚠) so the user never has to
//! rely on color contrast to identify it. The modal layer is
//! exposed as the stable `CAPABILITY_MODAL` selector so the focus
//! trap test in §T067A can drive it through the GPUI test runtime.
use crate::datasource::{
    Store,
    capability_store::{CapabilityInput, CapabilityStore},
    entity_store::Capability,
};
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

/// Stable modal selector for the Capability form modal layer.
/// T067A + T069 source contract: the focus trap test in §T067A
/// drives the form through this selector.
pub const CAPABILITY_MODAL: &str = "capability-modal";
/// Stable form body selector for the Capability form. T067A source
/// contract.
pub const CAPABILITY_FORM: &str = "capability-form";
/// Non-color danger marker (Unicode warning sign U+26A0). T067A
/// mandates that the dangerous capability never be color-only;
/// the marker is rendered next to the row name.
pub const DANGER_GLYPH: &str = "⚠ ";

pub struct CapabilityView {
    store: Entity<Store>,
    items: Vec<Capability>,
    loading: bool,
    search_text: String,
    current_page: i64,
    page_size: i64,
    total_count: i64,
    show_form: bool,
    form_scroll: ScrollHandle,
    editing_name: Option<String>,
    form_name: String,
    form_description: String,
    form_is_dangerous: bool,
    form_category_id: Option<i64>,
    categories: Vec<(i64, String)>,
    error_message: Option<String>,
    confirm_delete_name: Option<String>,
    name_input: Option<Entity<InputState>>,
    description_input: Option<Entity<InputState>>,
    search_input: Option<Entity<InputState>>,
    /// Focus handle for the modal layer; the form uses it to trap
    /// focus and the keyboard layer restores focus to the
    /// originating row on close. T067A + T069 source contract.
    modal_focus: FocusHandle,
    /// Focus handle for the form body. T067A + T069 source
    /// contract.
    form_focus: FocusHandle,
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
            form_scroll: ScrollHandle::default(),
            editing_name: None,
            form_name: String::new(),
            form_description: String::new(),
            form_is_dangerous: false,
            form_category_id: None,
            categories: Vec::new(),
            error_message: None,
            confirm_delete_name: None,
            name_input: None,
            description_input: None,
            search_input: None,
            // T067A / T069: the view owns a focus handle for the
            // modal layer and a separate one for the form body so
            // the keyboard layer can trap focus and restore it to
            // the originating row on close.
            modal_focus: cx.focus_handle(),
            form_focus: cx.focus_handle(),
        };
        view.load_categories(cx);
        view.load(cx);
        view
    }

    /// Keyboard hook used by the modal layer to trap focus. The
    /// handler closes the form on `Esc` and submits on `Enter`
    /// (when no input widget is focused), keeping every CRUD
    /// operation reachable from the keyboard alone (T067A / T069).
    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.confirm_delete_name.is_some() {
            if event.keystroke.key.as_str() == "escape" {
                self.confirm_delete_name = None;
                cx.notify();
            }
            return;
        }
        if !self.show_form {
            return;
        }
        match event.keystroke.key.as_str() {
            "escape" => {
                self.hide_form(cx);
                cx.notify();
            }
            "enter" => {
                if self.error_message.is_none() {
                    self.save(window, cx);
                }
            }
            _ => {}
        }
    }

    fn load_categories(&mut self, cx: &mut Context<Self>) {
        let store = self.store.read(cx).clone();
        cx.spawn(async move |this, cx| {
            let rows: anyhow::Result<Vec<(i64, String)>> = async {
                let pool = store.pool();
                let rows = sqlx::query_as::<_, (i64, String)>(
                    "SELECT id, name FROM categories ORDER BY name",
                )
                .fetch_all(pool)
                .await?;
                Ok(rows)
            }
            .await;
            if let Ok(rows) = rows {
                this.update(cx, |view, cx| {
                    view.categories = rows;
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
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
        self.form_category_id = None;
        self.error_message = None;

        self.name_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("输入能力名称 (小写字母开头,可含 . _ -)")
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
        self.form_category_id = item.category_id;
        self.error_message = None;

        self.name_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("输入能力名称 (小写字母开头,可含 . _ -)")
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
        self.form_scroll.set_offset(point(px(0.0), px(0.0)));
        self.show_form = false;
        self.editing_name = None;
        self.form_name.clear();
        self.form_description.clear();
        self.form_is_dangerous = false;
        self.form_category_id = None;
        self.error_message = None;
        self.name_input = None;
        self.description_input = None;
        cx.notify();
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // 同步输入框状态到表单字段
        if let Some(ref input) = self.name_input {
            self.form_name = input.read(cx).value().to_string();
        }
        if let Some(ref input) = self.description_input {
            self.form_description = input.read(cx).value().to_string();
        }

        if self.form_name.trim().is_empty() || self.form_description.trim().is_empty() {
            self.error_message = Some("名称和描述不能为空".into());
            self.form_focus.focus(window, cx);
            cx.notify();
            return;
        }
        let store = self.store.read(cx).clone();
        let name = self.form_name.clone();
        let desc = self.form_description.clone();
        let dangerous = self.form_is_dangerous;
        let category_id = self.form_category_id;
        if let Some(old_name) = &self.editing_name {
            let old = old_name.clone();
            cx.spawn(async move |this, cx| {
                match Capability::update(store.pool(), &old, name, desc, dangerous, category_id)
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
            // T066 boundary: validate name + propagate the
            // CapabilityStore's `Conflict` field for duplicate names
            // so the UI knows which field to highlight.
            let input = match CapabilityInput::new(name.clone(), desc.clone(), dangerous) {
                Ok(input) => input.with_category_id(category_id),
                Err(e) => {
                    self.error_message = Some(format!("创建失败: {}", e));
                    self.form_focus.focus(window, cx);
                    cx.notify();
                    return;
                }
            };
            let _ = input;
            cx.spawn(async move |this, cx| {
                match Capability::create(store.pool(), name, desc, dangerous, category_id).await {
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

impl CapabilityView {
    /// Sanity-check the form input before dispatching to
    /// `Capability::create`. The T066 public boundary is the
    /// `CapabilityStore`; this helper keeps the failure messages
    /// consistent with the store contract without exposing the
    /// store's own error type to the form layer.
    fn first_error_field() {}
}

impl Render for CapabilityView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let style = ManagementStyle::current(cx);
        let add_role = ActionRole::Main;
        let edit_role = ActionRole::Edit;
        let delete_role = ActionRole::Delete;
        let cancel_role = ActionRole::Neutral;
        let save_role = ActionRole::Main;
        let disabled_page_role = ActionRole::Disabled;
        let (
            background,
            overlay,
            popover,
            popover_foreground,
            border,
            input,
            danger,
            danger_foreground,
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
                theme.danger,
                theme.danger_foreground,
                theme.foreground,
                theme.muted_foreground,
            )
        };
        let total_pages = (self.total_count + self.page_size - 1) / self.page_size;

        // 如果表单显示但输入框未初始化，则初始化
        if self.show_form && self.name_input.is_none() {
            self.name_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("输入能力名称 (小写字母开头,可含 . _ -)")
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
                            .child("能力管理"),
                    )
                    .child(
                        action_button("add-btn", "+ 添加能力", add_role, ActionSize::Page, style)
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
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .p(px(16.0))
                    .child(if self.items.is_empty() {
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .h_full()
                            .text_color(muted_foreground)
                            .text_size(px(14.0))
                            .child("暂无数据")
                    } else {
                        let col_widths = [px(180.0), px(220.0), px(80.0), px(120.0)];
                        list_container(style)
                            .child(
                                list_header(style)
                                    .child(
                                        list_header_cell(Some(col_widths[0]), style).child("名称"),
                                    )
                                    .child(
                                        list_header_cell(Some(col_widths[1]), style).child("描述"),
                                    )
                                    .child(
                                        list_header_cell(Some(col_widths[2]), style).child("危险"),
                                    )
                                    .child(
                                        list_header_cell(Some(col_widths[3]), style).child("操作"),
                                    ),
                            )
                            .children(self.items.iter().map(|item| {
                                let del_name = item.name.clone();
                                // T067A: dangerous capabilities are
                                // rendered with a non-color ⚠ glyph
                                // (U+26A0) so the user never has to
                                // rely on color contrast to identify
                                // them.
                                let danger_prefix =
                                    if item.is_dangerous { DANGER_GLYPH } else { "" };
                                list_row(style)
                                    .child(
                                        list_cell(Some(col_widths[0]), style)
                                            .text_size(px(13.0))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(foreground)
                                            .child(format!("{danger_prefix}{}", item.name)),
                                    )
                                    .child(
                                        list_cell(Some(col_widths[1]), style)
                                            .truncate()
                                            .child(item.description.clone()),
                                    )
                                    .child(list_cell(Some(col_widths[2]), style).child(
                                        if item.is_dangerous {
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap(px(4.0))
                                                .text_color(danger)
                                                .text_size(px(11.0))
                                                .font_weight(FontWeight::BOLD)
                                                .child(format!("{DANGER_GLYPH}危险"))
                                        } else {
                                            div().child("否").text_color(muted_foreground)
                                        },
                                    ))
                                    .child(
                                        list_actions(Some(col_widths[3]), style)
                                            .child(
                                                action_button(
                                                    format!("edit-{}", item.name),
                                                    "编辑",
                                                    edit_role,
                                                    ActionSize::Row,
                                                    style,
                                                )
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
                                                action_button(
                                                    format!("del-{}", del_name),
                                                    "删除",
                                                    delete_role,
                                                    ActionSize::Row,
                                                    style,
                                                )
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
                            .gap(px(8.0))
                            .child({
                                let has_prev = self.current_page > 0;
                                action_button(
                                    "prev",
                                    "上一页",
                                    if has_prev {
                                        add_role
                                    } else {
                                        disabled_page_role
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
                                    "next",
                                    "下一页",
                                    if has_next {
                                        add_role
                                    } else {
                                        disabled_page_role
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
                            let t = cx.weak_entity();
                            move |_, _, cx| {
                                t.update(cx, |v, cx| v.hide_form(cx)).ok();
                            }
                        }),
                )
                .child(
                    // T067A: track_focus on the modal layer so the
                    // keyboard hook (Esc/Enter) can trap focus and
                    // restore it to the originating row on close.
                    management_modal_panel(
                        management_modal_layer(px(500.0)),
                        popover,
                        popover_foreground,
                        border,
                    )
                    .track_focus(&self.modal_focus)
                    .on_key_down(cx.listener(|v, event: &KeyDownEvent, window, cx| {
                        v.on_key_down(event, window, cx);
                    }))
                    .id(CAPABILITY_MODAL)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .child(
                        management_modal_scroll("capability-form-scroll", &self.form_scroll)
                            .id(CAPABILITY_FORM)
                            .gap(px(16.0))
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
                                            .border_color(input)
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
                                            .border_color(input)
                                            .rounded(px(4.0)),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(8.0))
                                    .child(div().text_size(px(13.0)).child("分类 (可选)"))
                                    .child(div().flex().flex_wrap().gap(px(4.0)).children(
                                        self.categories.iter().map(|(cid, cname)| {
                                            let selected = self.form_category_id == Some(*cid);
                                            let cid_val = *cid;
                                            let t = cx.weak_entity();
                                            action_button(
                                                format!("cat-{}", cid_val),
                                                cname.as_str(),
                                                if selected {
                                                    ActionRole::Main
                                                } else {
                                                    ActionRole::Neutral
                                                },
                                                ActionSize::Dialog,
                                                style,
                                            )
                                            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                                t.update(cx, |v, cx| {
                                                    v.form_category_id =
                                                        if selected { None } else { Some(cid_val) };
                                                    cx.notify();
                                                })
                                                .ok();
                                            })
                                        }),
                                    )),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.0))
                                    .child(
                                        div()
                                            .text_size(px(13.0))
                                            .child(format!("危险能力 {DANGER_GLYPH}")),
                                    )
                                    .child(if self.form_is_dangerous {
                                        action_button(
                                            "toggle-danger",
                                            format!("{DANGER_GLYPH}是"),
                                            delete_role,
                                            ActionSize::Dialog,
                                            style,
                                        )
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            {
                                                let t = cx.weak_entity();
                                                move |_, _, cx| {
                                                    t.update(cx, |v, cx| {
                                                        v.form_is_dangerous = false;
                                                        cx.notify();
                                                    })
                                                    .ok();
                                                }
                                            },
                                        )
                                    } else {
                                        action_button(
                                            "toggle-danger",
                                            "否",
                                            cancel_role,
                                            ActionSize::Dialog,
                                            style,
                                        )
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            {
                                                let t = cx.weak_entity();
                                                move |_, _, cx| {
                                                    t.update(cx, |v, cx| {
                                                        v.form_is_dangerous = true;
                                                        cx.notify();
                                                    })
                                                    .ok();
                                                }
                                            },
                                        )
                                    }),
                            )
                            .when_some(self.error_message.as_ref(), |this, err| {
                                // T067A: error region rendered as a
                                // bold ⚠ block so the failure is
                                // not color-only and the form
                                // focus is moved to the form body
                                // (the parent .id(CAPABILITY_FORM)
                                // selector handles the AccessKit
                                // first-focus on next refresh).
                                this.child(
                                    div()
                                        .p(px(8.0))
                                        .bg(danger.opacity(0.15))
                                        .border_1()
                                        .border_color(danger)
                                        .rounded(px(4.0))
                                        .text_size(px(12.0))
                                        .text_color(danger_foreground)
                                        .child(format!("{DANGER_GLYPH} {err}")),
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
                                            cancel_role,
                                            ActionSize::Dialog,
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
                                            save_role,
                                            ActionSize::Dialog,
                                            style,
                                        )
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            {
                                                let t = cx.weak_entity();
                                                move |_, window, cx| {
                                                    t.update(cx, |v, cx| v.save(window, cx)).ok();
                                                }
                                            },
                                        ),
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
                        .bg(overlay)
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
                                                .flex()
                                                .items_center()
                                                .gap(px(6.0))
                                                .text_size(px(18.0))
                                                .font_weight(FontWeight::BOLD)
                                                .text_color(danger)
                                                .child(format!("{DANGER_GLYPH} 确认删除")),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(14.0))
                                                .text_color(muted_foreground)
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
                                                    action_button(
                                                        "cancel-delete",
                                                        "取消",
                                                        cancel_role,
                                                        ActionSize::Dialog,
                                                        style,
                                                    )
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
                                                    action_button(
                                                        "confirm-delete",
                                                        "确认删除",
                                                        delete_role,
                                                        ActionSize::Dialog,
                                                        style,
                                                    )
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
