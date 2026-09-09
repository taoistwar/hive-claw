//! Tool management view for feature 011 (US11).
//! scroll:tool_list

use crate::datasource::{
    Store, ToolInput, ToolKind, ToolRecord, ToolSource, ToolStore,
    validation::{PublicBoundaryError, PublicErrorEnvelope},
};
use crate::ui::management_style::{
    ActionRole, ActionSize, ManagementStyle, action_button, list_actions, list_cell,
    list_container, list_header, list_header_cell, list_row, management_modal_layer,
    management_modal_panel, management_modal_scroll,
};
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::ActiveTheme as _;
use gpui_component::FocusTrapElement as _;
use gpui_component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_component::scroll::{Scrollable, ScrollableElement};

actions!(hivegui_tool_form, [ToolFormTab, ToolFormTabPrev]);

pub struct ToolView {
    store: Entity<Store>,
    items: Vec<ToolRecord>,
    loading: bool,
    search_text: String,
    current_page: i64,
    page_size: i64,
    total_count: i64,
    show_form: bool,
    form_focus: FocusHandle,
    add_focus: FocusHandle,
    save_focus: FocusHandle,
    error_focus: FocusHandle,
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
    form_category_id: String,
    form_required_capabilities: String,
    form_is_always: bool,
    kind_select_open: bool,
    source_select_open: bool,
    error_message: Option<String>,
    identifier_conflict: bool,
    confirm_delete_id: Option<i64>,
    identifier_input: Option<Entity<InputState>>,
    name_input: Option<Entity<InputState>>,
    description_input: Option<Entity<TextareaState>>,
    function_id_input: Option<Entity<InputState>>,
    workflow_id_input: Option<Entity<InputState>>,
    input_schema_textarea: Option<Entity<TextareaState>>,
    output_schema_textarea: Option<Entity<TextareaState>>,
    required_capabilities_textarea: Option<Entity<TextareaState>>,
    category_input: Option<Entity<InputState>>,
    search_input: Option<Entity<InputState>>,
}

impl ToolView {
    pub fn new(store: Entity<Store>, cx: &mut Context<Self>) -> Self {
        cx.bind_keys([
            KeyBinding::new("tab", ToolFormTab, Some("HiveguiToolForm")),
            KeyBinding::new("shift-tab", ToolFormTabPrev, Some("HiveguiToolForm")),
        ]);
        let mut v = Self {
            store,
            items: Vec::new(),
            loading: false,
            search_text: String::new(),
            current_page: 1,
            page_size: 20,
            total_count: 0,
            show_form: false,
            form_focus: cx.focus_handle(),
            add_focus: cx.focus_handle(),
            save_focus: cx.focus_handle(),
            error_focus: cx.focus_handle(),
            form_scroll: ScrollHandle::default(),
            editing_id: None,
            form_identifier: String::new(),
            form_name: String::new(),
            form_description: String::new(),
            form_kind: "function-wrap".to_string(),
            form_source: "workspace".into(),
            form_function_id: String::new(),
            form_workflow_id: String::new(),
            form_input_schema: "{}".into(),
            form_output_schema: "{}".into(),
            form_category_id: String::new(),
            form_required_capabilities: String::new(),
            form_is_always: false,
            kind_select_open: false,
            source_select_open: false,
            error_message: None,
            identifier_conflict: false,
            confirm_delete_id: None,
            identifier_input: None,
            name_input: None,
            description_input: None,
            function_id_input: None,
            workflow_id_input: None,
            input_schema_textarea: None,
            output_schema_textarea: None,
            required_capabilities_textarea: None,
            category_input: None,
            search_input: None,
        };
        v.load(cx);
        v
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        let pool = self.store.read(cx).pool().clone();
        let search = if self.search_text.is_empty() {
            None
        } else {
            Some(self.search_text.clone())
        };
        let page = self.current_page;
        cx.spawn(async move |this, cx| {
            let result = match ToolStore::new(pool) {
                Ok(store) => store.list(search, page).await,
                Err(error) => Err(error),
            };
            this.update(cx, |v, cx| match result {
                Ok(page) => {
                    v.items = page.items().to_vec();
                    v.total_count = page.total();
                    v.current_page = page.page();
                    v.loading = false;
                    cx.notify();
                }
                Err(error) => {
                    v.loading = false;
                    v.error_message = Some(tool_error_summary(&error));
                    cx.notify();
                }
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
        self.form_kind = "function-wrap".to_string();
        self.form_source = "workspace".into();
        self.form_function_id.clear();
        self.form_workflow_id.clear();
        self.form_input_schema = "{}".into();
        self.form_output_schema = "{}".into();
        self.form_category_id.clear();
        self.form_required_capabilities.clear();
        self.form_is_always = false;
        self.kind_select_open = false;
        self.source_select_open = false;
        self.error_message = None;
        self.identifier_conflict = false;
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
            TextareaState::new(window, cx)
                .placeholder("工具描述")
                .default_value("")
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
        self.input_schema_textarea = Some(cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("{}")
                .default_value("{}")
        }));
        self.output_schema_textarea = Some(cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("{}")
                .default_value("{}")
        }));
        self.required_capabilities_textarea = Some(cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("[]")
                .default_value("")
        }));
        self.category_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Category ID（可选）")
                .default_value("")
        }));
        self.install_form_focus_lifecycle(window, cx);
        cx.notify();
    }

    fn show_edit_form(&mut self, window: &mut Window, item: ToolRecord, cx: &mut Context<Self>) {
        self.show_form = true;
        self.editing_id = Some(item.id());
        self.form_identifier = item.identifier().to_string();
        self.form_name = item.name().to_string();
        self.form_description = item.description().to_string();
        self.form_kind = item.kind().as_str().to_string();
        self.form_source = item.source().as_str().to_string();
        self.form_function_id = item
            .function_id()
            .map(|i| i.to_string())
            .unwrap_or_default();
        self.form_workflow_id = item
            .workflow_id()
            .map(|i| i.to_string())
            .unwrap_or_default();
        self.form_input_schema = item.input_schema().to_string();
        self.form_output_schema = item.output_schema().to_string();
        self.form_category_id = item
            .category_id()
            .map(|id| id.to_string())
            .unwrap_or_default();
        self.form_required_capabilities = item.required_capabilities().unwrap_or("").to_string();
        self.form_is_always = item.is_always();
        self.kind_select_open = false;
        self.source_select_open = false;
        self.error_message = None;
        self.identifier_conflict = false;
        self.identifier_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("唯一标识符")
                .default_value(item.identifier())
        }));
        self.name_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("工具名称")
                .default_value(item.name())
        }));
        self.description_input = Some(cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("工具描述")
                .default_value(item.description())
        }));
        self.function_id_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("函数 ID")
                .default_value(
                    item.function_id()
                        .map(|i| i.to_string())
                        .unwrap_or_default(),
                )
        }));
        self.workflow_id_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("工作流 ID")
                .default_value(
                    item.workflow_id()
                        .map(|i| i.to_string())
                        .unwrap_or_default(),
                )
        }));
        self.input_schema_textarea = Some(cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("{}")
                .default_value(item.input_schema())
        }));
        self.output_schema_textarea = Some(cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("{}")
                .default_value(item.output_schema())
        }));
        self.required_capabilities_textarea = Some(cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("[]")
                .default_value(item.required_capabilities().unwrap_or(""))
        }));
        self.category_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Category ID（可选）")
                .default_value(
                    item.category_id()
                        .map(|id| id.to_string())
                        .unwrap_or_default(),
                )
        }));
        self.install_form_focus_lifecycle(window, cx);
        cx.notify();
    }

    fn install_form_focus_lifecycle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let save_focus = self.save_focus.clone();
        cx.on_focus(&save_focus, window, |view, _, cx| {
            view.form_scroll.scroll_to_bottom();
            cx.notify();
        })
        .detach();
        self.identifier_input
            .as_ref()
            .expect("Tool identifier input initialized")
            .update(cx, |input, cx| input.focus(window, cx));
    }

    fn focus_form_next(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        window.focus_next(cx);
        if !self.form_focus.contains_focused(window, cx)
            && let Some(identifier) = self.identifier_input.as_ref()
        {
            self.form_scroll.set_offset(point(px(0.0), px(0.0)));
            identifier.read(cx).focus_handle(cx).focus(window, cx);
        }
        cx.notify();
    }

    fn focus_form_prev(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        window.focus_prev(cx);
        if !self.form_focus.contains_focused(window, cx) {
            self.form_scroll.scroll_to_bottom();
            self.save_focus.focus(window, cx);
        }
        cx.notify();
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.show_form {
            return;
        }
        match event.keystroke.key.as_str() {
            "escape" => {
                cx.stop_propagation();
                self.hide_form(window, cx);
            }
            "enter" | " " | "space" if self.save_focus.is_focused(window) => {
                cx.stop_propagation();
                self.save(window, cx);
            }
            _ => {}
        }
    }

    fn hide_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.form_scroll.set_offset(point(px(0.0), px(0.0)));
        self.show_form = false;
        self.editing_id = None;
        self.error_message = None;
        self.identifier_conflict = false;
        self.add_focus.focus(window, cx);
        cx.notify();
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ref inp) = self.identifier_input {
            self.form_identifier = inp.read(cx).value().to_string();
        }
        if let Some(ref inp) = self.name_input {
            self.form_name = inp.read(cx).value().to_string();
        }
        if let Some(ref inp) = self.description_input {
            self.form_description = inp.read(cx).value().to_string();
        }
        if let Some(ref inp) = self.function_id_input {
            self.form_function_id = inp.read(cx).value().to_string();
        }
        if let Some(ref inp) = self.workflow_id_input {
            self.form_workflow_id = inp.read(cx).value().to_string();
        }
        if let Some(ref inp) = self.input_schema_textarea {
            self.form_input_schema = inp.read(cx).value().to_string();
        }
        if let Some(ref inp) = self.output_schema_textarea {
            self.form_output_schema = inp.read(cx).value().to_string();
        }
        if let Some(ref inp) = self.required_capabilities_textarea {
            self.form_required_capabilities = inp.read(cx).value().to_string();
        }
        if let Some(ref inp) = self.category_input {
            self.form_category_id = inp.read(cx).value().to_string();
        }
        let pool = self.store.read(cx).pool().clone();
        let idf = self.form_identifier.clone();
        let name = self.form_name.clone();
        let desc = self.form_description.clone();
        let kind = self.form_kind.clone();
        let source = self.form_source.clone();
        let is_always = self.form_is_always;
        let fid: Option<i64> = if kind == "function-wrap" {
            parse_optional_id(&self.form_function_id)
        } else {
            None
        };
        let wid: Option<i64> = if kind == "workflow-wrap" {
            parse_optional_id(&self.form_workflow_id)
        } else {
            None
        };
        let is = self.form_input_schema.clone();
        let os = self.form_output_schema.clone();
        let category_id = parse_optional_id(&self.form_category_id);
        let capabilities = (!self.form_required_capabilities.trim().is_empty())
            .then(|| self.form_required_capabilities.clone());
        let editing_id = self.editing_id;
        cx.spawn_in(window, async move |this, cx| {
            let result = async {
                let store = ToolStore::new(pool)?;
                store.check_identifier_available(&idf, editing_id).await?;
                let input = ToolInput::for_write(
                    idf,
                    name,
                    desc,
                    ToolKind::try_from(kind.as_str())?,
                    ToolSource::try_from(source.as_str())?,
                    is_always,
                    fid,
                    wid,
                    is,
                    os,
                    category_id,
                    capabilities,
                )?;
                match editing_id {
                    Some(id) => store.update(id, input).await,
                    None => store.create(input).await,
                }
            }
            .await;
            _ = cx.update(|window, cx| {
                _ = this.update(cx, |view, cx| match result {
                    Ok(_) => {
                        view.hide_form(window, cx);
                        view.load(cx);
                    }
                    Err(error) => {
                        view.identifier_conflict = matches!(
                            error.envelope(),
                            PublicErrorEnvelope::Conflict { field, reason, .. }
                                if field == "identifier" && reason == "duplicate"
                        );
                        view.error_message = Some(tool_error_summary(&error));
                        view.error_focus.focus(window, cx);
                        cx.notify();
                    }
                });
            });
        })
        .detach();
    }

    fn delete(&mut self, id: i64, cx: &mut Context<Self>) {
        let pool = self.store.read(cx).pool().clone();
        cx.spawn(async move |this, cx| match ToolStore::new(pool) {
            Ok(store) => match store.delete(id).await {
                Ok(()) => {
                    this.update(cx, |v, cx| v.load(cx)).ok();
                }
                Err(error) => {
                    this.update(cx, |v, cx| {
                        v.error_message = Some(tool_error_summary(&error));
                        cx.notify();
                    })
                    .ok();
                }
            },
            Err(error) => {
                this.update(cx, |v, cx| {
                    v.error_message = Some(tool_error_summary(&error));
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    fn next_page(&mut self, cx: &mut Context<Self>) {
        if self.current_page * self.page_size < self.total_count {
            self.current_page += 1;
            self.load(cx);
        }
    }
    fn prev_page(&mut self, cx: &mut Context<Self>) {
        if self.current_page > 1 {
            self.current_page -= 1;
            self.load(cx);
        }
    }

    fn select_kind(&mut self, new_kind: String, cx: &mut Context<Self>) {
        if self.form_kind != new_kind {
            self.form_function_id.clear();
            self.form_workflow_id.clear();
        }
        self.form_kind = new_kind;
        self.kind_select_open = false;
        cx.notify();
    }

    fn select_source(&mut self, new_source: String, cx: &mut Context<Self>) {
        self.form_source = new_source;
        self.source_select_open = false;
        cx.notify();
    }

    fn toggle_kind(&mut self, cx: &mut Context<Self>) {
        self.kind_select_open = !self.kind_select_open;
        self.source_select_open = false;
        cx.notify();
    }

    fn toggle_source(&mut self, cx: &mut Context<Self>) {
        self.source_select_open = !self.source_select_open;
        self.kind_select_open = false;
        cx.notify();
    }

    fn kind_label(&self) -> &'static str {
        match self.form_kind.as_str() {
            "function-wrap" => "函数",
            "workflow-wrap" => "工作流",
            _ => "未知",
        }
    }
}

fn tool_kind_label(kind: ToolKind) -> &'static str {
    match kind {
        ToolKind::FunctionWrap => "函数",
        ToolKind::WorkflowWrap => "工作流",
    }
}

fn selector_field(
    label: &'static str,
    value: impl Into<SharedString>,
    id: impl Into<ElementId>,
    debug_selector: &'static str,
    theme: &gpui_component::theme::Theme,
    on_toggle: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(4.0))
        .child(
            div()
                .text_size(px(13.0))
                .text_color(theme.foreground)
                .child(label),
        )
        .child(
            div()
                .id(id)
                .debug_selector(move || debug_selector.to_string())
                .h(px(32.0))
                .flex()
                .items_center()
                .justify_between()
                .px(px(8.0))
                .border_1()
                .border_color(theme.border)
                .rounded(px(4.0))
                .bg(theme.background)
                .text_size(px(13.0))
                .cursor(CursorStyle::PointingHand)
                .child(value.into())
                .child("⌄")
                .on_mouse_down(MouseButton::Left, on_toggle),
        )
}

fn selector_menu(theme: &gpui_component::theme::Theme) -> Scrollable<Div> {
    div()
        .flex()
        .flex_col()
        .max_h(px(160.0))
        .overflow_y_scrollbar()
        .border_1()
        .border_color(theme.border)
        .rounded(px(4.0))
        .bg(theme.popover)
        .text_color(theme.popover_foreground)
}

fn selector_option(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    selected: bool,
    theme: &gpui_component::theme::Theme,
    on_select: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let list_hover = theme.list_hover;
    div()
        .id(id)
        .px(px(8.0))
        .py(px(6.0))
        .bg(if selected {
            theme.list_active
        } else {
            theme.popover
        })
        .hover(move |option| option.bg(list_hover))
        .text_size(px(13.0))
        .cursor(CursorStyle::PointingHand)
        .child(label.into())
        .on_mouse_down(MouseButton::Left, on_select)
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
                TextareaState::new(window, cx)
                    .placeholder("工具描述")
                    .default_value(&self.form_description)
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
            self.input_schema_textarea = Some(cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder("{}")
                    .default_value(&self.form_input_schema)
            }));
            self.output_schema_textarea = Some(cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder("{}")
                    .default_value(&self.form_output_schema)
            }));
            self.required_capabilities_textarea = Some(cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder("[]")
                    .default_value(&self.form_required_capabilities)
            }));
            self.category_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("Category ID（可选）")
                    .default_value(&self.form_category_id)
            }));
        }

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
                        .debug_selector(|| "TOOL_ADD".to_string())
                        .role(Role::Button)
                        .aria_label("添加工具")
                        .track_focus(&self.add_focus)
                        .tab_index(0)
                        .on_key_down(cx.listener(|view, event: &KeyDownEvent, window, cx| {
                            if matches!(event.keystroke.key.as_str(), "enter" | " " | "space") {
                                cx.stop_propagation();
                                view.show_add_form(window, cx);
                            }
                        }))
                        .on_click({
                            let t = cx.weak_entity();
                            move |_, window, cx| {
                                t.update(cx, |v, cx| v.show_add_form(window, cx)).ok();
                            }
                        })
                        .when(self.add_focus.is_focused(window), |button| {
                            button.child(focus_marker("TOOL_ADD_FOCUSED"))
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
                                    .child(
                                        list_header_cell(Some(col_widths[1]), style).child("名称"),
                                    )
                                    .child(
                                        list_header_cell(Some(col_widths[2]), style)
                                            .child("Identifier"),
                                    )
                                    .child(
                                        list_header_cell(Some(col_widths[3]), style).child("Kind"),
                                    )
                                    .child(
                                        list_header_cell(Some(col_widths[4]), style).child("描述"),
                                    )
                                    .child(
                                        list_header_cell(Some(col_widths[5]), style).child("操作"),
                                    ),
                            )
                            .children(self.items.iter().map(|item| {
                                let id = item.id();
                                let ic = item.clone();
                                list_row(style)
                                    .child(
                                        list_cell(Some(col_widths[0]), style)
                                            .child(format!("{}", item.id())),
                                    )
                                    .child(
                                        list_cell(Some(col_widths[1]), style)
                                            .child(item.name().to_string()),
                                    )
                                    .child(
                                        list_cell(Some(col_widths[2]), style)
                                            .child(item.identifier().to_string()),
                                    )
                                    .child(
                                        list_cell(Some(col_widths[3]), style)
                                            .child(tool_kind_label(item.kind())),
                                    )
                                    .child(
                                        list_cell(Some(col_widths[4]), style)
                                            .child(item.description().to_string()),
                                    )
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
                                    if self.current_page > 1 {
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
                                    .child(format!("第 {} / {} 页", self.current_page, tp.max(1))),
                            )
                            .child(
                                action_button(
                                    "next",
                                    "下一页",
                                    if self.current_page * self.page_size < self.total_count {
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
                let function_id_input = self.function_id_input.clone().unwrap();
                let workflow_id_input = self.workflow_id_input.clone().unwrap();
                let input_schema_textarea = self.input_schema_textarea.clone().unwrap();
                let output_schema_textarea = self.output_schema_textarea.clone().unwrap();
                let required_capabilities_textarea =
                    self.required_capabilities_textarea.clone().unwrap();
                let category_input = self.category_input.clone().unwrap();
                let kind_label = self.kind_label().to_string();
                let source_label = self.form_source.clone();
                let kind_select_open = self.kind_select_open;
                let source_select_open = self.source_select_open;
                let form_kind = self.form_kind.clone();
                let form_source = self.form_source.clone();
                let identifier_focused = identifier_input
                    .read(cx)
                    .focus_handle(cx)
                    .is_focused(window);
                let current_identifier = identifier_input.read(cx).value().to_string();
                let view_handle = cx.weak_entity();
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
                            let t = view_handle.clone();
                            move |_, window, cx| {
                                t.update(cx, |v, cx| v.hide_form(window, cx)).ok();
                            }
                        }),
                )
                .child(
                    management_modal_panel(
                        management_modal_layer(px(550.0), window.bounds().size.height - px(48.0)),
                        theme.popover,
                        theme.foreground,
                        theme.border,
                    )
                    .debug_selector(|| "TOOL_MODAL".to_string())
                    .track_focus(&self.form_focus)
                    .focus_trap("tool-form-focus-trap", &self.form_focus)
                    .key_context("HiveguiToolForm")
                    .on_action(cx.listener(|view, _: &ToolFormTab, window, cx| {
                        view.focus_form_next(window, cx);
                    }))
                    .on_action(cx.listener(|view, _: &ToolFormTabPrev, window, cx| {
                        view.focus_form_prev(window, cx);
                    }))
                    .capture_key_down(cx.listener(|view, event: &KeyDownEvent, window, cx| {
                        view.on_key_down(event, window, cx);
                    }))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .child(
                        management_modal_scroll("tool-form-scroll", &self.form_scroll)
                            .debug_selector(|| "TOOL_FORM_SCROLL".to_string())
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
                            .child(form_field(
                                "Identifier *",
                                identifier_input,
                                "TOOL_IDENTIFIER_INPUT",
                                theme,
                            ))
                            .when(identifier_focused, |form| {
                                form.child(focus_marker("TOOL_IDENTIFIER_FOCUSED"))
                            })
                            .child(focus_marker(format!(
                                "TOOL_IDENTIFIER_VALUE-{current_identifier}"
                            )))
                            .child(form_field("名称 *", name_input, "TOOL_NAME_INPUT", theme))
                            .child(textarea_field(
                                "描述 *",
                                description_input,
                                "TOOL_DESCRIPTION_INPUT",
                                theme,
                            ))
                            .child(
                                selector_field(
                                    "类型 *",
                                    kind_label,
                                    "tool-kind-toggle",
                                    "TOOL_KIND_SELECT",
                                    theme,
                                    {
                                        let view = view_handle.clone();
                                        move |_, _, cx| {
                                            view.update(cx, |v, cx| v.toggle_kind(cx)).ok();
                                        }
                                    },
                                )
                                .when(kind_select_open, |field| {
                                    let kind_view = view_handle.clone();
                                    field.child(
                                        selector_menu(theme).children(
                                            [
                                                ("function-wrap", "函数"),
                                                ("workflow-wrap", "工作流"),
                                            ]
                                            .into_iter()
                                            .map(
                                                move |(k, l)| {
                                                    let label = l.to_string();
                                                    let selected = k == form_kind;
                                                    let option_id = format!("tool-kind-option-{k}");
                                                    let view = kind_view.clone();
                                                    selector_option(
                                                        option_id,
                                                        label,
                                                        selected,
                                                        theme,
                                                        move |_, _, cx| {
                                                            view.update(cx, |view, cx| {
                                                                if view.form_kind != k {
                                                                    view.form_function_id.clear();
                                                                    view.form_workflow_id.clear();
                                                                }
                                                                view.form_kind = k.to_string();
                                                                view.kind_select_open = false;
                                                                cx.notify();
                                                            })
                                                            .ok();
                                                        },
                                                    )
                                                },
                                            ),
                                        ),
                                    )
                                }),
                            )
                            .child(
                                selector_field(
                                    "Source",
                                    source_label,
                                    "tool-source-toggle",
                                    "TOOL_SOURCE_SELECT",
                                    theme,
                                    {
                                        let view = view_handle.clone();
                                        move |_, _, cx| {
                                            view.update(cx, |v, cx| v.toggle_source(cx)).ok();
                                        }
                                    },
                                )
                                .when(
                                    source_select_open,
                                    |field| {
                                        let source_view = view_handle.clone();
                                        field.child(
                                            selector_menu(theme).children(
                                                [
                                                    ("workspace", "workspace"),
                                                    ("builtin", "builtin"),
                                                ]
                                                .into_iter()
                                                .map(move |(v, l)| {
                                                    let label = l.to_string();
                                                    let value = v.to_string();
                                                    let selected = v == form_source;
                                                    let option_id =
                                                        format!("tool-source-option-{v}");
                                                    let view = source_view.clone();
                                                    selector_option(
                                                        option_id,
                                                        label,
                                                        selected,
                                                        theme,
                                                        move |_, _, cx| {
                                                            view.update(cx, |view, cx| {
                                                                view.form_source = value.clone();
                                                                view.source_select_open = false;
                                                                cx.notify();
                                                            })
                                                            .ok();
                                                        },
                                                    )
                                                }),
                                            ),
                                        )
                                    },
                                ),
                            )
                            .child(
                                div()
                                    .id("tool-is-always")
                                    .debug_selector(|| "TOOL_IS_ALWAYS".to_string())
                                    .role(Role::CheckBox)
                                    .aria_label("始终启用")
                                    .cursor(CursorStyle::PointingHand)
                                    .child(if self.form_is_always {
                                        "☑ 始终启用"
                                    } else {
                                        "☐ 始终启用"
                                    })
                                    .on_click({
                                        let view = view_handle.clone();
                                        move |_, _, cx| {
                                            view.update(cx, |view, cx| {
                                                view.form_is_always = !view.form_is_always;
                                                cx.notify();
                                            })
                                            .ok();
                                        }
                                    }),
                            )
                            .when(self.form_kind == "function-wrap", |this| {
                                this.child(form_field(
                                    "Function ID *",
                                    function_id_input,
                                    "TOOL_FUNCTION_TARGET",
                                    theme,
                                ))
                            })
                            .when(self.form_kind == "workflow-wrap", |this| {
                                this.child(form_field(
                                    "Workflow ID *",
                                    workflow_id_input,
                                    "TOOL_WORKFLOW_TARGET",
                                    theme,
                                ))
                            })
                            .child(textarea_field(
                                "Input Schema (JSON)",
                                input_schema_textarea,
                                "TOOL_INPUT_SCHEMA_TEXTAREA",
                                theme,
                            ))
                            .child(textarea_field(
                                "Output Schema (JSON)",
                                output_schema_textarea,
                                "TOOL_OUTPUT_SCHEMA_TEXTAREA",
                                theme,
                            ))
                            .child(form_field(
                                "Category ID",
                                category_input,
                                "TOOL_CATEGORY_SELECT",
                                theme,
                            ))
                            .child(textarea_field(
                                "Required Capabilities (JSON)",
                                required_capabilities_textarea,
                                "TOOL_REQUIRED_CAPABILITIES_TEXTAREA",
                                theme,
                            ))
                            .when_some(self.error_message.as_ref(), |this, err| {
                                this.child(
                                    div()
                                        .id("tool-form-error")
                                        .track_focus(&self.error_focus)
                                        .when(self.identifier_conflict, |error| {
                                            error
                                                .debug_selector(|| {
                                                    "TOOL_CONFLICT_IDENTIFIER".to_string()
                                                })
                                                .role(Role::Alert)
                                                .aria_label("field=identifier reason=duplicate")
                                        })
                                        .when(self.error_focus.is_focused(window), |error| {
                                            error.child(focus_marker("TOOL_ERROR_FOCUSED"))
                                        })
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
                                    .debug_selector(|| "TOOL_FORM_ACTIONS".to_string())
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
                                                move |_, window, cx| {
                                                    t.update(cx, |v, cx| v.hide_form(window, cx))
                                                        .ok();
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
                                        .track_focus(&self.save_focus)
                                        .tab_index(0)
                                        .when(self.save_focus.is_focused(window), |button| {
                                            button.child(focus_marker("TOOL_FORM_SAVE_FOCUSED"))
                                        })
                                        .on_click({
                                            let t = cx.weak_entity();
                                            move |_, window, cx| {
                                                t.update(cx, |v, cx| v.save(window, cx)).ok();
                                            }
                                        }),
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

fn form_field(
    label: &'static str,
    input: Entity<InputState>,
    selector: &'static str,
    theme: &gpui_component::theme::Theme,
) -> impl IntoElement {
    div()
        .debug_selector(move || selector.to_string())
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

fn textarea_field(
    label: &'static str,
    input: Entity<TextareaState>,
    selector: &'static str,
    theme: &gpui_component::theme::Theme,
) -> impl IntoElement {
    div()
        .debug_selector(move || selector.to_string())
        .flex()
        .flex_col()
        .gap(px(4.0))
        .child(div().text_size(px(13.0)).child(label))
        .child(
            Textarea::new(&input)
                .w_full()
                .min_h(px(72.0))
                .px(px(8.0))
                .py(px(8.0))
                .border_1()
                .border_color(theme.border)
                .rounded(px(4.0)),
        )
}

fn focus_marker(selector: impl Into<SharedString>) -> Stateful<Div> {
    let selector = selector.into();
    let debug_selector = selector.clone();
    div()
        .id(selector)
        .debug_selector(move || debug_selector.to_string())
        .w(px(0.0))
        .h(px(0.0))
        .overflow_hidden()
}

fn parse_optional_id(value: &str) -> Option<i64> {
    value.trim().parse::<i64>().ok()
}

fn tool_error_summary(error: &PublicBoundaryError) -> String {
    match error.envelope() {
        PublicErrorEnvelope::InvalidInput { field, reason } => {
            format!("field={field} reason={reason}")
        }
        PublicErrorEnvelope::Conflict { field, reason, .. } => {
            let value = error.value().unwrap_or_default();
            let references = error.references().join(",");
            format!("field={field} reason={reason} value={value} references={references}")
        }
        PublicErrorEnvelope::NotFound => "reason=not_found".to_string(),
        PublicErrorEnvelope::Forbidden => "reason=forbidden".to_string(),
        PublicErrorEnvelope::Internal { reason } => format!("reason={reason}"),
    }
}
