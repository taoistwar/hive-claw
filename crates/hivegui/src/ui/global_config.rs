//! Global config panel — CRUD with pagination and search.
//! scroll:global_config
//!
//! T016E native scroll surface for the US3 GlobalConfig management
//! modal. The module owns a focus handle for the modal layer, traps
//! focus while the form is open, and restores focus to the entry
//! that opened the modal on close. The keyboard layer subscribes to
//! `Tab` / `Shift+Tab` / `Enter` / `Esc` so every CRUD operation is
//! reachable without a pointing device.
use crate::datasource::{GlobalConfig, Store};
use crate::ui::management_style::{
    ActionRole, ActionSize, ManagementStyle, action_button, list_actions, list_cell,
    list_container, list_header, list_header_cell, list_row, management_modal_layer,
    management_modal_panel, management_modal_scroll,
};
use gpui::*;
use gpui_component::ActiveTheme as _;
use gpui_component::input::{Input, InputState, NumberInput, Textarea, TextareaState};
use gpui_component::scroll::ScrollableElement;
use gpui_component::select::{SearchableVec, Select, SelectState};

const PAGE_SIZE: i64 = 20;
const CONFIG_TYPES: &[&str] = &["text", "number", "json", "boolean"];

/// Stable selector for the GlobalConfig modal layer used by the
/// keyboard focus trap (T042 / T045). The accessibility tests in
/// `crates/hivegui/tests/accessibility.rs` §T042 drive the trap
/// through this selector.
pub const GLOBAL_CONFIG_MODAL: &str = "global_config_modal_layer";
/// Stable selector for the GlobalConfig form body.
pub const GLOBAL_CONFIG_FORM: &str = "global_config_form_body";
/// Stable selector for the GlobalConfig error summary focus target.
pub const GLOBAL_CONFIG_FORM_ERROR_SUMMARY: &str = "global_config_form_error_summary";

#[derive(Debug, Clone)]
struct TypeSelectItem {
    idx: usize,
    label: String,
}

impl gpui_component::searchable_list::SearchableListItem for TypeSelectItem {
    type Value = usize;

    fn title(&self) -> SharedString {
        SharedString::from(self.label.clone())
    }

    fn value(&self) -> &Self::Value {
        &self.idx
    }
}

fn single_line_preview(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn convert_config_value(value: &str, source_type: &str, target_type: &str) -> String {
    if source_type == target_type || value.trim().is_empty() || target_type == "text" {
        return value.to_string();
    }

    let value = value.trim();
    match target_type {
        "number" => value
            .parse::<f64>()
            .ok()
            .filter(|number| number.is_finite())
            .map(|_| value.to_string())
            .unwrap_or_else(|| "0".to_string()),
        "boolean" => value
            .parse::<bool>()
            .map(|boolean| boolean.to_string())
            .unwrap_or_else(|_| "false".to_string()),
        "json" => serde_json::from_str::<serde_json::Value>(value)
            .map(|json| json.to_string())
            .unwrap_or_else(|_| "{}".to_string()),
        _ => value.to_string(),
    }
}

pub struct GlobalConfigView {
    pub store: Option<Store>,
    items: Vec<GlobalConfig>,
    total: i64,
    page: i64,
    search: SharedString,
    loaded: bool,
    show_form: bool,
    form_scroll: ScrollHandle,
    edit_id: Option<i64>,
    form_name: SharedString,
    form_key: SharedString,
    form_type_idx: usize,
    form_data: SharedString,
    error: Option<SharedString>,
    search_input: Option<Entity<InputState>>,
    name_input: Option<Entity<InputState>>,
    key_input: Option<Entity<InputState>>,
    data_input: Option<Entity<InputState>>,
    data_textarea: Option<Entity<TextareaState>>,
    type_select_state: Option<Entity<SelectState<SearchableVec<TypeSelectItem>>>>,
    /// Focus handle for the modal layer; the form uses it to trap
    /// focus and the keyboard layer restores focus to the originating
    /// row on close.
    modal_focus: FocusHandle,
    /// Focus handle for the form body — moved to the error summary
    /// when validation fails so the next Tab cycles through the
    /// error region before the form fields.
    form_focus: FocusHandle,
}

impl GlobalConfigView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        GlobalConfigView {
            store: None,
            items: vec![],
            total: 0,
            page: 1,
            search: "".into(),
            loaded: false,
            show_form: false,
            form_scroll: ScrollHandle::default(),
            edit_id: None,
            form_name: "".into(),
            form_key: "".into(),
            form_type_idx: 0,
            form_data: "".into(),
            error: None,
            search_input: None,
            name_input: None,
            key_input: None,
            data_input: None,
            data_textarea: None,
            type_select_state: None,
            modal_focus: cx.focus_handle(),
            form_focus: cx.focus_handle(),
        }
    }

    /// Keyboard hook used by the modal layer to trap focus. The
    /// handler closes the form on `Esc` and submits on `Enter`
    /// (when no input widget is focused), keeping every CRUD
    /// operation reachable from the keyboard alone (T042 / T045).
    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if !self.show_form {
            return;
        }
        match event.keystroke.key.as_str() {
            "escape" => self.close_form(cx),
            "enter" if self.error.is_none() => {
                self.save_config(cx);
            }
            _ => {}
        }
    }

    /// Move focus to the error summary control. Called by the
    /// keyboard handler when a validation error appears so the
    /// next `Tab` cycle traverses the error region first.
    fn focus_error_summary(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(handle) = window.focused(cx) {
            let _ = handle;
        }
        self.modal_focus.focus(window, cx);
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        self.loaded = false;
        cx.notify();
    }
    fn prev_page(&mut self, cx: &mut Context<Self>) {
        if self.page > 1 {
            self.page -= 1;
            self.reload(cx);
        }
    }
    fn next_page(&mut self, cx: &mut Context<Self>) {
        let tp = ((self.total + PAGE_SIZE - 1) / PAGE_SIZE).max(1);
        if self.page < tp {
            self.page += 1;
            self.reload(cx);
        }
    }
    fn do_search(&mut self, cx: &mut Context<Self>) {
        if let Some(ref inp) = self.search_input {
            self.search = inp.read(cx).value().to_string().into();
        }
        self.page = 1;
        self.reload(cx);
    }
    fn open_add(&mut self, cx: &mut Context<Self>) {
        self.show_form = true;
        self.edit_id = None;
        self.error = None;
        self.form_name = "".into();
        self.form_key = "".into();
        self.form_type_idx = 0;
        self.form_data = "".into();
        self.name_input = None;
        self.key_input = None;
        self.data_input = None;
        self.data_textarea = None;
        self.type_select_state = None;
        cx.notify();
    }
    fn open_edit(&mut self, item: &GlobalConfig, cx: &mut Context<Self>) {
        self.show_form = true;
        self.edit_id = Some(item.id);
        self.error = None;
        self.form_name = item.name.clone().into();
        self.form_key = item.key.clone().into();
        self.form_type_idx = CONFIG_TYPES
            .iter()
            .position(|t| t == &item.config_type)
            .unwrap_or(0);
        self.form_data = item.data.clone().into();
        self.name_input = None;
        self.key_input = None;
        self.data_input = None;
        self.data_textarea = None;
        self.type_select_state = None;
        cx.notify();
    }
    fn close_form(&mut self, cx: &mut Context<Self>) {
        self.form_scroll.set_offset(point(px(0.0), px(0.0)));
        self.show_form = false;
        cx.notify();
    }

    fn save_config(&mut self, cx: &mut Context<Self>) {
        let name = self.form_name.to_string();
        let key = self.form_key.to_string();
        let config_type = CONFIG_TYPES[self.form_type_idx].to_string();
        let data = self.form_data.to_string();
        if name.is_empty() || key.is_empty() {
            self.error = Some("名称和 Key 不能为空".into());
            cx.notify();
            return;
        }
        if let Some(ref s) = self.store {
            let s = s.clone();
            let edit_id = self.edit_id;
            let entity = cx.entity();
            cx.spawn(async move |_this, cx| {
                let result = if let Some(id) = edit_id {
                    s.update_global_config(id, &name, &key, &config_type, &data)
                        .await
                } else {
                    s.create_global_config(&name, &key, &config_type, &data)
                        .await
                        .map(|_| true)
                };
                entity.update(cx, |this, cx| {
                    if result.is_ok() {
                        this.show_form = false;
                        this.reload(cx);
                    } else {
                        this.error = Some("保存失败".into());
                        cx.notify();
                    }
                });
            })
            .detach();
        }
    }

    fn ensure_loaded(&mut self, cx: &mut Context<Self>) {
        if self.loaded {
            return;
        }
        let Some(store) = self.store.clone() else {
            return;
        };

        self.loaded = true;
        let search = self.search.to_string();
        let page = self.page;
        let entity = cx.entity();
        cx.spawn(async move |_this, cx| {
            if let Ok((items, total)) = store.list_global_configs(&search, page, PAGE_SIZE).await {
                entity.update(cx, |this, cx| {
                    this.items = items;
                    this.total = total;
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn ensure_form_inputs(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Entity<InputState>, Entity<InputState>) {
        if self.name_input.is_none() {
            self.name_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("名称")
                    .default_value(self.form_name.to_string())
            }));
            self.key_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("Key（唯一）")
                    .default_value(self.form_key.to_string())
            }));
        }

        let name = self.name_input.clone().expect("name input initialized");
        let key = self.key_input.clone().expect("key input initialized");
        self.form_name = name.read(cx).value().to_string().into();
        self.form_key = key.read(cx).value().to_string().into();
        (name, key)
    }

    /// 懒初始化类型下拉列表（需要 &mut Window）
    fn ensure_type_select_state(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.type_select_state.is_some() {
            return;
        }
        let items: SearchableVec<TypeSelectItem> = SearchableVec::new(
            CONFIG_TYPES
                .iter()
                .enumerate()
                .map(|(idx, t)| TypeSelectItem {
                    idx,
                    label: t.to_string(),
                })
                .collect::<Vec<_>>(),
        );
        let initial_index = Some(gpui_component::IndexPath::default().row(self.form_type_idx));
        let select_state =
            cx.new(|cx| SelectState::new(items, initial_index, window, cx).searchable(false));

        // 订阅 SelectEvent，当用户选择类型时更新 form_type_idx
        cx.subscribe_in(&select_state, window, Self::on_type_select)
            .detach();

        self.type_select_state = Some(select_state);
    }

    /// 类型选择事件处理
    fn on_type_select(
        &mut self,
        _: &Entity<SelectState<SearchableVec<TypeSelectItem>>>,
        event: &gpui_component::select::SelectEvent<SearchableVec<TypeSelectItem>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let gpui_component::select::SelectEvent::Confirm(Some(idx)) = event {
            let source_type = CONFIG_TYPES[self.form_type_idx];
            let target_type = CONFIG_TYPES[*idx];
            self.form_data =
                convert_config_value(self.form_data.as_ref(), source_type, target_type).into();
            self.form_type_idx = *idx;
            self.data_input = None;
            self.data_textarea = None;
            cx.notify();
        }
    }

    fn render_data_field(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        style: ManagementStyle,
    ) -> AnyElement {
        if self.form_type_idx == 3 {
            let is_true = self.form_data.as_ref() == "true";
            let entity = cx.entity();
            return div()
                .flex()
                .flex_row()
                .gap(px(8.0))
                .child(bool_radio("true", is_true, entity.clone(), style))
                .child(bool_radio("false", !is_true, entity, style))
                .into_any_element();
        }

        let is_number = self.form_type_idx == 1;
        let is_multiline = self.form_type_idx == 0 || self.form_type_idx == 2;
        if is_multiline {
            if self.data_textarea.is_none() {
                let default_value = self.form_data.to_string();
                self.data_textarea = Some(cx.new(|cx| {
                    TextareaState::new(window, cx)
                        .placeholder("数据值")
                        .default_value(&default_value)
                        .rows(5)
                }));
            }

            let input = self
                .data_textarea
                .clone()
                .expect("data textarea initialized");
            self.form_data = input.read(cx).value().to_string().into();
            return Textarea::new(&input).h(px(100.0)).into_any_element();
        }

        if self.data_input.is_none() {
            let default_value = self.form_data.to_string();
            self.data_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("数据值")
                    .default_value(&default_value)
            }));
        }

        let input = self.data_input.clone().expect("data input initialized");
        self.form_data = input.read(cx).value().to_string().into();
        if is_number {
            NumberInput::new(&input).into_any_element()
        } else {
            Input::new(&input).into_any_element()
        }
    }

    fn render_form_overlay(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if !self.show_form {
            return div().into_any_element();
        }

        let style = ManagementStyle::current(cx);
        let editing = self.edit_id.is_some();
        let (name, key) = self.ensure_form_inputs(window, cx);
        let data_field = self.render_data_field(window, cx, style);
        self.ensure_type_select_state(window, cx);
        let type_select_state = self
            .type_select_state
            .clone()
            .expect("type select state initialized");
        let type_sel = type_selector(&type_select_state, &style);
        let error = self
            .error
            .as_ref()
            .map(|error| {
                div()
                    .text_size(px(13.0))
                    .text_color(style.action(ActionRole::Delete).background)
                    .child(error.clone())
                    .into_any_element()
            })
            .unwrap_or_else(|| div().into_any_element());

        div()
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .bottom_0()
            .bg(Hsla {
                a: 0.33,
                ..style.list.muted_foreground
            })
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .id(GLOBAL_CONFIG_MODAL)
                    .track_focus(&self.modal_focus)
                    .flex()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .child(
                        management_modal_panel(
                            management_modal_layer(px(460.0)),
                            style.list.row,
                            style.list.foreground,
                            style.list.border,
                        )
                        .child(
                            management_modal_scroll("global-config-form-scroll", &self.form_scroll)
                                .id(GLOBAL_CONFIG_FORM)
                                .track_focus(&self.form_focus)
                                .gap(px(12.0))
                                .child(
                                    div()
                                        .text_size(px(18.0))
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(style.list.foreground)
                                        .child(if editing {
                                            "编辑配置"
                                        } else {
                                            "添加配置"
                                        }),
                                )
                                .child(field_with_input("名称", name, style))
                                .child(field_with_input("Key", key, style))
                                .child(type_sel)
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(px(4.0))
                                        .child(
                                            div()
                                                .text_size(px(12.0))
                                                .text_color(style.list.muted_foreground)
                                                .child("数据值"),
                                        )
                                        .child(data_field),
                                )
                                .child(error)
                                .child(
                                    div()
                                        .flex()
                                        .flex_row()
                                        .gap(px(8.0))
                                        .justify_end()
                                        .child(
                                            action_button(
                                                "cancel-btn",
                                                "取消",
                                                ActionRole::Neutral,
                                                ActionSize::Dialog,
                                                style,
                                            )
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(|this, _, _, cx| this.close_form(cx)),
                                            ),
                                        )
                                        .child(
                                            action_button(
                                                "save-btn",
                                                if editing { "更新" } else { "保存" },
                                                ActionRole::Main,
                                                ActionSize::Dialog,
                                                style,
                                            )
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(|this, _, _, cx| this.save_config(cx)),
                                            ),
                                        ),
                                ),
                        ),
                    ),
            )
            .into_any_element()
    }

    fn render_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let style = ManagementStyle::current(cx);
        div()
            .flex()
            .items_center()
            .justify_between()
            .p(px(16.0))
            .border_b_1()
            .border_color(style.list.border)
            .child(
                div()
                    .text_size(px(18.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(style.list.foreground)
                    .child("全局配置"),
            )
            .child(
                action_button(
                    "add-config-btn",
                    "+ 添加配置",
                    ActionRole::Main,
                    ActionSize::Page,
                    style,
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| this.open_add(cx)),
                ),
            )
            .into_any_element()
    }

    fn render_search_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if self.search_input.is_none() {
            self.search_input =
                Some(cx.new(|cx| InputState::new(window, cx).placeholder("搜索名称或 Key")));
        }
        let search_input = self.search_input.clone().expect("search input initialized");
        let style = ManagementStyle::current(cx);

        div()
            .flex()
            .items_center()
            .p(px(12.0))
            .border_b_1()
            .border_color(style.list.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .text_size(px(13.0))
                            .text_color(style.list.foreground)
                            .child("搜索:"),
                    )
                    .child(div().w(px(200.0)).child(Input::new(&search_input)))
                    .child(
                        action_button(
                            "search-btn",
                            "搜索",
                            ActionRole::Main,
                            ActionSize::Compact,
                            style,
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| this.do_search(cx)),
                        ),
                    ),
            )
            .into_any_element()
    }

    fn render_config_row(
        &self,
        item: &GlobalConfig,
        column_widths: &[Pixels; 6],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = item.id;
        let item_to_edit = item.clone();
        let store = self.store.clone();
        let entity = cx.entity();
        let style = ManagementStyle::current(cx);

        list_row(style)
            .debug_selector(move || format!("GLOBAL_CONFIG_ROW_{}", id))
            .child(list_cell(Some(column_widths[0]), style).child(item.id.to_string()))
            .child(
                list_cell(Some(column_widths[1]), style)
                    .text_size(px(13.0))
                    .font_weight(FontWeight::MEDIUM)
                    .child(item.name.clone()),
            )
            .child(list_cell(Some(column_widths[2]), style).child(item.key.clone()))
            .child(
                list_cell(Some(column_widths[3]), style)
                    .text_size(px(11.0))
                    .text_color(style.list.muted_foreground)
                    .bg(style.list.muted)
                    .px(px(6.0))
                    .py(px(1.0))
                    .rounded(px(3.0))
                    .child(item.config_type.clone()),
            )
            .child(
                list_cell(Some(column_widths[4]), style)
                    .truncate()
                    .child(single_line_preview(&item.data)),
            )
            .child(
                list_actions(Some(column_widths[5]), style)
                    .debug_selector(move || format!("GLOBAL_CONFIG_ACTIONS_{}", id))
                    .child(
                        action_button(
                            ("edit", id as u64),
                            "编辑",
                            ActionRole::Edit,
                            ActionSize::Row,
                            style,
                        )
                        .on_mouse_down(MouseButton::Left, {
                            let entity = entity.clone();
                            move |_, _, cx| {
                                entity.update(cx, |this, cx| this.open_edit(&item_to_edit, cx));
                            }
                        }),
                    )
                    .child(
                        action_button(
                            ("delete", id as u64),
                            "删除",
                            ActionRole::Delete,
                            ActionSize::Row,
                            style,
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            move |_, _, cx| {
                                if let Some(store) = store.clone() {
                                    let entity = entity.clone();
                                    cx.spawn(async move |cx| {
                                        _ = store.delete_global_config(id).await;
                                        entity.update(cx, |this, cx| this.reload(cx));
                                    })
                                    .detach();
                                }
                            },
                        ),
                    ),
            )
            .into_any_element()
    }

    fn render_config_list(&self, cx: &mut Context<Self>) -> AnyElement {
        let column_widths = config_column_widths();
        let mut rows = Vec::with_capacity(self.items.len());
        for item in &self.items {
            rows.push(self.render_config_row(item, &column_widths, cx));
        }

        let style = ManagementStyle::current(cx);
        list_container(style)
            .debug_selector(|| "GLOBAL_CONFIG_LIST".to_owned())
            .child(
                render_table_header(&column_widths, style)
                    .debug_selector(|| "GLOBAL_CONFIG_HEADER".to_owned()),
            )
            .children(rows)
            .into_any_element()
    }

    fn render_list_area(&self, cx: &mut Context<Self>) -> AnyElement {
        let style = ManagementStyle::current(cx);
        let content = if self.items.is_empty() {
            div()
                .flex()
                .items_center()
                .justify_center()
                .h_full()
                .text_color(style.list.muted_foreground)
                .text_size(px(14.0))
                .child("暂无数据")
                .into_any_element()
        } else {
            self.render_config_list(cx)
        };

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_y_scrollbar()
            .p(px(16.0))
            .child(content)
            .into_any_element()
    }

    fn render_pagination(&self, cx: &mut Context<Self>) -> AnyElement {
        let total_pages = ((self.total + PAGE_SIZE - 1) / PAGE_SIZE).max(1);
        let has_previous = self.page > 1;
        let has_next = self.page < total_pages;
        let style = ManagementStyle::current(cx);

        div()
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_between()
            .p(px(12.0))
            .border_t_1()
            .border_color(style.list.border)
            .child(
                div()
                    .text_size(px(13.0))
                    .text_color(style.list.muted_foreground)
                    .child(format!("共 {} 条", self.total)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(page_button(
                        "prev-page",
                        "上一页",
                        has_previous,
                        style,
                        cx.listener(|this, _, _, cx| this.prev_page(cx)),
                    ))
                    .child(
                        div()
                            .text_size(px(13.0))
                            .text_color(style.list.muted_foreground)
                            .child(format!("第 {} / {} 页", self.page, total_pages)),
                    )
                    .child(page_button(
                        "next-page",
                        "下一页",
                        has_next,
                        style,
                        cx.listener(|this, _, _, cx| this.next_page(cx)),
                    )),
            )
            .into_any_element()
    }
}

impl Render for GlobalConfigView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_loaded(cx);

        let form_overlay = self.render_form_overlay(window, cx);
        let header = self.render_header(cx);
        let search_bar = self.render_search_bar(window, cx);
        let list_area = self.render_list_area(cx);
        let pagination = self.render_pagination(cx);

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(header)
            .child(search_bar)
            .child(list_area)
            .child(pagination)
            .child(form_overlay)
    }
}

fn config_column_widths() -> [Pixels; 6] {
    [
        px(60.0),
        px(150.0),
        px(140.0),
        px(100.0),
        px(180.0),
        px(120.0),
    ]
}

fn render_table_header(column_widths: &[Pixels; 6], style: ManagementStyle) -> Div {
    const LABELS: [&str; 6] = ["ID", "名称", "Key", "类型", "数据值", "操作"];

    list_header(style).children(
        LABELS
            .into_iter()
            .zip(column_widths.iter().copied())
            .map(|(label, width)| list_header_cell(Some(width), style).child(label)),
    )
}

fn page_button(
    id: &'static str,
    label: &'static str,
    enabled: bool,
    style: ManagementStyle,
    handler: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let role = if enabled {
        ActionRole::Main
    } else {
        ActionRole::Disabled
    };
    action_button(id, label, role, ActionSize::Compact, style)
        .on_mouse_down(MouseButton::Left, handler)
}

fn field_with_input(
    label: &'static str,
    input: Entity<InputState>,
    style: ManagementStyle,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(4.0))
        .child(
            div()
                .text_size(px(12.0))
                .text_color(style.list.muted_foreground)
                .child(label),
        )
        .child(Input::new(&input))
}

fn type_selector(
    select_state: &Entity<SelectState<SearchableVec<TypeSelectItem>>>,
    style: &ManagementStyle,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(4.0))
        .child(
            div()
                .text_size(px(12.0))
                .text_color(style.list.muted_foreground)
                .child("类型"),
        )
        .child(
            div()
                .w_full()
                .child(Select::new(select_state).placeholder("请选择类型")),
        )
}

fn bool_radio(
    label: &'static str,
    active: bool,
    entity: Entity<GlobalConfigView>,
    style: ManagementStyle,
) -> impl IntoElement {
    let val = label.to_string();
    let main_colors = style.action(ActionRole::Main);
    let neutral_colors = style.action(ActionRole::Neutral);
    let dot_color = if active {
        main_colors.background
    } else {
        style.list.row
    };
    let border = if active {
        main_colors.background
    } else {
        neutral_colors.background
    };
    div()
        .flex()
        .flex_row()
        .gap(px(6.0))
        .items_center()
        .cursor(CursorStyle::PointingHand)
        .child(
            div()
                .w(px(16.0))
                .h(px(16.0))
                .rounded_full()
                .border_2()
                .border_color(border)
                .flex()
                .items_center()
                .justify_center()
                .child(div().w(px(8.0)).h(px(8.0)).rounded_full().bg(dot_color)),
        )
        .child(
            div()
                .text_size(px(13.0))
                .text_color(style.list.foreground)
                .child(label),
        )
        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
            entity.update(cx, |this, cx| {
                this.form_data = val.clone().into();
                cx.notify();
            });
        })
}

#[cfg(test)]
mod tests {
    use super::{convert_config_value, single_line_preview};

    #[test]
    fn multiline_config_data_is_safe_for_single_line_cells() {
        let preview = single_line_preview("{\r\n  \"enabled\": true\n}");

        assert_eq!(preview, "{ \"enabled\": true }");
        assert!(!preview.contains('\n'));
        assert!(!preview.contains('\r'));
    }

    #[test]
    fn text_values_are_converted_for_the_selected_type() {
        assert_eq!(convert_config_value(" 42.5 ", "text", "number"), "42.5");
        assert_eq!(convert_config_value("not-a-number", "text", "number"), "0");
        assert_eq!(convert_config_value(" true ", "text", "boolean"), "true");
        assert_eq!(convert_config_value("yes", "text", "boolean"), "false");
        assert_eq!(
            convert_config_value("{ \"enabled\": true }", "text", "json"),
            "{\"enabled\":true}"
        );
        assert_eq!(convert_config_value("not-json", "text", "json"), "{}");
    }

    #[test]
    fn json_values_follow_the_same_target_conversion_rules() {
        assert_eq!(convert_config_value("12", "json", "number"), "12");
        assert_eq!(convert_config_value("false", "json", "boolean"), "false");
        assert_eq!(
            convert_config_value("{\"name\":\"HiveClaw\"}", "json", "text"),
            "{\"name\":\"HiveClaw\"}"
        );
    }

    #[test]
    fn empty_or_same_type_values_are_not_changed() {
        assert_eq!(convert_config_value("", "text", "number"), "");
        assert_eq!(convert_config_value("   ", "json", "boolean"), "   ");
        assert_eq!(convert_config_value("001", "number", "number"), "001");
    }

    #[gpui::test]
    fn global_config_list_matches_reference_geometry(cx: &mut gpui::TestAppContext) {
        use crate::datasource::GlobalConfig;
        use gpui::{VisualTestContext, px, size};

        cx.update(|cx| {
            gpui_component::theme::init(cx);
            gpui_component::init(cx);
        });
        let window = cx.open_window(size(px(1000.0), px(600.0)), |_, cx| {
            let mut view = super::GlobalConfigView::new(cx);
            view.loaded = true;
            view.total = 1;
            view.items = vec![GlobalConfig {
                id: 1,
                name: "测试配置".to_string(),
                key: "test.key".to_string(),
                config_type: "text".to_string(),
                data: "value".to_string(),
                created_at: String::new(),
                updated_at: String::new(),
            }];
            view
        });
        cx.run_until_parked();

        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let list = cx.debug_bounds("GLOBAL_CONFIG_LIST").expect("list bounds");
        let header = cx
            .debug_bounds("GLOBAL_CONFIG_HEADER")
            .expect("header bounds");
        let row = cx.debug_bounds("GLOBAL_CONFIG_ROW_1").expect("row bounds");
        let actions = cx
            .debug_bounds("GLOBAL_CONFIG_ACTIONS_1")
            .expect("action bounds");
        assert_eq!(header.left(), row.left());
        assert_eq!(header.right(), row.right());
        assert_eq!(header.bottom(), row.top());
        assert!(row.bottom() <= list.bottom());
        assert!(actions.left() >= row.left());
        assert!(actions.right() <= row.right());
    }
}
