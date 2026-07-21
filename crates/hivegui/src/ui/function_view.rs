use crate::datasource::{
    Store,
    entity_store::{Capability, Function, Plugin},
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
use gpui_component::scroll::{Scrollable, ScrollableElement};
use std::collections::{HashMap, HashSet};

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

pub struct FunctionView {
    store: Entity<Store>,
    items: Vec<Function>,
    plugins: Vec<Plugin>,
    capabilities: Vec<Capability>,
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
    form_kind: i64,
    form_plugin_id: Option<i64>,
    form_plugin_export: Option<String>,
    form_capability: Option<String>,
    plugin_exports: Vec<String>,
    kind_select_open: bool,
    plugin_select_open: bool,
    export_select_open: bool,
    capability_select_open: bool,
    form_input_schema: String,
    form_output_schema: String,
    error_message: Option<String>,
    confirm_delete_id: Option<i64>,
    identifier_input: Option<Entity<InputState>>,
    name_input: Option<Entity<InputState>>,
    description_input: Option<Entity<InputState>>,
    input_schema_input: Option<Entity<InputState>>,
    output_schema_input: Option<Entity<InputState>>,
    search_input: Option<Entity<InputState>>,
    // Test dialog fields
    show_test: bool,
    test_function: Option<Function>,
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
        let mut v = Self {
            store,
            items: Vec::new(),
            plugins: Vec::new(),
            capabilities: Vec::new(),
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
            form_kind: 1,
            form_plugin_id: None,
            form_plugin_export: None,
            form_capability: None,
            plugin_exports: Vec::new(),
            kind_select_open: false,
            plugin_select_open: false,
            export_select_open: false,
            capability_select_open: false,
            form_input_schema: "{}".into(),
            form_output_schema: "{}".into(),
            error_message: None,
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
            this.update(cx, |view, cx| {
                view.plugins = plugins;
                view.capabilities = capabilities;
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
            .and_then(|manifest| serde_json::from_str::<serde_json::Value>(manifest).ok())
            .and_then(|manifest| manifest.get("exports").cloned())
            .and_then(|exports| serde_json::from_value::<Vec<String>>(exports).ok())
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
        let store = self.store.read(cx).clone();
        let search = if self.search_text.is_empty() {
            None
        } else {
            Some(self.search_text.clone())
        };
        let offset = self.current_page * self.page_size;
        cx.spawn(async move |this, cx| {
            let items = Function::list(store.pool(), search.clone(), 20, offset).await?;
            let count = Function::count(store.pool(), search).await?;
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
        self.form_kind = 1;
        self.form_plugin_id = None;
        self.form_plugin_export = None;
        self.form_capability = None;
        self.plugin_exports.clear();
        self.kind_select_open = false;
        self.plugin_select_open = false;
        self.export_select_open = false;
        self.capability_select_open = false;
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
                .placeholder("函数名称")
                .default_value("")
        }));
        self.description_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("描述（可选）")
                .default_value("")
        }));
        self.input_schema_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .placeholder("{}")
                .default_value("{}")
        }));
        self.output_schema_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .placeholder("{}")
                .default_value("{}")
        }));

        cx.notify();
    }

    fn show_edit_form(&mut self, window: &mut Window, item: Function, cx: &mut Context<Self>) {
        self.show_form = true;
        self.editing_id = Some(item.id);
        self.form_identifier = item.identifier.clone();
        self.form_name = item.name.clone();
        self.form_description = item.description.clone().unwrap_or_default();
        self.form_kind = item.kind;
        self.form_plugin_id = item.plugin_id;
        self.form_plugin_export = item.plugin_export.clone();
        self.form_capability = item
            .required_capabilities
            .as_deref()
            .and_then(|json| serde_json::from_str::<Vec<String>>(json).ok())
            .and_then(|mut names| names.drain(..).next());
        self.kind_select_open = false;
        self.plugin_select_open = false;
        self.export_select_open = false;
        self.capability_select_open = false;
        self.refresh_plugin_exports();
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
                .placeholder("函数名称")
                .default_value(&item.name)
        }));
        self.description_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("描述（可选）")
                .default_value(&item.description.unwrap_or_default())
        }));
        self.input_schema_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .placeholder("{}")
                .default_value(&item.input_schema)
        }));
        self.output_schema_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .placeholder("{}")
                .default_value(&item.output_schema)
        }));

        cx.notify();
    }

    fn hide_form(&mut self, cx: &mut Context<Self>) {
        self.form_scroll.set_offset(point(px(0.0), px(0.0)));
        self.show_form = false;
        self.editing_id = None;
        self.error_message = None;
        self.kind_select_open = false;
        self.plugin_select_open = false;
        self.export_select_open = false;
        self.capability_select_open = false;
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
        if let Some(ref inp) = self.input_schema_input {
            self.form_input_schema = inp.read(cx).value().to_string();
        }
        if let Some(ref inp) = self.output_schema_input {
            self.form_output_schema = inp.read(cx).value().to_string();
        }

        if self.form_identifier.trim().is_empty() || self.form_name.trim().is_empty() {
            self.error_message = Some("Identifier 和名称不能为空".into());
            cx.notify();
            return;
        }
        let kind = self.form_kind;
        if kind == 2 && (self.form_plugin_id.is_none() || self.form_plugin_export.is_none()) {
            self.error_message = Some("自定义函数必须选择关联插件和插件导出函数名".into());
            cx.notify();
            return;
        }
        let plugin_id = (kind == 2).then_some(self.form_plugin_id).flatten();
        let plugin_export = (kind == 2)
            .then_some(self.form_plugin_export.clone())
            .flatten();
        let required_capabilities = self.form_capability.as_ref().map(|name| {
            serde_json::to_string(&vec![name.clone()]).expect("serialize capability selection")
        });
        let store = self.store.read(cx).clone();
        let idf = self.form_identifier.clone();
        let name = self.form_name.clone();
        let desc = if self.form_description.is_empty() {
            None
        } else {
            Some(self.form_description.clone())
        };
        let is = self.form_input_schema.clone();
        let os = self.form_output_schema.clone();

        if let Some(eid) = self.editing_id {
            cx.spawn(async move |this, cx| {
                match Function::update(
                    store.pool(),
                    eid,
                    idf,
                    name,
                    desc,
                    kind,
                    is,
                    os,
                    plugin_id,
                    plugin_export,
                    None,
                    required_capabilities,
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
                match Function::create(
                    store.pool(),
                    idf,
                    name,
                    desc,
                    kind,
                    is,
                    os,
                    plugin_id,
                    plugin_export,
                    None,
                    required_capabilities,
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
            async move |this, cx| match Function::delete(store.pool(), id).await {
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
        function: Function,
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

        if let Ok(schema) = serde_json::from_str::<serde_json::Value>(&function.input_schema) {
            if let Some(obj) = schema.as_object() {
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

        let base_dir = Store::default_db_path();
        let mut allowed_capabilities = self.test_capabilities.iter().cloned().collect::<Vec<_>>();
        allowed_capabilities.sort();

        cx.spawn(async move |this, cx| {
            let start = std::time::Instant::now();
            let result = crate::runtime::FunctionTestExecutor::execute_with_capabilities(
                &function,
                input,
                &base_dir,
                allowed_capabilities,
            )
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
                                    .child(list_cell(Some(col_widths[3]), style).child(format!("{}", item.kind)))
                                    .child(
                                        list_actions(Some(col_widths[4]), style)
                                            .child(
                                                action_button(
                                                    ("test", id as u64),
                                                    "测试",
                                                    ActionRole::Main,
                                                    ActionSize::Row,
                                                    style,
                                                )
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
                                    if self.current_page > 0 {
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
                                self.current_page + 1,
                                tp.max(1)
                            )))
                            .child(
                                action_button(
                                    "next",
                                    "下一页",
                                    if (self.current_page + 1) * self.page_size < self.total_count {
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
                let kind_label = if self.form_kind == 1 {
                    "内置函数"
                } else {
                    "自定义函数"
                };
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
                            management_modal_scroll("function-form-scroll", &self.form_scroll)
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
                                .child(form_field("Identifier *", identifier_input, theme))
                                .child(form_field("名称 *", name_input, theme))
                                .child(form_field("描述", description_input, theme))
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
                                    .when(self.kind_select_open, |field| {
                                        field.child(selector_menu(theme).children(
                                            [(1_i64, "内置函数"), (2_i64, "自定义函数")]
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
                                                                    view.form_kind = kind;
                                                                    view.kind_select_open = false;
                                                                    if kind == 1 {
                                                                        view.form_plugin_id = None;
                                                                        view.form_plugin_export = None;
                                                                        view.plugin_exports.clear();
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
                                .when(self.form_kind == 2, |form| {
                                    form.child(
                                        selector_field(
                                            "关联插件 *",
                                            plugin_label,
                                            "plugin-selector",
                                            theme,
                                            {
                                                let view = cx.weak_entity();
                                                move |_, _, cx| {
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
                                                move |_, _, cx| {
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
                                .child(
                                    selector_field(
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
                                    }),
                                )
                                .child(form_field_multiline("Input Schema (JSON)", input_schema_input, theme))
                                .child(form_field_multiline("Output Schema (JSON)", output_schema_input, theme))
                                .when_some(self.error_message.as_ref(), |this, err| {
                                    this.child(
                                        div()
                                            .p(px(8.0))
                                            .bg(theme.warning.opacity(0.15))
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
                                                ActionSize::Dialog,
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
                                                ActionSize::Dialog,
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

                let kind_label = if function.kind == 1 { "内置函数" } else { "自定义函数" };
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
                        management_modal_layer(px(700.0)),
                        theme.popover,
                        theme.foreground,
                        theme.border,
                    )
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
                                            field_div.child(
                                                div()
                                                    .id(("test-enum", key_hash))
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
                                                field_div.child(
                                                    Input::new(input_state)
                                                        .w_full()
                                                        .h(px(32.0))
                                                        .px(px(8.0))
                                                        .border_1()
                                                        .border_color(theme.border)
                                                        .rounded(px(4.0))
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
                                    TestState::Idle => div().flex().flex_col().gap(px(8.0)),
                                    TestState::Running => {
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap(px(8.0))
                                            .child(
                                                div()
                                                    .text_size(px(13.0))
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .child("测试结果："),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(12.0))
                                                    .text_color(theme.muted_foreground)
                                                    .child("执行中..."),
                                            )
                                    }
                                    TestState::Success { output, elapsed_ms } => {
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap(px(8.0))
                                            .child(
                                                div()
                                                    .text_size(px(13.0))
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .child("测试结果："),
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
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap(px(8.0))
                                            .child(
                                                div()
                                                    .text_size(px(13.0))
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .child("测试结果："),
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

fn form_field(
    label: &'static str,
    input: Entity<InputState>,
    theme: &gpui_component::theme::Theme,
) -> impl IntoElement {
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
            Input::new(&input)
                .w_full()
                .h(px(32.0))
                .px(px(8.0))
                .border_1()
                .border_color(theme.border)
                .rounded(px(4.0)),
        )
}

fn form_field_multiline(
    label: &'static str,
    input: Entity<InputState>,
    theme: &gpui_component::theme::Theme,
) -> impl IntoElement {
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
            Input::new(&input)
                .w_full()
                .h(px(120.0))
                .px(px(8.0))
                .py(px(8.0))
                .border_1()
                .border_color(theme.border)
                .rounded(px(4.0)),
        )
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
