//! Function management view for feature 011 (US9).
//! scroll:function_list

use crate::datasource::{
    FunctionInput, FunctionKind, FunctionRecord, FunctionStore, Store,
    entity_store::{Capability, Category, Function as RuntimeFunction, Plugin},
    plugin_manifest::manifest_exports,
    validation::{PublicBoundaryError, PublicErrorEnvelope},
};
use crate::ui::management_style::{
    ActionRole, ActionSize, ManagementStyle, action_button, list_actions, list_cell,
    list_container, list_header, list_header_cell, list_row, management_modal_layer,
    management_modal_panel, management_modal_scroll,
};
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_component::scroll::{Scrollable, ScrollableElement};
use gpui_component::{ActiveTheme as _, FocusTrapElement as _};
use std::collections::{HashMap, HashSet};

actions!(
    hivegui_function_form,
    [FunctionFormTab, FunctionFormTabPrev]
);

/// Parsed schema field for test input form
#[derive(Debug, Clone)]
struct SchemaField {
    key: String,
    field_type: String, // "string" | "number" | "integer" | "boolean" | "object"
    description: Option<String>,
    enum_values: Option<Vec<String>>,
    default_value: Option<serde_json::Value>,
}

/// Test dialog state
#[derive(Debug, Clone)]
enum TestState {
    Idle,
    Running,
    Success { output: String, elapsed_ms: i64 },
    Error { message: String },
}

#[derive(Debug, Clone)]
struct FunctionUiError {
    summary: String,
    semantic: FunctionSemanticStatus,
}

/// Semantic states emitted by the Function management surface.
///
/// The same builder is used by the rendered view and the narrow AccessKit
/// probe below, so tests inspect GPUI's actual `Element` accessibility data
/// instead of a parallel label registry.
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FunctionSemanticStatus {
    BuiltinImmutable {
        id: i64,
        identifier: String,
    },
    PlaceholderNonExecutable {
        id: i64,
        identifier: String,
    },
    PlaceholderSchemaOnly,
    IdentifierConflict {
        value: String,
        field: String,
        reason: String,
    },
    FormError {
        label: String,
    },
    TestSuccess,
    TestError,
}

impl FunctionSemanticStatus {
    fn element_id(&self) -> String {
        match self {
            Self::BuiltinImmutable { id, .. } => format!("FUNCTION_BUILTIN_READONLY-{id}"),
            Self::PlaceholderNonExecutable { id, .. } => {
                format!("FUNCTION_PLACEHOLDER_NON_EXECUTABLE-{id}")
            }
            Self::PlaceholderSchemaOnly => "function-placeholder-schema-only".to_string(),
            Self::IdentifierConflict { value, .. } => {
                format!("FUNCTION_IDENTIFIER_CONFLICT-{value}")
            }
            Self::FormError { .. } => "FUNCTION_FORM_ERROR".to_string(),
            Self::TestSuccess => "function-test-success".to_string(),
            Self::TestError => "function-test-error".to_string(),
        }
    }

    fn debug_selector(&self) -> String {
        match self {
            Self::PlaceholderSchemaOnly => "FUNCTION_PLACEHOLDER_SCHEMA_ONLY".to_string(),
            Self::TestSuccess => "FUNCTION_TEST_SUCCESS".to_string(),
            Self::TestError => "FUNCTION_TEST_ERROR".to_string(),
            _ => self.element_id(),
        }
    }

    fn role(&self) -> Role {
        match self {
            Self::IdentifierConflict { .. } | Self::FormError { .. } | Self::TestError => {
                Role::Alert
            }
            Self::BuiltinImmutable { .. }
            | Self::PlaceholderNonExecutable { .. }
            | Self::PlaceholderSchemaOnly
            | Self::TestSuccess => Role::Status,
        }
    }

    fn label(&self) -> String {
        match self {
            Self::BuiltinImmutable { identifier, .. } => {
                format!("{identifier} builtin_immutable")
            }
            Self::PlaceholderNonExecutable { identifier, .. } => {
                format!("{identifier} function_not_executable")
            }
            Self::PlaceholderSchemaOnly => "function_not_executable schema_only".to_string(),
            Self::IdentifierConflict {
                value,
                field,
                reason,
            } => format!("{value} field={field} reason={reason}"),
            Self::FormError { label } => label.clone(),
            Self::TestSuccess => "status=success".to_string(),
            Self::TestError => "status=error".to_string(),
        }
    }
}

fn function_semantic_element(semantic: FunctionSemanticStatus) -> Stateful<Div> {
    let element_id = semantic.element_id();
    let debug_selector = semantic.debug_selector();
    let role = semantic.role();
    let label = semantic.label();
    div()
        .id(element_id)
        .debug_selector(move || debug_selector.clone())
        .role(role)
        .aria_label(label)
}

/// Build the exact semantic element used by `FunctionView` and ask GPUI's
/// `Element` implementation to populate a real AccessKit node.
#[doc(hidden)]
pub fn function_semantic_accesskit_probe(
    semantic: FunctionSemanticStatus,
) -> gpui::accesskit::Node {
    let element = function_semantic_element(semantic);
    let role = element
        .a11y_role()
        .expect("Function semantic elements always expose an AccessKit role");
    let mut node = gpui::accesskit::Node::new(role);
    element.write_a11y_info(&mut node);
    node
}

pub struct FunctionView {
    store: Entity<Store>,
    items: Vec<RuntimeFunction>,
    plugins: Vec<Plugin>,
    capabilities: Vec<Capability>,
    categories: Vec<Category>,
    loading: bool,
    search_text: String,
    current_page: i64,
    page_size: i64,
    total_count: i64,
    show_form: bool,
    form_focus: FocusHandle,
    add_focus: FocusHandle,
    kind_focus: FocusHandle,
    plugin_focus: FocusHandle,
    export_focus: FocusHandle,
    capability_focus: FocusHandle,
    category_focus: FocusHandle,
    cancel_focus: FocusHandle,
    save_focus: FocusHandle,
    error_focus: FocusHandle,
    test_capability_focus: FocusHandle,
    form_scroll: ScrollHandle,
    editing_id: Option<i64>,
    form_identifier: String,
    form_name: String,
    form_description: String,
    form_kind: String,
    form_plugin_id: Option<i64>,
    form_plugin_export: Option<String>,
    form_capability: Option<String>,
    form_category_id: Option<i64>,
    plugin_exports: Vec<String>,
    kind_select_open: bool,
    plugin_select_open: bool,
    export_select_open: bool,
    capability_select_open: bool,
    category_select_open: bool,
    form_input_schema: String,
    form_output_schema: String,
    error_message: Option<String>,
    form_error: Option<FunctionUiError>,
    confirm_delete_id: Option<i64>,
    identifier_input: Option<Entity<InputState>>,
    name_input: Option<Entity<InputState>>,
    description_input: Option<Entity<InputState>>,
    input_schema_input: Option<Entity<TextareaState>>,
    output_schema_input: Option<Entity<TextareaState>>,
    search_input: Option<Entity<InputState>>,
    // Test dialog fields
    show_test: bool,
    test_function: Option<RuntimeFunction>,
    test_scroll: ScrollHandle,
    test_state: TestState,
    test_inputs: HashMap<String, String>,
    test_capabilities: HashSet<String>,
    test_available_capabilities: Vec<Capability>,
    test_capability_select_open: bool,
    test_capability_scroll: ScrollHandle,
    test_capability_filter: String,
    test_capability_filter_input: Option<Entity<InputState>>,
    parsed_schema_fields: Vec<SchemaField>,
    is_primitive_schema: bool,
    test_input_states: Vec<(String, Entity<InputState>)>,
}

impl FunctionView {
    pub fn new(store: Entity<Store>, cx: &mut Context<Self>) -> Self {
        cx.bind_keys([
            KeyBinding::new("tab", FunctionFormTab, Some("HiveguiFunctionForm")),
            KeyBinding::new(
                "shift-tab",
                FunctionFormTabPrev,
                Some("HiveguiFunctionForm"),
            ),
        ]);
        let mut v = Self {
            store,
            items: Vec::new(),
            plugins: Vec::new(),
            capabilities: Vec::new(),
            categories: Vec::new(),
            loading: false,
            search_text: String::new(),
            current_page: 1,
            page_size: 20,
            total_count: 0,
            show_form: false,
            form_focus: cx.focus_handle(),
            add_focus: cx.focus_handle(),
            kind_focus: cx.focus_handle(),
            plugin_focus: cx.focus_handle(),
            export_focus: cx.focus_handle(),
            capability_focus: cx.focus_handle(),
            category_focus: cx.focus_handle(),
            cancel_focus: cx.focus_handle(),
            save_focus: cx.focus_handle(),
            error_focus: cx.focus_handle(),
            test_capability_focus: cx.focus_handle(),
            form_scroll: ScrollHandle::default(),
            editing_id: None,
            form_identifier: String::new(),
            form_name: String::new(),
            form_description: String::new(),
            form_kind: "placeholder".to_string(),
            form_plugin_id: None,
            form_plugin_export: None,
            form_capability: None,
            form_category_id: None,
            plugin_exports: Vec::new(),
            kind_select_open: false,
            plugin_select_open: false,
            export_select_open: false,
            capability_select_open: false,
            category_select_open: false,
            form_input_schema: "{}".into(),
            form_output_schema: "{}".into(),
            error_message: None,
            form_error: None,
            confirm_delete_id: None,
            identifier_input: None,
            name_input: None,
            description_input: None,
            input_schema_input: None,
            output_schema_input: None,
            search_input: None,
            show_test: false,
            test_function: None,
            test_scroll: ScrollHandle::default(),
            test_state: TestState::Idle,
            test_inputs: HashMap::new(),
            test_capabilities: HashSet::new(),
            test_available_capabilities: Vec::new(),
            test_capability_select_open: false,
            test_capability_scroll: ScrollHandle::default(),
            test_capability_filter: String::new(),
            test_capability_filter_input: None,
            parsed_schema_fields: Vec::new(),
            is_primitive_schema: false,
            test_input_states: Vec::new(),
        };
        v.load(cx);
        v.load_options(cx);
        v
    }

    fn load_options(&mut self, cx: &mut Context<Self>) {
        let store = self.store.read(cx).clone();
        cx.spawn(async move |this, cx| {
            let plugins = Plugin::list(store.pool(), None, 1_000, 0).await?;
            let capabilities = Capability::list(store.pool(), None, 1_000, 0).await?;
            let categories = Category::list_all(store.pool()).await?;
            this.update(cx, |view, cx| {
                view.plugins = plugins;
                view.capabilities = capabilities;
                view.categories = categories;
                view.refresh_plugin_exports();
                cx.notify();
            })
        })
        .detach();
    }

    fn refresh_plugin_exports(&mut self) {
        self.plugin_exports = self
            .form_plugin_id
            .and_then(|id| self.plugins.iter().find(|plugin| plugin.id == id))
            .and_then(|plugin| plugin.manifest.as_deref())
            .map(|manifest| manifest_exports(Some(manifest)))
            .unwrap_or_default();

        if !self.plugin_exports.is_empty()
            && self
                .form_plugin_export
                .as_ref()
                .is_some_and(|name| !self.plugin_exports.contains(name))
        {
            self.form_plugin_export = None;
        }
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
            let result = match FunctionStore::new(pool) {
                Ok(store) => store.list(search, page).await,
                Err(error) => Err(error),
            };
            this.update(cx, |v, cx| match result {
                Ok(page) => {
                    v.items = page
                        .items()
                        .iter()
                        .cloned()
                        .map(FunctionRecord::into_legacy_entity)
                        .collect();
                    v.total_count = page.total();
                    v.current_page = page.page();
                    v.loading = false;
                    cx.notify();
                }
                Err(error) => {
                    v.loading = false;
                    v.error_message = Some(format!(
                        "加载失败: {}",
                        FunctionUiError::from_public(&error).summary
                    ));
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
        self.form_kind = "placeholder".to_string();
        self.form_plugin_id = None;
        self.form_plugin_export = None;
        self.form_capability = None;
        self.form_category_id = None;
        self.plugin_exports.clear();
        self.kind_select_open = false;
        self.plugin_select_open = false;
        self.export_select_open = false;
        self.capability_select_open = false;
        self.category_select_open = false;
        self.form_input_schema = "{}".into();
        self.form_output_schema = "{}".into();
        self.error_message = None;
        self.form_error = None;
        self.form_scroll.set_offset(point(px(0.0), px(0.0)));

        self.identifier_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("唯一标识符")
                .default_value("")
        }));
        self.name_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("函数名称")
                .default_value("")
        }));
        self.description_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("描述（可选）")
                .default_value("")
        }));
        self.input_schema_input = Some(cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("{}")
                .default_value("{}")
        }));
        self.output_schema_input = Some(cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("{}")
                .default_value("{}")
        }));

        self.install_form_focus_lifecycle(window, cx);
        cx.notify();
    }

    fn show_edit_form(
        &mut self,
        window: &mut Window,
        item: RuntimeFunction,
        cx: &mut Context<Self>,
    ) {
        if item.kind == "builtin" {
            return;
        }
        self.show_form = true;
        self.editing_id = Some(item.id);
        self.form_identifier = item.identifier.clone();
        self.form_name = item.name.clone();
        self.form_description = item.description.clone().unwrap_or_default();
        self.form_kind = item.kind;
        self.form_plugin_id = item.plugin_id;
        self.form_plugin_export = item.plugin_export.clone();
        self.form_category_id = item.category_id;
        self.form_capability = item
            .required_capabilities
            .as_deref()
            .and_then(|json| serde_json::from_str::<Vec<String>>(json).ok())
            .and_then(|mut names| names.drain(..).next());
        self.kind_select_open = false;
        self.plugin_select_open = false;
        self.export_select_open = false;
        self.capability_select_open = false;
        self.category_select_open = false;
        self.refresh_plugin_exports();
        self.form_input_schema = item.input_schema.clone();
        self.form_output_schema = item.output_schema.clone();
        self.error_message = None;
        self.form_error = None;
        self.form_scroll.set_offset(point(px(0.0), px(0.0)));

        self.identifier_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("唯一标识符")
                .default_value(&item.identifier)
        }));
        self.name_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("函数名称")
                .default_value(&item.name)
        }));
        self.description_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("描述（可选）")
                .default_value(item.description.unwrap_or_default())
        }));
        self.input_schema_input = Some(cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("{}")
                .default_value(&item.input_schema)
        }));
        self.output_schema_input = Some(cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("{}")
                .default_value(&item.output_schema)
        }));

        self.install_form_focus_lifecycle(window, cx);
        cx.notify();
    }

    fn install_form_focus_lifecycle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let identifier = self
            .identifier_input
            .as_ref()
            .expect("Function identifier input initialized")
            .clone();
        let identifier_focus = identifier.read(cx).focus_handle(cx);
        cx.on_focus(&identifier_focus, window, |view, _, cx| {
            view.form_scroll.scroll_to_item(1);
            cx.notify();
        })
        .detach();
        let save_focus = self.save_focus.clone();
        cx.on_focus(&save_focus, window, |view, _, cx| {
            view.form_scroll.scroll_to_bottom();
            cx.notify();
        })
        .detach();
        cx.on_next_frame(window, move |_view, window, cx| {
            identifier.update(cx, |input, cx| input.focus(window, cx));
        });
        self.identifier_input
            .as_ref()
            .expect("Function identifier input initialized")
            .update(cx, |input, cx| input.focus(window, cx));
    }

    /// Keyboard handler for the Function modal. The gpui-component focus trap
    /// owns Tab/Shift-Tab; Escape closes, and Enter/Space activates Save only
    /// when the real Save focus handle owns focus. Textarea Enter is therefore
    /// left to the multiline editor.
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

    fn hide_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.form_scroll.set_offset(point(px(0.0), px(0.0)));
        let hidden = false;
        self.show_form = hidden;
        self.editing_id = None;
        self.error_message = None;
        self.form_error = None;
        self.kind_select_open = false;
        self.plugin_select_open = false;
        self.export_select_open = false;
        self.capability_select_open = false;
        self.category_select_open = false;
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
        if let Some(ref inp) = self.input_schema_input {
            self.form_input_schema = inp.read(cx).value().to_string();
        }
        if let Some(ref inp) = self.output_schema_input {
            self.form_output_schema = inp.read(cx).value().to_string();
        }

        let kind = self.form_kind.clone();
        let plugin_id = (kind == "custom").then_some(self.form_plugin_id).flatten();
        let plugin_export = (kind == "custom")
            .then_some(self.form_plugin_export.clone())
            .flatten();
        let mut required_capabilities = self.form_capability.as_ref().map(|name| {
            serde_json::to_string(&vec![name.clone()]).expect("serialize capability selection")
        });
        if kind == "placeholder" {
            required_capabilities = None;
        }
        let kind = match FunctionKind::try_from(kind.as_str()) {
            Ok(kind) => kind,
            Err(error) => {
                self.publish_form_error("保存失败", error, window, cx);
                return;
            }
        };
        let idf = self.form_identifier.clone();
        let name = self.form_name.clone();
        let desc = if self.form_description.is_empty() {
            None
        } else {
            Some(self.form_description.clone())
        };
        let is = self.form_input_schema.clone();
        let os = self.form_output_schema.clone();
        let input = match FunctionInput::for_write(
            idf,
            name,
            desc,
            kind,
            is,
            os,
            plugin_id,
            plugin_export,
            self.form_category_id,
            required_capabilities,
        ) {
            Ok(input) => input,
            Err(error) => {
                self.publish_form_error("保存失败", error, window, cx);
                return;
            }
        };
        let pool = self.store.read(cx).pool().clone();

        if let Some(eid) = self.editing_id {
            cx.spawn_in(window, async move |this, cx| {
                let result = match FunctionStore::new(pool) {
                    Ok(store) => store.update(eid, input).await,
                    Err(error) => Err(error),
                };
                _ = cx.update(|window, cx| {
                    _ = this.update(cx, |v, cx| match result {
                        Ok(_) => {
                            v.hide_form(window, cx);
                            v.load(cx);
                        }
                        Err(error) => {
                            let ui_error = FunctionUiError::from_public(&error);
                            v.error_message = Some(format!("更新失败: {}", ui_error.summary));
                            v.form_error = Some(ui_error);
                            v.error_focus.focus(window, cx);
                            cx.notify();
                        }
                    });
                });
            })
            .detach();
        } else {
            cx.spawn_in(window, async move |this, cx| {
                let result = match FunctionStore::new(pool) {
                    Ok(store) => store.create(input).await,
                    Err(error) => Err(error),
                };
                _ = cx.update(|window, cx| {
                    _ = this.update(cx, |v, cx| match result {
                        Ok(_) => {
                            v.hide_form(window, cx);
                            v.load(cx);
                        }
                        Err(error) => {
                            let ui_error = FunctionUiError::from_public(&error);
                            v.error_message = Some(format!("创建失败: {}", ui_error.summary));
                            v.form_error = Some(ui_error);
                            v.error_focus.focus(window, cx);
                            cx.notify();
                        }
                    });
                });
            })
            .detach();
        }
    }

    fn publish_form_error(
        &mut self,
        prefix: &str,
        error: PublicBoundaryError,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let error = FunctionUiError::from_public(&error);
        self.error_message = Some(format!("{prefix}: {}", error.summary));
        self.form_error = Some(error);
        self.error_focus.focus(window, cx);
        cx.notify();
    }

    fn delete(&mut self, id: i64, cx: &mut Context<Self>) {
        let pool = self.store.read(cx).pool().clone();
        cx.spawn(async move |this, cx| match FunctionStore::new(pool) {
            Ok(store) => match store.delete(id).await {
                Ok(()) => {
                    this.update(cx, |v, cx| v.load(cx)).ok();
                }
                Err(error) => {
                    let error = FunctionUiError::from_public(&error);
                    this.update(cx, |v, cx| {
                        v.error_message = Some(format!("删除失败: {}", error.summary));
                        cx.notify();
                    })
                    .ok();
                }
            },
            Err(error) => {
                let error = FunctionUiError::from_public(&error);
                this.update(cx, |v, cx| {
                    v.error_message = Some(format!("删除失败: {}", error.summary));
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

    fn refresh_test_capabilities(&mut self, cx: &mut Context<Self>) {
        let Some(function) = self.test_function.as_ref() else {
            return;
        };
        let function_id = function.id;
        let required_capabilities = function.required_capabilities.clone();
        let store = self.store.read(cx).clone();

        self.test_available_capabilities.clear();
        self.test_capabilities.clear();
        cx.spawn(async move |this, cx| {
            let capabilities = Capability::list(store.pool(), None, 1_000, 0).await?;
            this.update(cx, |view, cx| {
                if !view.show_test
                    || view.test_function.as_ref().map(|function| function.id) != Some(function_id)
                {
                    return;
                }
                let available = capabilities
                    .iter()
                    .map(|capability| capability.name.clone())
                    .collect::<Vec<_>>();
                view.test_capabilities = crate::runtime::resolve_test_capabilities(
                    required_capabilities.as_deref(),
                    &available,
                );
                view.test_available_capabilities = capabilities;
                cx.notify();
            })
        })
        .detach();
    }

    fn show_test_dialog(
        &mut self,
        window: &mut Window,
        function: RuntimeFunction,
        cx: &mut Context<Self>,
    ) {
        self.show_test = true;
        self.test_function = Some(function.clone());
        self.test_state = TestState::Idle;
        self.test_inputs.clear();
        self.test_capabilities.clear();
        self.test_capability_select_open = false;
        self.test_capability_scroll
            .set_offset(point(px(0.0), px(0.0)));
        self.test_capability_filter.clear();
        let capability_filter_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("过滤 Capability..."));
        cx.subscribe_in(
            &capability_filter_input,
            window,
            |view, state, event, _window, cx| {
                if let InputEvent::Change = event {
                    view.test_capability_filter = state.read(cx).value().to_string();
                    view.test_capability_scroll
                        .set_offset(point(px(0.0), px(0.0)));
                    cx.notify();
                }
            },
        )
        .detach();
        self.test_capability_filter_input = Some(capability_filter_input);
        self.test_scroll.set_offset(point(px(0.0), px(0.0)));
        self.test_input_states.clear();

        // Parse input_schema to generate form fields
        self.parsed_schema_fields.clear();
        self.is_primitive_schema = false;

        if let Ok(schema) = serde_json::from_str::<serde_json::Value>(&function.input_schema)
            && let Some(obj) = schema.as_object()
        {
            let schema_type = obj.get("type").and_then(|t| t.as_str()).unwrap_or("");

            if schema_type != "object" && !schema_type.is_empty() {
                // Primitive type schema (string, number, etc.)
                self.is_primitive_schema = true;
                let field = SchemaField {
                    key: "__value__".to_string(),
                    field_type: schema_type.to_string(),
                    description: obj
                        .get("description")
                        .and_then(|d| d.as_str())
                        .map(|s| s.to_string()),
                    enum_values: obj.get("enum").and_then(|e| e.as_array()).map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                            .collect()
                    }),
                    default_value: obj.get("default").cloned(),
                };
                self.parsed_schema_fields.push(field);
            } else if let Some(properties) = obj.get("properties").and_then(|p| p.as_object()) {
                // Object schema with properties
                for (key, prop) in properties {
                    let prop_obj = prop.as_object();
                    let field_type = prop_obj
                        .and_then(|p| p.get("type"))
                        .and_then(|t| t.as_str())
                        .unwrap_or("string")
                        .to_string();
                    let description = prop_obj
                        .and_then(|p| p.get("description"))
                        .and_then(|d| d.as_str())
                        .map(|s| s.to_string());
                    let enum_values = prop_obj
                        .and_then(|p| p.get("enum"))
                        .and_then(|e| e.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                                .collect()
                        });
                    let default_value = prop_obj.and_then(|p| p.get("default")).cloned();

                    self.parsed_schema_fields.push(SchemaField {
                        key: key.clone(),
                        field_type,
                        description,
                        enum_values,
                        default_value,
                    });
                }
            }
        }

        // Pre-create InputState for each field
        for field in &self.parsed_schema_fields {
            let input_state = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(format!(
                        "输入 {}",
                        if field.key == "__value__" {
                            "值"
                        } else {
                            &field.key
                        }
                    ))
                    .default_value("")
            });
            self.test_input_states
                .push((field.key.clone(), input_state));
        }

        self.refresh_test_capabilities(cx);
        cx.notify();
    }

    fn hide_test_dialog(&mut self, cx: &mut Context<Self>) {
        self.show_test = false;
        self.test_function = None;
        self.test_state = TestState::Idle;
        self.test_inputs.clear();
        self.test_capabilities.clear();
        self.test_available_capabilities.clear();
        self.test_capability_select_open = false;
        self.test_capability_filter.clear();
        self.test_capability_filter_input = None;
        self.test_capability_scroll
            .set_offset(point(px(0.0), px(0.0)));
        cx.notify();
    }

    fn run_test(&mut self, cx: &mut Context<Self>) {
        let function = match &self.test_function {
            Some(f) => f.clone(),
            None => return,
        };

        for (key, state) in &self.test_input_states {
            self.test_inputs
                .insert(key.clone(), state.read(cx).value().to_string());
        }

        self.test_state = TestState::Running;
        cx.notify();

        // Build input JSON from test_inputs
        let input = if self.is_primitive_schema {
            // For primitive types, get the value directly
            if let Some(val) = self.test_inputs.get("__value__") {
                serde_json::from_str(val).unwrap_or(serde_json::Value::String(val.clone()))
            } else {
                serde_json::Value::Null
            }
        } else {
            // For object types, build a JSON object
            let mut map = serde_json::Map::new();
            for field in &self.parsed_schema_fields {
                if let Some(val) = self.test_inputs.get(&field.key) {
                    if let Ok(json_val) = serde_json::from_str(val) {
                        map.insert(field.key.clone(), json_val);
                    } else {
                        map.insert(field.key.clone(), serde_json::Value::String(val.clone()));
                    }
                }
            }
            serde_json::Value::Object(map)
        };

        let plugin_root = self.store.read(cx).plugin_root().to_path_buf();
        let pool = self.store.read(cx).pool().clone();
        let executor = crate::runtime::FunctionTestExecutor::new(plugin_root, pool);
        let mut allowed_capabilities = self.test_capabilities.iter().cloned().collect::<Vec<_>>();
        allowed_capabilities.sort();

        cx.spawn(async move |this, cx| {
            let start = std::time::Instant::now();
            let result = executor
                .execute_with_capabilities(&function, input, allowed_capabilities)
                .await;

            let elapsed_ms = start.elapsed().as_millis() as i64;

            let _ = match result {
                Ok(output) => this.update(cx, |v, cx| {
                    v.test_state = TestState::Success {
                        output: crate::runtime::format_test_output(&output),
                        elapsed_ms,
                    };
                    cx.notify();
                }),
                Err(message) => this.update(cx, |v, cx| {
                    v.test_state = TestState::Error { message };
                    cx.notify();
                }),
            };
        })
        .detach();
    }
}

impl FunctionUiError {
    fn from_public(error: &PublicBoundaryError) -> Self {
        match error.envelope() {
            PublicErrorEnvelope::InvalidInput { field, reason } => {
                let summary = format!("field={field} reason={reason}");
                Self {
                    semantic: FunctionSemanticStatus::FormError {
                        label: summary.clone(),
                    },
                    summary,
                }
            }
            PublicErrorEnvelope::Conflict {
                shape,
                field,
                reason,
            } => {
                if shape == "value" {
                    let value = error.value().unwrap_or_default();
                    let summary = format!("field={field} reason={reason} value={value}");
                    let selector = if field == "identifier" && !value.is_empty() {
                        FunctionSemanticStatus::IdentifierConflict {
                            value: value.to_string(),
                            field: field.to_string(),
                            reason: reason.to_string(),
                        }
                    } else {
                        FunctionSemanticStatus::FormError {
                            label: summary.clone(),
                        }
                    };
                    Self {
                        semantic: selector,
                        summary,
                    }
                } else {
                    let references = error.references().join(",");
                    let summary = format!("field={field} reason={reason} references={references}");
                    Self {
                        semantic: FunctionSemanticStatus::FormError {
                            label: summary.clone(),
                        },
                        summary,
                    }
                }
            }
            PublicErrorEnvelope::NotFound => Self {
                summary: "reason=not_found".to_string(),
                semantic: FunctionSemanticStatus::FormError {
                    label: "reason=not_found".to_string(),
                },
            },
            PublicErrorEnvelope::Forbidden => Self {
                summary: "reason=forbidden".to_string(),
                semantic: FunctionSemanticStatus::FormError {
                    label: "reason=forbidden".to_string(),
                },
            },
            PublicErrorEnvelope::Internal { reason } => Self {
                summary: format!("reason={reason}"),
                semantic: FunctionSemanticStatus::FormError {
                    label: format!("reason={reason}"),
                },
            },
        }
    }
}

impl Render for FunctionView {
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
                    .placeholder("函数名称")
                    .default_value(&self.form_name)
            }));
            self.description_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("描述（可选）")
                    .default_value(&self.form_description)
            }));
            self.input_schema_input = Some(cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder("{}")
                    .default_value(&self.form_input_schema)
            }));
            self.output_schema_input = Some(cx.new(|cx| {
                TextareaState::new(window, cx)
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
                cx.subscribe_in(input, window, |this, state, event, _window, cx| {
                    if let InputEvent::Change = event {
                        this.search_text = state.read(cx).value().to_string();
                        this.current_page = 1;
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
            .when(self.loading, |view| {
                view.child(focus_marker("FUNCTION_LIST_LOADING"))
            })
            .when(!self.loading, |view| {
                view.child(focus_marker("FUNCTION_LIST_LOADED"))
            })
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
                            .child("函数管理"),
                    )
                    .child(
                        action_button(
                            "add-btn",
                            "+ 添加函数",
                            ActionRole::Main,
                            ActionSize::Page,
                            style,
                        )
                        .debug_selector(|| "FUNCTION_ADD".to_string())
                        .role(Role::Button)
                        .aria_label("添加函数")
                        .track_focus(&self.add_focus)
                        .tab_index(0)
                        .on_key_down(cx.listener(
                            |view, event: &KeyDownEvent, window, cx| {
                                if matches!(event.keystroke.key.as_str(), "enter" | " " | "space")
                                {
                                    cx.stop_propagation();
                                    view.show_add_form(window, cx);
                                }
                            },
                        ))
                        .on_click({
                            let t = cx.weak_entity();
                            move |_, window, cx| {
                                t.update(cx, |v, cx| v.show_add_form(window, cx)).ok();
                            }
                        })
                        .when(self.add_focus.is_focused(window), |button| {
                            button.child(focus_marker("FUNCTION_ADD_FOCUSED"))
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
                            .text_color(theme.muted_foreground)
                            .text_size(px(14.0))
                            .child("暂无数据")
                    } else {
                        let col_widths = [px(60.0), px(120.0), px(100.0), px(80.0), px(120.0)];
                        list_container(style)
                            .child(
                                list_header(style)
                                    .child(list_header_cell(Some(col_widths[0]), style).child("ID"))
                                    .child(list_header_cell(Some(col_widths[1]), style).child("名称"))
                                    .child(list_header_cell(Some(col_widths[2]), style).child("Identifier"))
                                    .child(list_header_cell(Some(col_widths[3]), style).child("Kind"))
                                    .child(list_header_cell(Some(col_widths[4]), style).child("操作")),
                            )
                            .children(self.items.iter().map(|item| {
                                let id = item.id;
                                let ic = item.clone();
                                let identifier = item.identifier.clone();
                                list_row(style)
                                    .child(list_cell(Some(col_widths[0]), style).child(format!("{}", item.id)))
                                    .child(
                                        list_cell(Some(col_widths[1]), style)
                                            .text_size(px(13.0))
                                            .font_weight(FontWeight::MEDIUM)
                                            .child(item.name.clone()),
                                    )
                                    .child(
                                        list_cell(Some(col_widths[2]), style)
                                            .truncate()
                                            .child(item.identifier.clone()),
                                    )
                                    .child(
                                        list_cell(Some(col_widths[3]), style)
                                            .flex()
                                            .items_center()
                                            .gap(px(4.0))
                                            .child(function_kind_label(&item.kind))
                                            .when(item.kind == "builtin", |cell| {
                                                cell.child(
                                                    function_semantic_element(
                                                        FunctionSemanticStatus::BuiltinImmutable {
                                                            id,
                                                            identifier: identifier.clone(),
                                                        },
                                                    )
                                                        .text_size(px(10.0))
                                                        .text_color(theme.muted_foreground)
                                                        .child("只读"),
                                                )
                                            })
                                            .when(item.kind == "placeholder", |cell| {
                                                cell.child(
                                                    function_semantic_element(
                                                        FunctionSemanticStatus::PlaceholderNonExecutable {
                                                            id,
                                                            identifier: identifier.clone(),
                                                        },
                                                    )
                                                        .text_size(px(10.0))
                                                        .text_color(theme.muted_foreground)
                                                        .child("不可执行"),
                                                )
                                            }),
                                    )
                                    .child(
                                        list_actions(Some(col_widths[4]), style)
                                            .when(ic.kind != "placeholder", |actions| actions.child(
                                                action_button(
                                                    ("test", id as u64),
                                                    "测试",
                                                    ActionRole::Main,
                                                    ActionSize::Row,
                                                    style,
                                                )
                                                .debug_selector(move || format!("FUNCTION_TEST-{id}"))
                                                .role(Role::Button)
                                                .aria_label(format!("测试 {}", ic.identifier))
                                                .on_mouse_down(MouseButton::Left, {
                                                    let t = cx.weak_entity();
                                                    let ic_test = ic.clone();
                                                    move |_, window, cx| {
                                                        t.update(cx, |v, cx| {
                                                            v.show_test_dialog(window, ic_test.clone(), cx)
                                                        })
                                                        .ok();
                                                    }
                                                }),
                                            ))
                                            .when(ic.kind != "builtin", |actions| actions
                                                .child(
                                                    action_button(
                                                        ("edit", id as u64),
                                                        "编辑",
                                                        ActionRole::Edit,
                                                        ActionSize::Row,
                                                        style,
                                                    )
                                                    .debug_selector(move || format!("FUNCTION_EDIT-{id}"))
                                                    .role(Role::Button)
                                                    .aria_label(format!("编辑 {}", ic.identifier))
                                                    .on_mouse_down(MouseButton::Left, {
                                                        let t = cx.weak_entity();
                                                        let ic_edit = ic.clone();
                                                        move |_, window, cx| {
                                                            t.update(cx, |v, cx| {
                                                                v.show_edit_form(window, ic_edit.clone(), cx)
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
                                                    .debug_selector(move || format!("FUNCTION_DELETE-{id}"))
                                                    .role(Role::Button)
                                                    .aria_label(format!("删除 {}", ic.identifier))
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
                            .text_color(theme.muted_foreground)
                            .child(format!("共 {} 条", self.total_count)),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(8.0))
                            .child(
                                action_button(
                                    "prev",
                                    "上一页",
                                    if self.current_page > 1 {
                                        ActionRole::Main
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
                            .child(div().text_size(px(13.0)).child(format!(
                                "第 {} / {} 页",
                                self.current_page,
                                tp.max(1)
                            )))
                            .child(
                                action_button(
                                    "next",
                                    "下一页",
                                    if self.current_page * self.page_size < self.total_count {
                                        ActionRole::Main
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
                let input_schema_input = self.input_schema_input.clone().unwrap();
                let output_schema_input = self.output_schema_input.clone().unwrap();
                let identifier_focused = identifier_input
                    .read(cx)
                    .focus_handle(cx)
                    .is_focused(window);
                let input_schema_focused = input_schema_input
                    .read(cx)
                    .focus_handle(cx)
                    .is_focused(window);
                let current_identifier = identifier_input.read(cx).value().to_string();
                let kind_label = function_kind_label(&self.form_kind);
                let plugin_label = self
                    .form_plugin_id
                    .and_then(|id| self.plugins.iter().find(|plugin| plugin.id == id))
                    .map(|plugin| format!("{} ({})", plugin.name, plugin.version))
                    .unwrap_or_else(|| "请选择插件".into());
                let export_label = self
                    .form_plugin_export
                    .clone()
                    .unwrap_or_else(|| "请选择导出函数".into());
                let capability_label = self
                    .form_capability
                    .clone()
                    .unwrap_or_else(|| "无".into());
                let category_label = self
                    .form_category_id
                    .and_then(|id| self.categories.iter().find(|category| category.id == id))
                    .map(|category| category.name.clone())
                    .unwrap_or_else(|| "无".into());

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
                        .debug_selector(|| "FUNCTION_MODAL".to_string())
                        .track_focus(&self.form_focus)
                        .focus_trap("function-form-focus-trap", &self.form_focus)
                        .key_context("HiveguiFunctionForm")
                        .on_action(cx.listener(
                            |view, _: &FunctionFormTab, window, cx| {
                                view.focus_form_next(window, cx);
                            },
                        ))
                        .on_action(cx.listener(
                            |view, _: &FunctionFormTabPrev, window, cx| {
                                view.focus_form_prev(window, cx);
                            },
                        ))
                        .capture_key_down(cx.listener(|v, event: &KeyDownEvent, window, cx| {
                            v.on_key_down(event, window, cx);
                        }))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                            cx.stop_propagation();
                        })
                        .child(
                            management_modal_scroll("function-form-scroll", &self.form_scroll)
                                .debug_selector(|| "FUNCTION_FORM_SCROLL".to_string())
                                .gap(px(12.0))
                                .child(
                                    div()
                                        .text_size(px(18.0))
                                        .font_weight(FontWeight::BOLD)
                                        .child(if self.editing_id.is_some() {
                                            "编辑函数"
                                        } else {
                                            "添加函数"
                                        }),
                                )
                                .child(form_field(
                                    "Identifier *",
                                    identifier_input,
                                    "FUNCTION_IDENTIFIER",
                                    identifier_focused.then_some("FUNCTION_IDENTIFIER_FOCUSED"),
                                    Some(format!(
                                        "FUNCTION_IDENTIFIER_VALUE-{current_identifier}"
                                    )),
                                    theme,
                                ))
                                .child(form_field(
                                    "名称 *",
                                    name_input,
                                    "FUNCTION_NAME",
                                    None,
                                    None,
                                    theme,
                                ))
                                .child(form_field(
                                    "描述",
                                    description_input,
                                    "FUNCTION_DESCRIPTION",
                                    None,
                                    None,
                                    theme,
                                ))
                                .child(
                                    selector_field(
                                        "Kind *",
                                        kind_label,
                                        "kind-selector",
                                        theme,
                                        {
                                            let view = cx.weak_entity();
                                            move |_, _, cx| {
                                                view.update(cx, |view, cx| {
                                                    view.kind_select_open =
                                                        !view.kind_select_open;
                                                    view.plugin_select_open = false;
                                                    view.export_select_open = false;
                                                    view.capability_select_open = false;
                                                    cx.notify();
                                                })
                                                .ok();
                                            }
                                        },
                                    )
                                    .id("function-kind-selector-a11y")
                                    .debug_selector(|| "FUNCTION_KIND_SELECTOR".to_string())
                                    .track_focus(&self.kind_focus)
                                    .tab_index(0)
                                    .role(Role::Button)
                                    .aria_label(format!("Kind {kind_label}"))
                                    .on_key_down(cx.listener(
                                        |view, event: &KeyDownEvent, _window, cx| {
                                            if matches!(
                                                event.keystroke.key.as_str(),
                                                "enter" | " " | "space"
                                            ) {
                                                cx.stop_propagation();
                                                view.kind_select_open = !view.kind_select_open;
                                                view.plugin_select_open = false;
                                                view.export_select_open = false;
                                                view.capability_select_open = false;
                                                view.category_select_open = false;
                                                cx.notify();
                                            }
                                        },
                                    ))
                                    .when(self.kind_select_open, |field| {
                                        field.child(selector_menu(theme).children(
                                            [
                                                ("custom", "自定义函数"),
                                                ("placeholder", "占位"),
                                            ]
                                                .into_iter()
                                                .map(|(kind, label)| {
                                                    selector_option(
                                                        format!("kind-option-{kind}"),
                                                        label,
                                                        self.form_kind == kind,
                                                        theme,
                                                        {
                                                            let view = cx.weak_entity();
                                                            move |_, _, cx| {
                                                                view.update(cx, |view, cx| {
                                                                    view.form_kind = kind.to_string();
                                                                    view.kind_select_open = false;
                                                                    if kind != "custom" {
                                                                        view.form_plugin_id = None;
                                                                        view.form_plugin_export = None;
                                                                        view.plugin_exports.clear();
                                                                    }
                                                                    if kind == "placeholder" {
                                                                        view.form_capability = None;
                                                                    }
                                                                    cx.notify();
                                                                })
                                                                .ok();
                                                            }
                                                        },
                                                    )
                                                    .debug_selector(move || {
                                                        format!("FUNCTION_KIND_OPTION-{kind}")
                                                    })
                                                    .role(Role::Button)
                                                    .aria_label(label)
                                                }),
                                        ))
                                    }),
                                )
                                .when(self.form_kind == "custom", |form| {
                                    form.child(
                                        selector_field(
                                            "关联插件 *",
                                            plugin_label,
                                            "plugin-selector",
                                            theme,
                                            {
                                                let view = cx.weak_entity();
                                                move |_, _window, cx| {
                                                    view.update(cx, |view, cx| {
                                                        view.plugin_select_open =
                                                            !view.plugin_select_open;
                                                        view.kind_select_open = false;
                                                        view.export_select_open = false;
                                                        view.capability_select_open = false;
                                                        cx.notify();
                                                    })
                                                    .ok();
                                                }
                                            },
                                        )
                                        .id("function-plugin-selector-a11y")
                                        .debug_selector(|| {
                                            "FUNCTION_PLUGIN_SELECTOR".to_string()
                                        })
                                        .track_focus(&self.plugin_focus)
                                        .tab_index(0)
                                        .role(Role::Button)
                                        .aria_label("关联插件")
                                        .when(self.plugin_select_open, |field| {
                                            field.child(selector_menu(theme).children(
                                                self.plugins.iter().map(|plugin| {
                                                    let plugin_id = plugin.id;
                                                    let label = format!(
                                                        "{} ({})",
                                                        plugin.name, plugin.version
                                                    );
                                                    selector_option(
                                                        format!("plugin-option-{plugin_id}"),
                                                        label,
                                                        self.form_plugin_id == Some(plugin_id),
                                                        theme,
                                                        {
                                                            let view = cx.weak_entity();
                                                            move |_, _, cx| {
                                                                view.update(cx, |view, cx| {
                                                                    view.form_plugin_id =
                                                                        Some(plugin_id);
                                                                    view.form_plugin_export = None;
                                                                    view.plugin_select_open = false;
                                                                    view.refresh_plugin_exports();
                                                                    if view.plugin_exports.is_empty()
                                                                    {
                                                                        view.error_message = Some(
                                                                            "该插件没有导出函数元数据，请重新上传 WASM"
                                                                                .into(),
                                                                        );
                                                                    } else {
                                                                        view.error_message = None;
                                                                    }
                                                                    cx.notify();
                                                                })
                                                                .ok();
                                                            }
                                                        },
                                                    )
                                                }),
                                            ))
                                        }),
                                    )
                                    .child(
                                        selector_field(
                                            "插件导出函数名 *",
                                            export_label,
                                            "export-selector",
                                            theme,
                                            {
                                                let view = cx.weak_entity();
                                                move |_, _window, cx| {
                                                    view.update(cx, |view, cx| {
                                                        view.export_select_open =
                                                            !view.export_select_open;
                                                        view.kind_select_open = false;
                                                        view.plugin_select_open = false;
                                                        view.capability_select_open = false;
                                                        cx.notify();
                                                    })
                                                    .ok();
                                                }
                                            },
                                        )
                                        .id("function-export-selector-a11y")
                                        .debug_selector(|| {
                                            "FUNCTION_EXPORT_SELECTOR".to_string()
                                        })
                                        .track_focus(&self.export_focus)
                                        .tab_index(0)
                                        .role(Role::Button)
                                        .aria_label("插件导出函数名")
                                        .when(self.export_select_open, |field| {
                                            field.child(selector_menu(theme).children(
                                                self.plugin_exports.iter().map(|name| {
                                                    let export = name.clone();
                                                    selector_option(
                                                        format!("export-option-{name}"),
                                                        name.clone(),
                                                        self.form_plugin_export.as_ref()
                                                            == Some(name),
                                                        theme,
                                                        {
                                                            let view = cx.weak_entity();
                                                            move |_, _, cx| {
                                                                view.update(cx, |view, cx| {
                                                                    view.form_plugin_export =
                                                                        Some(export.clone());
                                                                    view.export_select_open = false;
                                                                    view.error_message = None;
                                                                    cx.notify();
                                                                })
                                                                .ok();
                                                            }
                                                        },
                                                    )
                                                }),
                                            ))
                                        }),
                                    )
                                })
                                .when(self.form_kind != "placeholder", |form| {
                                    form.child(selector_field(
                                        "所属 Capability",
                                        capability_label,
                                        "capability-selector",
                                        theme,
                                        {
                                            let view = cx.weak_entity();
                                            move |_, _, cx| {
                                                view.update(cx, |view, cx| {
                                                    view.capability_select_open =
                                                        !view.capability_select_open;
                                                    view.kind_select_open = false;
                                                    view.plugin_select_open = false;
                                                    view.export_select_open = false;
                                                    cx.notify();
                                                })
                                                .ok();
                                            }
                                        },
                                    )
                                    .id("function-capability-selector-a11y")
                                    .debug_selector(|| {
                                        "FUNCTION_CAPABILITY_SELECTOR".to_string()
                                    })
                                    .track_focus(&self.capability_focus)
                                    .tab_index(0)
                                    .role(Role::Button)
                                    .aria_label("所属 Capability")
                                    .when(self.capability_select_open, |field| {
                                        field.child(
                                            selector_menu(theme)
                                                .child(selector_option(
                                                    "capability-option-none",
                                                    "无",
                                                    self.form_capability.is_none(),
                                                    theme,
                                                    {
                                                        let view = cx.weak_entity();
                                                        move |_, _, cx| {
                                                            view.update(cx, |view, cx| {
                                                                view.form_capability = None;
                                                                view.capability_select_open = false;
                                                                cx.notify();
                                                            })
                                                            .ok();
                                                        }
                                                    },
                                                ))
                                                .children(self.capabilities.iter().map(
                                                    |capability| {
                                                        let name = capability.name.clone();
                                                        selector_option(
                                                            format!(
                                                                "capability-option-{}",
                                                                capability.name
                                                            ),
                                                            capability.name.clone(),
                                                            self.form_capability.as_ref()
                                                                == Some(&capability.name),
                                                            theme,
                                                            {
                                                                let view = cx.weak_entity();
                                                                move |_, _, cx| {
                                                                    view.update(
                                                                        cx,
                                                                        |view, cx| {
                                                                            view.form_capability =
                                                                                Some(name.clone());
                                                                            view.capability_select_open = false;
                                                                            cx.notify();
                                                                        },
                                                                    )
                                                                    .ok();
                                                                }
                                                            },
                                                        )
                                                    },
                                                )),
                                        )
                                    }))
                                })
                                .child(
                                    selector_field(
                                        "所属 Category",
                                        category_label,
                                        "category-selector",
                                        theme,
                                        {
                                            let view = cx.weak_entity();
                                            move |_, _, cx| {
                                                view.update(cx, |view, cx| {
                                                    view.category_select_open =
                                                        !view.category_select_open;
                                                    view.kind_select_open = false;
                                                    view.plugin_select_open = false;
                                                    view.export_select_open = false;
                                                    view.capability_select_open = false;
                                                    cx.notify();
                                                })
                                                .ok();
                                            }
                                        },
                                    )
                                    .id("function-category-selector-a11y")
                                    .debug_selector(|| {
                                        "FUNCTION_CATEGORY_SELECTOR".to_string()
                                    })
                                    .track_focus(&self.category_focus)
                                    .tab_index(0)
                                    .role(Role::Button)
                                    .aria_label("所属 Category")
                                    .when(self.category_select_open, |field| {
                                        field.child(
                                            selector_menu(theme)
                                                .child(selector_option(
                                                    "category-option-none",
                                                    "无",
                                                    self.form_category_id.is_none(),
                                                    theme,
                                                    {
                                                        let view = cx.weak_entity();
                                                        move |_, _, cx| {
                                                            view.update(cx, |view, cx| {
                                                                view.form_category_id = None;
                                                                view.category_select_open = false;
                                                                cx.notify();
                                                            })
                                                            .ok();
                                                        }
                                                    },
                                                ))
                                                .children(self.categories.iter().map(|category| {
                                                    let category_id = category.id;
                                                    selector_option(
                                                        format!(
                                                            "category-option-{category_id}"
                                                        ),
                                                        category.name.clone(),
                                                        self.form_category_id
                                                            == Some(category_id),
                                                        theme,
                                                        {
                                                            let view = cx.weak_entity();
                                                            move |_, _, cx| {
                                                                view.update(cx, |view, cx| {
                                                                    view.form_category_id =
                                                                        Some(category_id);
                                                                    view.category_select_open =
                                                                        false;
                                                                    cx.notify();
                                                                })
                                                                .ok();
                                                            }
                                                        },
                                                    )
                                                })),
                                        )
                                    }),
                                )
                                .when(self.form_kind == "placeholder", |form| {
                                    form.child(
                                        function_semantic_element(
                                            FunctionSemanticStatus::PlaceholderSchemaOnly,
                                        )
                                            .p(px(8.0))
                                            .rounded(px(4.0))
                                            .bg(theme.muted)
                                            .text_size(px(12.0))
                                            .text_color(theme.muted_foreground)
                                            .child("占位函数仅保存 Schema，不可执行"),
                                    )
                                })
                                .child(form_field_multiline(
                                    "Input Schema (JSON)",
                                    input_schema_input,
                                    "FUNCTION_INPUT_SCHEMA",
                                    input_schema_focused
                                        .then_some("FUNCTION_INPUT_SCHEMA_FOCUSED"),
                                    theme,
                                ))
                                .child(form_field_multiline(
                                    "Output Schema (JSON)",
                                    output_schema_input,
                                    "FUNCTION_OUTPUT_SCHEMA",
                                    None,
                                    theme,
                                ))
                                .when_some(self.error_message.as_ref(), |this, err| {
                                    let semantic = self
                                        .form_error
                                        .as_ref()
                                        .map(|error| error.semantic.clone())
                                        .unwrap_or_else(|| FunctionSemanticStatus::FormError {
                                            label: "reason=validation".to_string(),
                                        });
                                    this.child(
                                        function_semantic_element(semantic)
                                            .track_focus(&self.error_focus)
                                            .tab_index(0)
                                            .p(px(8.0))
                                            .bg(theme.warning.opacity(0.15))
                                            .rounded(px(4.0))
                                            .text_size(px(12.0))
                                            .text_color(theme.warning)
                                            .child(err.clone())
                                            .when(self.error_focus.is_focused(window), |error| {
                                                error.child(focus_marker(
                                                    "FUNCTION_FORM_ERROR_FOCUSED",
                                                ))
                                            }),
                                    )
                                })
                                .child(
                                    div()
                                        .debug_selector(|| {
                                            "FUNCTION_FORM_ACTIONS".to_string()
                                        })
                                        .flex()
                                        .justify_end()
                                        .gap(px(8.0))
                                        .child(
                                            action_button(
                                                "cancel",
                                                "取消",
                                                ActionRole::Neutral,
                                                ActionSize::Dialog,
                                                style,
                                            )
                                            .debug_selector(|| {
                                                "FUNCTION_FORM_CANCEL".to_string()
                                            })
                                            .track_focus(&self.cancel_focus)
                                            .tab_index(0)
                                            .role(Role::Button)
                                            .aria_label("取消")
                                            .on_key_down(cx.listener(
                                                |view, event: &KeyDownEvent, window, cx| {
                                                    if matches!(
                                                        event.keystroke.key.as_str(),
                                                        "enter" | " " | "space"
                                                    ) {
                                                        cx.stop_propagation();
                                                        view.hide_form(window, cx);
                                                    }
                                                },
                                            ))
                                            .on_mouse_down(MouseButton::Left, {
                                                let t = cx.weak_entity();
                                                move |_, window, cx| {
                                                    t.update(cx, |v, cx| {
                                                        v.hide_form(window, cx)
                                                    })
                                                    .ok();
                                                }
                                            }),
                                        )
                                        .child(
                                            action_button(
                                                "save",
                                                "保存",
                                                ActionRole::Main,
                                                ActionSize::Dialog,
                                                style,
                                            )
                                            .debug_selector(|| {
                                                "FUNCTION_FORM_SAVE".to_string()
                                            })
                                            .track_focus(&self.save_focus)
                                            .tab_index(0)
                                            .role(Role::Button)
                                            .aria_label("保存")
                                            .on_mouse_down(MouseButton::Left, {
                                                let t = cx.weak_entity();
                                                move |_, window, cx| {
                                                    t.update(cx, |v, cx| v.save(window, cx)).ok();
                                                }
                                            })
                                            .when(self.save_focus.is_focused(window), |button| {
                                                button.child(focus_marker(
                                                    "FUNCTION_FORM_SAVE_FOCUSED",
                                                ))
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
                                                .text_color(theme.muted_foreground)
                                                .child("确定要删除这个函数吗？此操作不可恢复。"),
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
                                                        ActionSize::Dialog,
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
            .when(self.show_test, |this| {
                let function = match &self.test_function {
                    Some(f) => f.clone(),
                    None => return this,
                };

                let kind_label = function_kind_label(&function.kind);
                let mut selected_capabilities = self
                    .test_capabilities
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>();
                selected_capabilities.sort();
                let capability_summary = if selected_capabilities.is_empty() {
                    if self.test_available_capabilities.is_empty() {
                        "暂无可用 Capability".to_string()
                    } else {
                        "请选择 Capability".to_string()
                    }
                } else {
                    selected_capabilities.join(", ")
                };

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
                                t.update(cx, |v, cx| v.hide_test_dialog(cx)).ok();
                            }
                        }),
                )
                .child(
                    management_modal_panel(
                        management_modal_layer(px(700.0), window.bounds().size.height - px(48.0)),
                        theme.popover,
                        theme.foreground,
                        theme.border,
                    )
                    .debug_selector(|| "FUNCTION_TEST_DIALOG".to_string())
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .child(
                        management_modal_scroll("test-dialog-scroll", &self.test_scroll)
                            .gap(px(12.0))
                            .child(
                                div()
                                    .text_size(px(18.0))
                                    .font_weight(FontWeight::BOLD)
                                    .child(format!("测试 Function「{}」", function.identifier)),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(8.0))
                                    .child(
                                        div()
                                            .text_size(px(13.0))
                                            .child(format!("函数信息：{} | kind: {}", function.name, kind_label)),
                                    )
                                    .when_some(function.description.as_ref(), |this, desc| {
                                        this.child(
                                            div()
                                                .text_size(px(12.0))
                                                .text_color(theme.muted_foreground)
                                                .child(desc.clone()),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(8.0))
                                    .child(
                                        div()
                                            .text_size(px(13.0))
                                            .font_weight(FontWeight::MEDIUM)
                                            .child("输入参数："),
                                    )
                                    .when(self.parsed_schema_fields.is_empty(), |this| {
                                        this.child(
                                            div()
                                                .text_size(px(12.0))
                                                .text_color(theme.muted_foreground)
                                                .child("该函数无输入参数（空 schema）"),
                                        )
                                    })
                                    .children(self.parsed_schema_fields.iter().map(|field| {
                                        let key = field.key.clone();
                                        let description = field.description.clone();
                                        let enum_values = field.enum_values.clone();
                                        let current_value = self.test_inputs.get(&key).cloned().unwrap_or_default();

                                        let label = if key == "__value__" {
                                            "值".to_string()
                                        } else if let Some(desc) = description {
                                            format!("{} ({})", key, desc)
                                        } else {
                                            key.clone()
                                        };

                                        let field_div = div()
                                            .flex()
                                            .flex_col()
                                            .gap(px(4.0))
                                            .child(
                                                div()
                                                    .text_size(px(13.0))
                                                    .child(label),
                                            );

                                        if let Some(enums) = enum_values {
                                            // Enum selector - cycle through options on click
                                            let selected_label = if current_value.is_empty() {
                                                format!("选择 {}", if key == "__value__" { "值" } else { &key })
                                            } else {
                                                current_value.clone()
                                            };

                                            let key_clone = key.clone();
                                            let key_hash = key.chars().map(|c| c as u64).sum::<u64>();
                                            let input_selector = format!("FUNCTION_TEST_INPUT-{key}");
                                            field_div.child(
                                                div()
                                                    .id(("test-enum", key_hash))
                                                    .debug_selector(move || input_selector.clone())
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
                                                    .child(selected_label)
                                                    .child("⌄")
                                                    .on_mouse_down(MouseButton::Left, {
                                                        let t = cx.weak_entity();
                                                        move |_, _, cx| {
                                                            t.update(cx, |v, cx| {
                                                                let current_idx = enums.iter().position(|e| e == &v.test_inputs.get(&key_clone).cloned().unwrap_or_default());
                                                                let next_idx = current_idx.map(|i| (i + 1) % enums.len()).unwrap_or(0);
                                                                if let Some(val) = enums.get(next_idx) {
                                                                    v.test_inputs.insert(key_clone.clone(), val.clone());
                                                                }
                                                                cx.notify();
                                                            }).ok();
                                                        }
                                                    })
                                            )
                                        } else {
                                            // Text input - use pre-created InputState
                                            if let Some((_, input_state)) = self.test_input_states.iter().find(|(k, _)| k == &key) {
                                                let input_selector = format!("FUNCTION_TEST_INPUT-{key}");
                                                field_div.child(
                                                    div()
                                                        .debug_selector(move || input_selector.clone())
                                                        .child(
                                                            Input::new(input_state)
                                                                .aria_label(format!("Function test input {key}"))
                                                                .w_full()
                                                                .h(px(32.0))
                                                                .px(px(8.0))
                                                                .border_1()
                                                                .border_color(theme.border)
                                                                .rounded(px(4.0)),
                                                        )
                                                )
                                            } else {
                                                field_div
                                            }
                                        }
                                    })),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(4.0))
                                    .child(
                                        selector_field(
                                            "允许的 Capabilities",
                                            capability_summary,
                                            "test-capability-selector",
                                            theme,
                                            {
                                                let view = cx.weak_entity();
                                                move |_, _, cx| {
                                                    view.update(cx, |view, cx| {
                                                        view.test_capability_select_open =
                                                            !view.test_capability_select_open;
                                                        view.test_capability_scroll
                                                            .set_offset(point(px(0.0), px(0.0)));
                                                        cx.notify();
                                                    })
                                                    .ok();
                                                }
                                            },
                                        )
                                        .id("function-test-capability-selector-a11y")
                                        .debug_selector(|| {
                                            "FUNCTION_TEST_CAPABILITY_SELECTOR".to_string()
                                        })
                                        .track_focus(&self.test_capability_focus)
                                        .tab_index(0)
                                        .role(Role::Button)
                                        .aria_label("允许的 Capabilities")
                                        .when(self.test_capability_select_open, |field| {
                                            field.child(
                                                div()
                                                    .id("test-capability-options")
                                                    .flex()
                                                    .flex_col()
                                                    .max_h(px(200.0))
                                                    .pr(px(12.0))
                                                    .overflow_y_scroll()
                                                    .track_scroll(&self.test_capability_scroll)
                                                    .vertical_scrollbar(
                                                        &self.test_capability_scroll,
                                                    )
                                                    .border_1()
                                                    .border_color(theme.border)
                                                    .rounded(px(4.0))
                                                    .bg(theme.popover)
                                                    .text_color(theme.popover_foreground)
                                                    .when_some(
                                                        self.test_capability_filter_input.as_ref(),
                                                        |menu, input| {
                                                            menu.child(
                                                                div().p(px(4.0)).child(
                                                                    Input::new(input)
                                                                        .w_full()
                                                                        .h(px(32.0))
                                                                        .px(px(8.0))
                                                                        .border_1()
                                                                        .border_color(theme.border)
                                                                        .rounded(px(4.0)),
                                                                ),
                                                            )
                                                        },
                                                    )
                                                    .children(
                                                        self.test_available_capabilities
                                                            .iter()
                                                            .filter(|capability| {
                                                                crate::runtime::capability_matches_filter(
                                                                    &capability.name,
                                                                    &capability.description,
                                                                    &self.test_capability_filter,
                                                                )
                                                            })
                                                            .map(|capability| {
                                                    let name = capability.name.clone();
                                                    let selected =
                                                        self.test_capabilities.contains(&name);
                                                    let label = format!(
                                                        "{}{}{}",
                                                        if selected { "✓ " } else { "" },
                                                        capability.name,
                                                        if capability.is_dangerous {
                                                            "（危险）"
                                                        } else {
                                                            ""
                                                        }
                                                    );

                                                    selector_option(
                                                        format!(
                                                            "test-capability-option-{}",
                                                            capability.name
                                                        ),
                                                        label,
                                                        selected,
                                                        theme,
                                                        {
                                                            let view = cx.weak_entity();
                                                            move |_, _, cx| {
                                                                view.update(cx, |view, cx| {
                                                                    if !view
                                                                        .test_capabilities
                                                                        .remove(&name)
                                                                    {
                                                                        view.test_capabilities
                                                                            .insert(name.clone());
                                                                    }
                                                                    view.test_state =
                                                                        TestState::Idle;
                                                                    cx.notify();
                                                                })
                                                                .ok();
                                                            }
                                                        },
                                                    )
                                                }),
                                                    ),
                                            )
                                        }),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(12.0))
                                            .text_color(theme.muted_foreground)
                                            .child("仅用于本次测试，不会修改函数配置"),
                                    ),
                            )
                            .child(
                                match &self.test_state {
                                    TestState::Idle => div()
                                        .id("function-test-idle")
                                        .flex()
                                        .flex_col()
                                        .gap(px(8.0)),
                                    TestState::Running => {
                                        div()
                                            .id("function-test-running")
                                            .flex()
                                            .flex_col()
                                            .gap(px(8.0))
                                            .child(
                                                div()
                                                    .text_size(px(13.0))
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .child("执行结果"),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(12.0))
                                                    .text_color(theme.muted_foreground)
                                                    .child("执行中..."),
                                            )
                                    }
                                    TestState::Success { output, elapsed_ms } => {
                                        function_semantic_element(
                                            FunctionSemanticStatus::TestSuccess,
                                        )
                                            .flex()
                                            .flex_col()
                                            .gap(px(8.0))
                                            .child(
                                                div()
                                                    .text_size(px(13.0))
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .child("执行结果"),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(12.0))
                                                    .text_color(theme.success)
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .child(format!("耗时：{}ms", elapsed_ms)),
                                            )
                                            .child(
                                                div()
                                                    .w_full()
                                                    .p(px(12.0))
                                                    .bg(theme.background.opacity(0.5))
                                                    .border_1()
                                                    .border_color(theme.border)
                                                    .rounded(px(4.0))
                                                    .text_size(px(12.0))
                                                    .font_family("monospace")
                                                    .child(output.clone()),
                                            )
                                    }
                                    TestState::Error { message } => {
                                        function_semantic_element(FunctionSemanticStatus::TestError)
                                            .flex()
                                            .flex_col()
                                            .gap(px(8.0))
                                            .child(
                                                div()
                                                    .text_size(px(13.0))
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .child("执行结果"),
                                            )
                                            .child(
                                                div()
                                                    .w_full()
                                                    .p(px(12.0))
                                                    .bg(theme.warning.opacity(0.15))
                                                    .border_1()
                                                    .border_color(theme.warning)
                                                    .rounded(px(4.0))
                                                    .text_size(px(12.0))
                                                    .text_color(theme.warning)
                                                    .child(format!("错误：{}", message)),
                                            )
                                    }
                                }
                            )
                            .child(
                                div()
                                    .flex()
                                    .justify_end()
                                    .gap(px(8.0))
                                    .child(
                                        action_button(
                                            "reset-test",
                                            "重置",
                                            ActionRole::Neutral,
                                            ActionSize::Dialog,
                                            style,
                                        )
                                        .on_mouse_down(MouseButton::Left, {
                                            let t = cx.weak_entity();
                                            move |_, _, cx| {
                                                t.update(cx, |v, cx| {
                                                    v.test_inputs.clear();
                                                    let required_capabilities = v
                                                        .test_function
                                                        .as_ref()
                                                        .and_then(|function| {
                                                            function.required_capabilities.clone()
                                                        });
                                                    let available = v
                                                        .test_available_capabilities
                                                        .iter()
                                                        .map(|capability| {
                                                            capability.name.clone()
                                                        })
                                                        .collect::<Vec<_>>();
                                                    v.test_capabilities =
                                                        crate::runtime::resolve_test_capabilities(
                                                            required_capabilities.as_deref(),
                                                            &available,
                                                        );
                                                    v.test_capability_select_open = false;
                                                    v.test_state = TestState::Idle;
                                                    cx.notify();
                                                }).ok();
                                            }
                                        }),
                                    )
                                    .child(
                                        action_button(
                                            "close-test",
                                            "关闭",
                                            ActionRole::Neutral,
                                            ActionSize::Dialog,
                                            style,
                                        )
                                        .on_mouse_down(MouseButton::Left, {
                                            let t = cx.weak_entity();
                                            move |_, _, cx| {
                                                t.update(cx, |v, cx| v.hide_test_dialog(cx)).ok();
                                            }
                                        }),
                                    )
                                    .child(
                                        action_button(
                                            "run-test",
                                            "执行测试",
                                            ActionRole::Main,
                                            ActionSize::Dialog,
                                            style,
                                        )
                                        .debug_selector(|| "FUNCTION_TEST_RUN".to_string())
                                        .role(Role::Button)
                                        .aria_label("执行测试")
                                        .on_mouse_down(MouseButton::Left, {
                                            let t = cx.weak_entity();
                                            move |_, _, cx| {
                                                t.update(cx, |v, cx| v.run_test(cx)).ok();
                                            }
                                        }),
                                    ),
                            ),
                    ),
                )
            })
    }
}

fn function_kind_label(kind: &str) -> &'static str {
    match kind {
        "builtin" => "内置函数",
        "custom" => "自定义函数",
        "placeholder" => "占位",
        _ => "未知",
    }
}

fn form_field(
    label: &'static str,
    input: Entity<InputState>,
    selector: &'static str,
    focused_selector: Option<&'static str>,
    value_selector: Option<String>,
    theme: &gpui_component::theme::Theme,
) -> impl IntoElement {
    let debug_selector = selector.to_string();
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
                .debug_selector(move || debug_selector.clone())
                .child(
                    Input::new(&input)
                        .aria_label(label)
                        .w_full()
                        .h(px(32.0))
                        .px(px(8.0))
                        .border_1()
                        .border_color(theme.border)
                        .rounded(px(4.0)),
                )
                .when_some(focused_selector, |field, selector| {
                    field.child(focus_marker(selector))
                })
                .when_some(value_selector, |field, selector| {
                    field.child(focus_marker(selector))
                }),
        )
}

fn form_field_multiline(
    label: &'static str,
    input: Entity<TextareaState>,
    selector: &'static str,
    focused_selector: Option<&'static str>,
    theme: &gpui_component::theme::Theme,
) -> impl IntoElement {
    let debug_selector = selector.to_string();
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
                .debug_selector(move || debug_selector.clone())
                .child(
                    Textarea::new(&input)
                        .aria_label(label)
                        .w_full()
                        .h(px(120.0))
                        .px(px(8.0))
                        .py(px(8.0))
                        .border_1()
                        .border_color(theme.border)
                        .rounded(px(4.0)),
                )
                .when_some(focused_selector, |field, selector| {
                    field.child(focus_marker(selector))
                }),
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

fn selector_field(
    label: &'static str,
    value: impl Into<SharedString>,
    id: impl Into<ElementId>,
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
