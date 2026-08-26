//! Agent management UI for the standalone local runtime.
//! scroll:agent_execution

use crate::datasource::{
    Store,
    entity_store::{AGENT_PAGE_SIZE, AgentFilter, AgentInput, AgentRecord, AgentStore},
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
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::scroll::ScrollableElement;

pub struct AgentView {
    store: Entity<Store>,
    items: Vec<AgentRecord>,
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
    form_system_prompt: String,
    form_model_preset: String,
    form_parent_agent_id: Option<i64>,
    form_tool_ids: Vec<i64>,
    form_skill_ids: Vec<i64>,
    form_capability_names: Vec<String>,
    available_parents: Vec<(i64, String)>,
    available_tools: Vec<(i64, String)>,
    available_skills: Vec<(i64, String)>,
    available_capabilities: Vec<String>,
    error_message: Option<String>,
    identifier_conflict: bool,
    confirm_delete_id: Option<i64>,
    identifier_input: Option<Entity<InputState>>,
    name_input: Option<Entity<InputState>>,
    description_input: Option<Entity<InputState>>,
    system_prompt_input: Option<Entity<InputState>>,
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
            page_size: AGENT_PAGE_SIZE,
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
            form_system_prompt: String::new(),
            form_model_preset: String::new(),
            form_parent_agent_id: None,
            form_tool_ids: Vec::new(),
            form_skill_ids: Vec::new(),
            form_capability_names: Vec::new(),
            available_parents: Vec::new(),
            available_tools: Vec::new(),
            available_skills: Vec::new(),
            available_capabilities: Vec::new(),
            error_message: None,
            identifier_conflict: false,
            confirm_delete_id: None,
            identifier_input: None,
            name_input: None,
            description_input: None,
            system_prompt_input: None,
            model_preset_input: None,
            search_input: None,
        };
        v.load_references(cx);
        v.load(cx);
        v
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        let store = self.store.read(cx).clone();
        let page = if self.search_text.is_empty() {
            AgentFilter::first().with_page(self.current_page + 1)
        } else {
            AgentFilter::first()
                .with_search(self.search_text.clone())
                .with_page(self.current_page + 1)
        };

        cx.spawn(async move |this, cx| {
            let result: std::result::Result<(Vec<AgentRecord>, i64, i64), String> = async {
                let agent_store = AgentStore::new(store.pool().clone())
                    .map_err(|e| format!("AgentStore 创建失败: {e}"))?;
                let data = agent_store
                    .search(&page)
                    .await
                    .map_err(|e| format!("分页加载失败: {e}"))?;
                Ok((data.records().to_vec(), data.total(), data.page_size()))
            }
            .await;
            match result {
                Ok((items, total_count, page_size)) => {
                    this.update(cx, |v, cx| {
                        v.items = items;
                        v.total_count = total_count;
                        v.page_size = page_size;
                        v.loading = false;
                        cx.notify();
                    })
                    .ok();
                }
                Err(err) => {
                    this.update(cx, |v, cx| {
                        v.error_message = Some(format!("加载失败: {err}"));
                        v.loading = false;
                        cx.notify();
                    })
                    .ok();
                }
            }
        })
        .detach();
    }

    fn load_references(&mut self, cx: &mut Context<Self>) {
        let store = self.store.read(cx).clone();
        cx.spawn(async move |this, cx| {
            let result = async {
                let pool = store.pool();
                let parents = sqlx::query_as::<_, (i64, String)>(
                    "SELECT id, identifier FROM agents ORDER BY identifier",
                )
                .fetch_all(pool)
                .await
                .map_err(|e| format!("读取 Agent 参照失败: {e}"))?;

                let tools = sqlx::query_as::<_, (i64, String)>(
                    "SELECT id, COALESCE(name, identifier) FROM tools ORDER BY COALESCE(name, identifier)",
                )
                .fetch_all(pool)
                .await
                .map_err(|e| format!("读取 Tool 参照失败: {e}"))?;

                let skills = sqlx::query_as::<_, (i64, String)>(
                    "SELECT id, COALESCE(name, identifier) FROM skills ORDER BY COALESCE(name, identifier)",
                )
                .fetch_all(pool)
                .await
                .map_err(|e| format!("读取 Skill 参照失败: {e}"))?;

                let capabilities = sqlx::query_as::<_, (String,)>("SELECT name FROM capabilities ORDER BY name")
                    .fetch_all(pool)
                    .await
                    .map_err(|e| format!("读取 Capability 参照失败: {e}"))?;

                Ok::<_, String>((parents, tools, skills, capabilities))
            }
            .await;

            match result {
                Ok((parents, tools, skills, capabilities)) => {
                    this.update(cx, |view, _| {
                        view.available_parents = parents;
                        view.available_tools = tools;
                        view.available_skills = skills;
                        view.available_capabilities = capabilities
                            .into_iter()
                            .map(|(capability,)| capability)
                            .collect();
                    })
                    .ok();
                }
                Err(err) => {
                    this.update(cx, |v, _| {
                        v.error_message = Some(err);
                    })
                    .ok();
                }
            }
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
        self.model_preset_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("模型预设（可选）")
                .default_value(&self.form_model_preset)
        }));
    }

    fn sync_form_inputs(&mut self, cx: &mut Context<Self>) {
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
        if let Some(ref inp) = self.model_preset_input {
            self.form_model_preset = inp.read(cx).value().to_string();
        }
    }

    fn show_add_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_form = true;
        self.editing_id = None;
        self.form_identifier.clear();
        self.form_name.clear();
        self.form_description.clear();
        self.form_system_prompt = "You are a local Agent.".to_string();
        self.form_model_preset.clear();
        self.form_parent_agent_id = None;
        self.form_tool_ids.clear();
        self.form_skill_ids.clear();
        self.form_capability_names.clear();
        self.error_message = None;
        self.identifier_conflict = false;
        self.form_scroll.set_offset(point(px(0.0), px(0.0)));
        self.init_inputs(window, cx);
        self.focus_identifier_on_next_frame(window, cx);
    }

    fn show_edit_form(&mut self, window: &mut Window, item: AgentRecord, cx: &mut Context<Self>) {
        self.show_form = true;
        self.editing_id = Some(item.id());
        self.form_identifier = item.identifier().to_string();
        self.form_name = item.name().to_string();
        self.form_description = item.description().unwrap_or_default().to_string();
        self.form_system_prompt = item.system_prompt().to_string();
        self.form_model_preset = item.model_preset().unwrap_or_default().to_string();
        self.form_parent_agent_id = item.parent_agent_id();
        self.form_tool_ids = item.tool_ids().to_vec();
        self.form_skill_ids = item.skill_ids().to_vec();
        self.form_capability_names = item
            .capability_names()
            .iter()
            .map(std::string::ToString::to_string)
            .collect();
        self.error_message = None;
        self.identifier_conflict = false;
        self.form_scroll.set_offset(point(px(0.0), px(0.0)));
        self.init_inputs(window, cx);
        self.focus_identifier_on_next_frame(window, cx);
    }

    fn focus_identifier_on_next_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let identifier = self
            .identifier_input
            .as_ref()
            .expect("Agent identifier input initialized")
            .clone();
        cx.on_next_frame(window, move |_view, window, cx| {
            identifier.update(cx, |input, cx| input.focus(window, cx));
        });
    }

    fn hide_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.form_scroll.set_offset(point(px(0.0), px(0.0)));
        self.show_form = false;
        self.editing_id = None;
        self.form_parent_agent_id = None;
        self.form_tool_ids.clear();
        self.form_skill_ids.clear();
        self.form_capability_names.clear();
        self.error_message = None;
        self.identifier_conflict = false;
        self.identifier_input = None;
        self.name_input = None;
        self.description_input = None;
        self.system_prompt_input = None;
        self.model_preset_input = None;
        self.add_focus.focus(window, cx);
        cx.notify();
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_form_inputs(cx);

        if self.form_identifier.trim().is_empty() || self.form_name.trim().is_empty() {
            self.error_message = Some("Identifier 和名称不能为空".into());
            self.error_focus.focus(window, cx);
            cx.notify();
            return;
        }

        if self.form_system_prompt.trim().is_empty() {
            self.error_message = Some("System Prompt 不能为空".into());
            self.error_focus.focus(window, cx);
            cx.notify();
            return;
        }

        let mut input = match AgentInput::new_root(
            self.form_identifier.clone(),
            self.form_name.clone(),
            self.form_system_prompt.clone(),
        ) {
            Ok(input) => input,
            Err(e) => {
                self.error_message = Some(format!("参数校验失败: {e}"));
                self.error_focus.focus(window, cx);
                cx.notify();
                return;
            }
        };

        if !self.form_description.is_empty() {
            input = input.with_description(self.form_description.clone());
        }

        if !self.form_model_preset.is_empty() {
            input = input.with_model_preset(self.form_model_preset.clone());
        }

        input = input
            .with_parent(self.form_parent_agent_id)
            .with_tools(self.form_tool_ids.clone())
            .with_skills(self.form_skill_ids.clone())
            .with_always_skills(Vec::<i64>::new())
            .with_capabilities(self.form_capability_names.clone());

        let store = self.store.read(cx).clone();

        if let Some(eid) = self.editing_id {
            let editing_id = eid;
            cx.spawn_in(window, async move |this, cx| {
                let result = async {
                    let agent_store = AgentStore::new(store.pool().clone())
                        .map_err(|e| format!("AgentStore 创建失败: {e}"))?;
                    agent_store
                        .update(editing_id, input)
                        .await
                        .map_err(|e| format!("更新失败: {e}"))?;
                    Ok::<_, String>(())
                }
                .await;

                _ = cx.update(|window, cx| {
                    _ = this.update(cx, |v, cx| match result {
                        Ok(()) => {
                            v.hide_form(window, cx);
                            v.load(cx);
                            v.load_references(cx);
                        }
                        Err(err) => {
                            v.identifier_conflict =
                                err.contains("DuplicateIdentifier") || err.contains("duplicate");
                            v.error_message = Some(err);
                            v.error_focus.focus(window, cx);
                            cx.notify();
                        }
                    });
                });
            })
            .detach();
        } else {
            cx.spawn_in(window, async move |this, cx| {
                let result = match AgentStore::new(store.pool().clone()) {
                    Ok(agent_store) => agent_store.create(input).await.map(|_| ()),
                    Err(error) => Err(error),
                };
                _ = cx.update(|window, cx| {
                    _ = this.update(cx, |v, cx| match result {
                        Ok(()) => {
                            v.hide_form(window, cx);
                            v.load(cx);
                            v.load_references(cx);
                        }
                        Err(error) => {
                            v.identifier_conflict =
                                error.field() == "identifier" && error.reason() == "duplicate";
                            v.error_message = Some(format!("创建失败: {error}"));
                            v.error_focus.focus(window, cx);
                            cx.notify();
                        }
                    });
                });
            })
            .detach();
        }
    }

    fn on_form_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event.keystroke.key.as_str() {
            "escape" => {
                cx.stop_propagation();
                self.hide_form(window, cx);
            }
            "enter" if self.show_form => {
                cx.stop_propagation();
                self.save(window, cx);
            }
            _ => {}
        }
    }

    fn delete(&mut self, id: i64, cx: &mut Context<Self>) {
        let store = self.store.read(cx).clone();
        cx.spawn(async move |this, cx| {
            let result = async {
                let agent_store = AgentStore::new(store.pool().clone())
                    .map_err(|e| format!("AgentStore 创建失败: {e}"))?;
                agent_store
                    .delete(id, None)
                    .await
                    .map_err(|e| format!("删除失败: {e}"))?;
                Ok::<_, String>(())
            }
            .await;

            match result {
                Ok(()) => {
                    this.update(cx, |v, cx| {
                        v.load(cx);
                        v.load_references(cx);
                    })
                    .ok();
                }
                Err(err) => {
                    this.update(cx, |v, cx| {
                        v.error_message = Some(err);
                        cx.notify();
                    })
                    .ok();
                }
            }
        })
        .detach();
    }

    fn set_default(&mut self, id: i64, cx: &mut Context<Self>) {
        let store = self.store.read(cx).clone();
        cx.spawn(async move |this, cx| {
            let result = async {
                let agent_store = AgentStore::new(store.pool().clone())
                    .map_err(|e| format!("AgentStore 创建失败: {e}"))?;
                agent_store
                    .set_default(id)
                    .await
                    .map_err(|e| format!("设置默认失败: {e}"))?;
                Ok::<_, String>(())
            }
            .await;
            match result {
                Ok(()) => {
                    this.update(cx, |v, cx| {
                        v.load(cx);
                    })
                    .ok();
                }
                Err(err) => {
                    this.update(cx, |v, cx| {
                        v.error_message = Some(err);
                        cx.notify();
                    })
                    .ok();
                }
            }
        })
        .detach();
    }

    fn toggle_tool(&mut self, id: i64) {
        if let Some(pos) = self.form_tool_ids.iter().position(|value| *value == id) {
            self.form_tool_ids.remove(pos);
        } else {
            self.form_tool_ids.push(id);
        }
    }

    fn toggle_skill(&mut self, id: i64) {
        if let Some(pos) = self.form_skill_ids.iter().position(|value| *value == id) {
            self.form_skill_ids.remove(pos);
        } else {
            self.form_skill_ids.push(id);
        }
    }

    fn toggle_capability(&mut self, name: String) {
        if let Some(pos) = self
            .form_capability_names
            .iter()
            .position(|value| value == &name)
        {
            self.form_capability_names.remove(pos);
        } else {
            self.form_capability_names.push(name);
        }
    }

    fn parent_label(&self, parent_agent_id: Option<i64>) -> String {
        parent_agent_id
            .and_then(|parent_id| {
                self.available_parents
                    .iter()
                    .find(|(id, _)| *id == parent_id)
                    .map(|(_, label)| label.clone())
            })
            .unwrap_or_else(|| "无".to_string())
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
        let total_pages = (self.total_count + self.page_size - 1) / self.page_size;

        if self.show_form && self.identifier_input.is_none() {
            self.init_inputs(window, cx);
        }

        if self.search_input.is_none() {
            self.search_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("输入名称")
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

        let parent_candidates = self.available_parents.clone();
        let tools = self.available_tools.clone();
        let skills = self.available_skills.clone();
        let capabilities = self.available_capabilities.clone();
        let form_tool_ids = self.form_tool_ids.clone();
        let form_skill_ids = self.form_skill_ids.clone();
        let form_capabilities = self.form_capability_names.clone();
        let selected_parent = self.form_parent_agent_id;

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
                            .child("Agent 管理"),
                    )
                    .child(
                        action_button(
                            "add-btn",
                            "+ 添加 Agent",
                            ActionRole::Main,
                            ActionSize::Page,
                            style,
                        )
                        .debug_selector(|| "AGENT_ADD".to_string())
                        .track_focus(&self.add_focus)
                        .tab_index(0)
                        .role(Role::Button)
                        .aria_label("添加 Agent")
                        .on_mouse_down(MouseButton::Left, {
                            let t = cx.weak_entity();
                            move |_, window, cx| {
                                t.update(cx, |v, cx| v.show_add_form(window, cx)).ok();
                            }
                        })
                        .when(self.add_focus.is_focused(window), |button| {
                            button.child(focus_marker("AGENT_ADD_FOCUSED"))
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
                    .child(if self.loading {
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .h_full()
                            .text_color(style.list.muted_foreground)
                            .text_size(px(14.0))
                            .child("加载中...")
                    } else if self.items.is_empty() {
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
                            px(150.0),
                            px(70.0),
                            px(70.0),
                            px(160.0),
                            px(170.0),
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
                                        list_header_cell(Some(col_widths[3]), style).child("Depth"),
                                    )
                                    .child(
                                        list_header_cell(Some(col_widths[4]), style).child("默认"),
                                    )
                                    .child(
                                        list_header_cell(Some(col_widths[5]), style).child("上级"),
                                    )
                                    .child(
                                        list_header_cell(Some(col_widths[6]), style).child("操作"),
                                    ),
                            )
                            .children(self.items.iter().map(|item| {
                                let id = item.id();
                                let current = item.clone();
                                let is_default = item.is_default();
                                let parent_label = self.parent_label(item.parent_agent_id());
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
                                            .child(format!("{}", item.depth())),
                                    )
                                    .child(
                                        list_cell(Some(col_widths[4]), style)
                                            .child(if is_default { "是" } else { "否" }),
                                    )
                                    .child(
                                        list_cell(Some(col_widths[5]), style).child(parent_label),
                                    )
                                    .child(
                                        list_actions(Some(col_widths[6]), style)
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
                                                            v.show_edit_form(
                                                                window,
                                                                current.clone(),
                                                                cx,
                                                            )
                                                        })
                                                        .ok();
                                                    }
                                                }),
                                            )
                                            .when(!is_default, |row| {
                                                row.child(
                                                    action_button(
                                                        ("set-default", id as u64),
                                                        "设为默认",
                                                        ActionRole::Warning,
                                                        ActionSize::Row,
                                                        style,
                                                    )
                                                    .on_mouse_down(MouseButton::Left, {
                                                        let t = cx.weak_entity();
                                                        move |_, _, cx| {
                                                            t.update(cx, |v, cx| {
                                                                v.set_default(id, cx);
                                                            })
                                                            .ok();
                                                        }
                                                    }),
                                                )
                                            })
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
                                    .child(format!(
                                        "第 {} / {} 页",
                                        self.current_page + 1,
                                        total_pages.max(1)
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
                let system_prompt_input = self.system_prompt_input.clone().unwrap();
                let model_preset_input = self.model_preset_input.clone().unwrap();
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
                        management_modal_layer(px(620.0)),
                        theme.popover,
                        theme.foreground,
                        theme.border,
                    )
                    .debug_selector(|| "AGENT_MODAL".to_string())
                    .track_focus(&self.form_focus)
                    .focus_trap("agent-form-focus-trap", &self.form_focus)
                    .key_context("HiveguiAgentForm")
                    .capture_key_down(cx.listener(|view, event: &KeyDownEvent, window, cx| {
                        view.on_form_key_down(event, window, cx);
                    }))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
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
                    .child(form_field(
                        "Identifier *",
                        identifier_input,
                        "AGENT_IDENTIFIER_INPUT",
                        Some(format!("AGENT_IDENTIFIER_VALUE-{}", self.form_identifier)),
                        theme,
                    ))
                    .child(form_field(
                        "名称 *",
                        name_input,
                        "AGENT_NAME_INPUT",
                        Some(format!("AGENT_NAME_VALUE-{}", self.form_name)),
                        theme,
                    ))
                    .child(
                        management_modal_scroll("agent-form-scroll", &self.form_scroll)
                            .debug_selector(|| "AGENT_FORM_SCROLL".to_string())
                            .gap(px(10.0))
                            .child(form_field(
                                "描述",
                                description_input,
                                "AGENT_DESCRIPTION_INPUT",
                                None,
                                theme,
                            ))
                            .child(form_field(
                                "System Prompt",
                                system_prompt_input,
                                "AGENT_SYSTEM_PROMPT_INPUT",
                                None,
                                theme,
                            ))
                            .child(form_field(
                                "Model Preset",
                                model_preset_input,
                                "AGENT_MODEL_PRESET_INPUT",
                                None,
                                theme,
                            ))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(4.0))
                                    .child(div().text_size(px(13.0)).child("上级 Agent"))
                                    .child(div().flex().flex_col().gap(px(4.0)).child({
                                        let t = cx.weak_entity();
                                        action_button(
                                            ("parent", 0_u64),
                                            if selected_parent.is_none() {
                                                "☑ 无父 Agent"
                                            } else {
                                                "□ 无父 Agent"
                                            },
                                            if selected_parent.is_none() {
                                                ActionRole::Edit
                                            } else {
                                                ActionRole::Neutral
                                            },
                                            ActionSize::Compact,
                                            style,
                                        )
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            move |_, _, _cx| {
                                                let _ = t.update(_cx, |v, _| {
                                                    v.form_parent_agent_id = None;
                                                });
                                            },
                                        )
                                    }))
                                    .children(
                                        parent_candidates
                                            .into_iter()
                                            .filter(|(pid, _)| Some(*pid) != self.editing_id)
                                            .map(|(pid, label)| {
                                                let active = selected_parent == Some(pid);
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .gap(px(6.0))
                                                    .child(
                                                        div()
                                                            .text_size(px(13.0))
                                                            .text_color(theme.foreground)
                                                            .child(label.clone()),
                                                    )
                                                    .child(
                                                        action_button(
                                                            ("set-parent", pid as u64),
                                                            if active {
                                                                "取消"
                                                            } else {
                                                                "选择"
                                                            },
                                                            if active {
                                                                ActionRole::Edit
                                                            } else {
                                                                ActionRole::Neutral
                                                            },
                                                            ActionSize::Compact,
                                                            style,
                                                        )
                                                        .on_mouse_down(MouseButton::Left, {
                                                            let t = cx.weak_entity();
                                                            move |_, _, cx| {
                                                                t.update(cx, |v, _| {
                                                                    v.form_parent_agent_id = if v
                                                                        .form_parent_agent_id
                                                                        == Some(pid)
                                                                    {
                                                                        None
                                                                    } else {
                                                                        Some(pid)
                                                                    };
                                                                })
                                                                .ok();
                                                            }
                                                        }),
                                                    )
                                            }),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(4.0))
                                    .child(div().text_size(px(13.0)).child("Tool 关联（可多选）"))
                                    .children(tools.into_iter().map(|(tool_id, label)| {
                                        let active = form_tool_ids.contains(&tool_id);
                                        let t = cx.weak_entity();
                                        let id = tool_id;
                                        let tool_label = label.clone();
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(6.0))
                                            .child(
                                                div()
                                                    .text_size(px(13.0))
                                                    .text_color(theme.foreground)
                                                    .child(if active { "☑" } else { "□" }),
                                            )
                                            .child(div().text_size(px(13.0)).child(tool_label))
                                            .child(
                                                action_button(
                                                    ("toggle-tool", id as u64),
                                                    if active { "移除" } else { "添加" },
                                                    if active {
                                                        ActionRole::Edit
                                                    } else {
                                                        ActionRole::Neutral
                                                    },
                                                    ActionSize::Compact,
                                                    style,
                                                )
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    move |_, _, cx| {
                                                        t.update(cx, |v, _| {
                                                            v.toggle_tool(id);
                                                        })
                                                        .ok();
                                                    },
                                                ),
                                            )
                                    })),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(4.0))
                                    .child(div().text_size(px(13.0)).child("Skill 关联（可多选）"))
                                    .children(skills.into_iter().map(|(skill_id, label)| {
                                        let active = form_skill_ids.contains(&skill_id);
                                        let t = cx.weak_entity();
                                        let id = skill_id;
                                        let skill_label = label.clone();
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(6.0))
                                            .child(
                                                div()
                                                    .text_size(px(13.0))
                                                    .text_color(theme.foreground)
                                                    .child(if active { "☑" } else { "□" }),
                                            )
                                            .child(div().text_size(px(13.0)).child(skill_label))
                                            .child(
                                                action_button(
                                                    ("toggle-skill", id as u64),
                                                    if active { "移除" } else { "添加" },
                                                    if active {
                                                        ActionRole::Edit
                                                    } else {
                                                        ActionRole::Neutral
                                                    },
                                                    ActionSize::Compact,
                                                    style,
                                                )
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    move |_, _, cx| {
                                                        t.update(cx, |v, _| {
                                                            v.toggle_skill(id);
                                                        })
                                                        .ok();
                                                    },
                                                ),
                                            )
                                    })),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(4.0))
                                    .child(
                                        div()
                                            .text_size(px(13.0))
                                            .child("Capability 关联（可多选）"),
                                    )
                                    .children(capabilities.into_iter().enumerate().map(
                                        |(idx, name)| {
                                            let cap_name = name.clone();
                                            let active = form_capabilities.contains(&cap_name);
                                            let t = cx.weak_entity();
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap(px(6.0))
                                                .child(
                                                    div()
                                                        .text_size(px(13.0))
                                                        .text_color(theme.foreground)
                                                        .child(if active { "☑" } else { "□" }),
                                                )
                                                .child(
                                                    div()
                                                        .text_size(px(13.0))
                                                        .child(cap_name.clone()),
                                                )
                                                .child(
                                                    action_button(
                                                        ("toggle-capability", idx as u64),
                                                        if active { "移除" } else { "添加" },
                                                        if active {
                                                            ActionRole::Edit
                                                        } else {
                                                            ActionRole::Neutral
                                                        },
                                                        ActionSize::Compact,
                                                        style,
                                                    )
                                                    .on_mouse_down(
                                                        MouseButton::Left,
                                                        move |_, _, cx| {
                                                            t.update(cx, |v, _| {
                                                                v.toggle_capability(
                                                                    cap_name.clone(),
                                                                );
                                                            })
                                                            .ok();
                                                        },
                                                    ),
                                                )
                                        },
                                    )),
                            )
                            .when_some(self.error_message.as_ref(), |this, err| {
                                this.child(
                                    div()
                                        .id("agent-form-error-a11y")
                                        .debug_selector(|| {
                                            if self.identifier_conflict {
                                                "AGENT_CONFLICT-field=identifier-reason=duplicate"
                                                    .to_string()
                                            } else {
                                                "AGENT_FORM_ERROR".to_string()
                                            }
                                        })
                                        .track_focus(&self.error_focus)
                                        .tab_index(0)
                                        .role(Role::Alert)
                                        .aria_label(if self.identifier_conflict {
                                            "field=identifier reason=duplicate"
                                        } else {
                                            "Agent form error"
                                        })
                                        .p(px(8.0))
                                        .bg(theme.warning.opacity(0.1))
                                        .rounded(px(4.0))
                                        .text_size(px(12.0))
                                        .text_color(theme.warning)
                                        .child(err.clone())
                                        .when(self.error_focus.is_focused(window), |error| {
                                            error.child(focus_marker("AGENT_ERROR_FOCUSED"))
                                        }),
                                )
                            })
                            .child(
                                div()
                                    .debug_selector(|| "AGENT_FORM_ACTIONS".to_string())
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
                                        .role(Role::Button)
                                        .aria_label("保存 Agent")
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
            .when_some(self.confirm_delete_id, |this, _id| {
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
                                                .child("确定要删除这个 Agent 吗？此操作不可恢复。"),
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
    value_selector: Option<String>,
    theme: &gpui_component::theme::Theme,
) -> impl IntoElement {
    let selector = selector.to_string();
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
                .debug_selector(move || selector.clone())
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
                .when_some(value_selector, |field, selector| {
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
