//! LLM 提示词调试工具 — 三栏布局：LLM 设置 / 消息编辑 / 结果展示。

use crate::datasource::entity_store::Tool as DbTool;
use crate::datasource::llm_store::{LlmModel, LlmProvider, LlmStore};
use crate::datasource::{Crypto, Store};
use crate::ui::management_style::{ActionRole, ManagementStyle};
use gpui::*;
use gpui_component::input::{Input, InputState};
use gpui_component::scroll::ScrollableElement;
use gpui_component::{
    ActiveTheme as _, Icon, IconName, Sizable,
    select::{SearchableVec, Select, SelectState},
};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

/// 模型选择器条目
#[derive(Debug, Clone)]
struct ModelSelectItem {
    id: i64,
    label: String,
}

impl gpui_component::searchable_list::SearchableListItem for ModelSelectItem {
    type Value = i64;

    fn title(&self) -> SharedString {
        SharedString::from(self.label.clone())
    }

    fn value(&self) -> &Self::Value {
        &self.id
    }
}

/// 消息角色
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum MessageRole {
    System,
    User,
    Assistant,
}

impl MessageRole {
    pub fn label(&self) -> &'static str {
        match self {
            MessageRole::System => "System",
            MessageRole::User => "User",
            MessageRole::Assistant => "Assistant",
        }
    }

    pub fn api_role(&self) -> &'static str {
        match self {
            MessageRole::System => "system",
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
        }
    }

    pub fn all() -> &'static [MessageRole] {
        &[
            MessageRole::System,
            MessageRole::User,
            MessageRole::Assistant,
        ]
    }
}

/// 一条调试消息
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DebugMessage {
    pub id: u64,
    pub role: MessageRole,
    pub content: String,
}

/// 工具定义（Function call）
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ToolDefinition {
    pub id: u64,
    pub name: String,
    pub description: String,
    /// JSON Schema 字符串（用户编辑）
    pub parameters_json: String,
}

/// API 调用状态
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum CallState {
    Idle,
    Loading,
    Success(String),
    Error(String),
}

/// 执行历史记录
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct ExecutionRecord {
    id: u64,
    timestamp: u64,
    model_name: String,
    temperature: f64,
    max_tokens: u32,
    top_p: f64,
    thinking_enabled: bool,
    thinking_budget: u32,
    messages: Vec<DebugMessage>,
    tools: Vec<ToolDefinition>,
    result: CallState,
}

/// 右侧面板 Tab（已废弃，保留兼容）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
enum RightPanelTab {
    Results,
    History,
}

/// 右键菜单项
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContextMenuItem {
    View,
    Apply,
}

pub struct PromptDebugger {
    store: Entity<Store>,
    llm_store: LlmStore,
    crypto: Crypto,
    // 左侧：LLM 设置
    models: Vec<LlmModel>,
    providers: Vec<LlmProvider>,
    selected_model_id: Option<i64>,
    model_select_state: Option<Entity<SelectState<SearchableVec<ModelSelectItem>>>>,
    temperature: f64,
    max_tokens: u32,
    top_p: f64,
    presence_penalty: f64,
    frequency_penalty: f64,
    // 中间：消息
    messages: Vec<DebugMessage>,
    next_msg_id: u64,
    // 工具定义
    tools: Vec<ToolDefinition>,
    next_tool_id: u64,
    // 工具编辑状态
    editing_tools: HashMap<u64, bool>,
    collapsed_tools: HashMap<u64, bool>,
    tool_inputs: HashMap<u64, (Entity<InputState>, Entity<InputState>, Entity<InputState>)>, // name, desc, params
    // 从函数管理选择工具
    available_tools: Vec<DbTool>,
    show_tool_picker: bool,
    tool_picker_search: String,
    tool_picker_scroll: ScrollHandle,
    // 右侧：结果
    call_state: CallState,
    // UI 状态
    loaded: bool,
    style: ManagementStyle,
    // 参数输入框
    temp_input: Option<Entity<InputState>>,
    max_tokens_input: Option<Entity<InputState>>,
    top_p_input: Option<Entity<InputState>>,
    presence_penalty_input: Option<Entity<InputState>>,
    frequency_penalty_input: Option<Entity<InputState>>,
    thinking_budget_input: Option<Entity<InputState>>,
    // 思考模式
    thinking_enabled: bool,
    thinking_budget_tokens: u32,
    // 消息折叠/编辑状态
    collapsed_messages: HashMap<u64, bool>,
    editing_messages: HashMap<u64, bool>,
    message_inputs: HashMap<u64, Entity<InputState>>,
    // 执行历史
    execution_history: Vec<ExecutionRecord>,
    next_record_id: u64,
    selected_record_ids: HashSet<u64>,
    comparing_records: Vec<ExecutionRecord>,
    history_scroll: ScrollHandle,
    // 右键菜单
    context_menu: Option<(u64, gpui::Point<Pixels>)>, // (record_id, position)
    // 查看/对比弹出窗口
    show_view_modal: bool,
    view_records: Vec<ExecutionRecord>,
    view_scroll: ScrollHandle,
    show_only_diff: bool,
}

impl PromptDebugger {
    pub fn new(cx: &mut Context<Self>, store: Entity<Store>, llm_store: LlmStore) -> Self {
        let crypto = llm_store.crypto().clone();
        let style = ManagementStyle::from_theme(cx.theme());
        let temp_input = None;
        let max_tokens_input = None;
        let top_p_input = None;
        let presence_penalty_input = None;
        let frequency_penalty_input = None;
        let thinking_budget_input = None;
        let mut this = Self {
            store,
            llm_store,
            crypto,
            models: vec![],
            providers: vec![],
            selected_model_id: None,
            model_select_state: None,
            temperature: 0.7,
            max_tokens: 2048,
            top_p: 1.0,
            presence_penalty: 0.0,
            frequency_penalty: 0.0,
            thinking_enabled: true,
            thinking_budget_tokens: 1024,
            messages: vec![
                DebugMessage {
                    id: 1,
                    role: MessageRole::System,
                    content: "你是一个专业的AI助手。".to_string(),
                },
                DebugMessage {
                    id: 2,
                    role: MessageRole::User,
                    content: "你好".to_string(),
                },
            ],
            next_msg_id: 3,
            tools: vec![],
            next_tool_id: 1,
            editing_tools: HashMap::new(),
            collapsed_tools: HashMap::new(),
            tool_inputs: HashMap::new(),
            available_tools: vec![],
            show_tool_picker: false,
            tool_picker_search: String::new(),
            tool_picker_scroll: ScrollHandle::default(),
            call_state: CallState::Idle,
            loaded: false,
            style,
            temp_input,
            max_tokens_input,
            top_p_input,
            presence_penalty_input,
            frequency_penalty_input,
            thinking_budget_input,
            collapsed_messages: HashMap::new(),
            editing_messages: HashMap::new(),
            message_inputs: HashMap::new(),
            execution_history: vec![],
            next_record_id: 1,
            selected_record_ids: HashSet::new(),
            comparing_records: vec![],
            history_scroll: ScrollHandle::default(),
            context_menu: None,
            show_view_modal: false,
            view_records: vec![],
            view_scroll: ScrollHandle::default(),
            show_only_diff: false,
        };
        this.load_data(cx);
        this.load_history();
        this
    }

    fn load_data(&mut self, cx: &mut Context<Self>) {
        let llm_store = self.llm_store.clone();
        let store = self.store.read(cx).clone();
        cx.spawn(async move |this, cx| {
            let models = llm_store.list_models().await.unwrap_or_default();
            let providers = llm_store.list_providers().await.unwrap_or_default();
            let available_tools = DbTool::list(store.pool(), None, 100, 0)
                .await
                .unwrap_or_default();
            _ = this.update(cx, |this, cx| {
                this.models = models;
                this.providers = providers;
                this.available_tools = available_tools;
                if this.selected_model_id.is_none() {
                    this.selected_model_id = this.models.first().map(|m| m.id);
                }
                this.loaded = true;
                this.style = ManagementStyle::from_theme(cx.theme());
                cx.notify();
            });
        })
        .detach();
    }

    /// 懒初始化模型下拉列表（需要 &mut Window）
    fn ensure_model_select_state(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.model_select_state.is_some() {
            return;
        }
        let items: SearchableVec<ModelSelectItem> = SearchableVec::new(
            self.models
                .iter()
                .map(|m| {
                    let provider_label = m
                        .provider_id
                        .and_then(|pid| self.providers.iter().find(|p| p.id == pid))
                        .map(|p| p.name.clone())
                        .unwrap_or_default();
                    let label = if provider_label.is_empty() {
                        m.name.clone()
                    } else {
                        format!("{} ({})", m.name, provider_label)
                    };
                    ModelSelectItem { id: m.id, label }
                })
                .collect::<Vec<_>>(),
        );
        let initial_index = self
            .selected_model_id
            .and_then(|sid| self.models.iter().position(|m| m.id == sid))
            .map(|ix| gpui_component::IndexPath::default().row(ix));
        let select_state =
            cx.new(|cx| SelectState::new(items, initial_index, window, cx).searchable(true));

        // 订阅 SelectEvent，当用户选择模型时更新 selected_model_id
        cx.subscribe_in(&select_state, window, Self::on_model_select)
            .detach();

        self.model_select_state = Some(select_state);
    }

    /// 模型选择事件处理
    fn on_model_select(
        &mut self,
        _: &Entity<SelectState<SearchableVec<ModelSelectItem>>>,
        event: &gpui_component::select::SelectEvent<SearchableVec<ModelSelectItem>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let gpui_component::select::SelectEvent::Confirm(Some(model_id)) = event {
            self.selected_model_id = Some(*model_id);
            cx.notify();
        }
    }

    fn add_message(&mut self, role: MessageRole) {
        let id = self.next_msg_id;
        self.next_msg_id += 1;
        self.messages.push(DebugMessage {
            id,
            role,
            content: String::new(),
        });
    }

    fn remove_message(&mut self, id: u64) {
        self.messages.retain(|m| m.id != id);
    }

    fn update_message_content(&mut self, id: u64, content: String) {
        if let Some(msg) = self.messages.iter_mut().find(|m| m.id == id) {
            msg.content = content;
        }
    }

    fn update_message_role(&mut self, id: u64, role: MessageRole) {
        if let Some(msg) = self.messages.iter_mut().find(|m| m.id == id) {
            msg.role = role;
        }
    }

    fn move_message_up(&mut self, id: u64) {
        if let Some(pos) = self.messages.iter().position(|m| m.id == id) {
            if pos > 0 {
                self.messages.swap(pos, pos - 1);
            }
        }
    }

    fn move_message_down(&mut self, id: u64) {
        if let Some(pos) = self.messages.iter().position(|m| m.id == id) {
            if pos + 1 < self.messages.len() {
                self.messages.swap(pos, pos + 1);
            }
        }
    }

    fn add_tool(&mut self) {
        let id = self.next_tool_id;
        self.next_tool_id += 1;
        self.tools.push(ToolDefinition {
            id,
            name: String::new(),
            description: String::new(),
            parameters_json: r#"{"type": "object", "properties": {}}"#.to_string(),
        });
        // 新工具直接进入编辑模式
        self.editing_tools.insert(id, true);
    }

    fn toggle_tool_picker(&mut self) {
        self.show_tool_picker = !self.show_tool_picker;
        if self.show_tool_picker {
            self.tool_picker_search.clear();
        }
    }

    fn add_tool_from_management(&mut self, tool_id: i64) {
        if let Some(db_tool) = self.available_tools.iter().find(|t| t.id == tool_id) {
            let id = self.next_tool_id;
            self.next_tool_id += 1;
            self.tools.push(ToolDefinition {
                id,
                name: db_tool.name.clone(),
                description: db_tool.description.clone(),
                parameters_json: db_tool.input_schema.clone(),
            });
            // 新工具直接进入编辑模式
            self.editing_tools.insert(id, true);
        }
    }

    fn remove_tool(&mut self, id: u64) {
        self.tools.retain(|t| t.id != id);
        self.editing_tools.remove(&id);
        self.collapsed_tools.remove(&id);
        self.tool_inputs.remove(&id);
    }

    fn toggle_collapse_tool(&mut self, id: u64) {
        let collapsed = self.collapsed_tools.entry(id).or_insert(false);
        *collapsed = !*collapsed;
    }

    fn toggle_tool_edit(&mut self, id: u64) {
        let editing = self.editing_tools.entry(id).or_insert(false);
        *editing = !*editing;
        // 进入编辑时自动展开
        if *editing {
            if let Some(collapsed) = self.collapsed_tools.get_mut(&id) {
                *collapsed = false;
            }
        }
    }

    fn get_tool_inputs(
        &mut self,
        id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Entity<InputState>, Entity<InputState>, Entity<InputState>) {
        self.tool_inputs
            .entry(id)
            .or_insert_with(|| {
                let tool = self.tools.iter().find(|t| t.id == id);
                let name = tool.map(|t| t.name.clone()).unwrap_or_default();
                let desc = tool.map(|t| t.description.clone()).unwrap_or_default();
                let params = tool.map(|t| t.parameters_json.clone()).unwrap_or_default();
                let name_input = cx.new(|cx| InputState::new(window, cx).default_value(&name));
                let desc_input = cx.new(|cx| {
                    InputState::new(window, cx)
                        .multi_line(true)
                        .default_value(&desc)
                });
                let params_input = cx.new(|cx| {
                    InputState::new(window, cx)
                        .multi_line(true)
                        .auto_grow(3, 8)
                        .default_value(&params)
                });
                (name_input, desc_input, params_input)
            })
            .clone()
    }

    fn save_tool_edit(&mut self, id: u64, cx: &mut Context<Self>) {
        if let Some((name_input, desc_input, params_input)) = self.tool_inputs.get(&id) {
            let new_name = name_input.read(cx).text().to_string();
            let new_desc = desc_input.read(cx).text().to_string();
            let new_params = params_input.read(cx).text().to_string();
            if let Some(tool) = self.tools.iter_mut().find(|t| t.id == id) {
                tool.name = new_name;
                tool.description = new_desc;
                tool.parameters_json = new_params;
            }
        }
        if let Some(editing) = self.editing_tools.get_mut(&id) {
            *editing = false;
        }
    }

    fn toggle_thinking(&mut self) {
        self.thinking_enabled = !self.thinking_enabled;
    }

    fn toggle_collapse(&mut self, id: u64) {
        let collapsed = self.collapsed_messages.entry(id).or_insert(false);
        *collapsed = !*collapsed;
    }

    fn toggle_edit(&mut self, id: u64) {
        let editing = self.editing_messages.entry(id).or_insert(false);
        *editing = !*editing;
        // 进入编辑时自动展开
        if *editing {
            if let Some(collapsed) = self.collapsed_messages.get_mut(&id) {
                *collapsed = false;
            }
        }
    }

    fn get_message_input(
        &mut self,
        id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        self.message_inputs
            .entry(id)
            .or_insert_with(|| {
                let content = self
                    .messages
                    .iter()
                    .find(|m| m.id == id)
                    .map(|m| m.content.clone())
                    .unwrap_or_default();
                cx.new(|cx| {
                    InputState::new(window, cx)
                        .multi_line(true)
                        .auto_grow(1, 5)
                        .default_value(&content)
                })
            })
            .clone()
    }

    fn save_message_content(&mut self, id: u64, cx: &mut Context<Self>) {
        if let Some(input) = self.message_inputs.get(&id) {
            let new_content = input.read(cx).text().to_string();
            self.update_message_content(id, new_content);
        }
        if let Some(editing) = self.editing_messages.get_mut(&id) {
            *editing = false;
        }
    }

    /// 获取历史文件路径
    fn history_file_path() -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        PathBuf::from(home)
            .join(".hiveclaw")
            .join("prompt_debug_history.json")
    }

    /// 加载历史记录
    fn load_history(&mut self) {
        let path = Self::history_file_path();
        if let Ok(content) = std::fs::read_to_string(&path) {
            if let Ok(records) = serde_json::from_str::<Vec<ExecutionRecord>>(&content) {
                self.execution_history = records;
                self.next_record_id = self
                    .execution_history
                    .iter()
                    .map(|r| r.id + 1)
                    .max()
                    .unwrap_or(1);
            }
        }
    }

    /// 保存历史记录
    fn save_history(&self) {
        let path = Self::history_file_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(content) = serde_json::to_string_pretty(&self.execution_history) {
            let _ = std::fs::write(&path, content);
        }
    }

    /// 保存当前执行到历史
    fn save_execution_record(&mut self) {
        let model_name = self
            .selected_model_id
            .and_then(|id| self.models.iter().find(|m| m.id == id))
            .map(|m| m.name.clone())
            .unwrap_or_default();

        let record = ExecutionRecord {
            id: self.next_record_id,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            model_name,
            temperature: self.temperature,
            max_tokens: self.max_tokens,
            top_p: self.top_p,
            thinking_enabled: self.thinking_enabled,
            thinking_budget: self.thinking_budget_tokens,
            messages: self.messages.clone(),
            tools: self.tools.clone(),
            result: self.call_state.clone(),
        };

        self.next_record_id += 1;
        self.execution_history.push(record);
        // 保留最近 50 条
        if self.execution_history.len() > 50 {
            self.execution_history =
                self.execution_history[self.execution_history.len() - 50..].to_vec();
        }
        self.save_history();
    }

    /// 切换记录选中状态
    fn toggle_record_selection(&mut self, id: u64) {
        if self.selected_record_ids.contains(&id) {
            self.selected_record_ids.remove(&id);
        } else {
            self.selected_record_ids.insert(id);
        }
    }

    /// 全选/取消全选
    fn toggle_select_all(&mut self) {
        if self.selected_record_ids.len() == self.execution_history.len() {
            self.selected_record_ids.clear();
        } else {
            self.selected_record_ids = self.execution_history.iter().map(|r| r.id).collect();
        }
    }

    /// 清除历史
    fn clear_history(&mut self) {
        self.execution_history.clear();
        self.selected_record_ids.clear();
        self.comparing_records.clear();
        self.save_history();
    }

    /// 开始对比选中的记录
    fn start_comparison(&mut self) {
        let selected: Vec<ExecutionRecord> = self
            .execution_history
            .iter()
            .filter(|r| self.selected_record_ids.contains(&r.id))
            .cloned()
            .collect();
        if selected.len() >= 2 {
            self.view_records = selected;
            self.show_view_modal = true;
        }
    }

    /// 打开查看窗口（单条记录）
    fn open_view_record(&mut self, record_id: u64) {
        if let Some(record) = self.execution_history.iter().find(|r| r.id == record_id) {
            self.view_records = vec![record.clone()];
            self.show_view_modal = true;
        }
        self.context_menu = None;
    }

    /// 关闭查看窗口
    fn close_view_modal(&mut self) {
        self.show_view_modal = false;
        self.view_records.clear();
        self.show_only_diff = false;
    }

    /// 切换仅看差异
    fn toggle_show_only_diff(&mut self) {
        self.show_only_diff = !self.show_only_diff;
    }

    /// 显示右键菜单
    fn show_context_menu(&mut self, record_id: u64, position: gpui::Point<Pixels>) {
        self.context_menu = Some((record_id, position));
    }

    /// 隐藏右键菜单
    fn hide_context_menu(&mut self) {
        self.context_menu = None;
    }

    /// 应用历史记录到编辑器
    fn apply_record(
        &mut self,
        record: &ExecutionRecord,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.messages = record.messages.clone();
        self.temperature = record.temperature;
        self.max_tokens = record.max_tokens;
        self.top_p = record.top_p;
        self.thinking_enabled = record.thinking_enabled;
        self.thinking_budget_tokens = record.thinking_budget;
        // 重建 InputState 以更新默认值
        self.temp_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("0.70")
                .default_value(&record.temperature.to_string())
        }));
        self.max_tokens_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("2048")
                .default_value(&record.max_tokens.to_string())
        }));
        self.top_p_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("1.00")
                .default_value(&record.top_p.to_string())
        }));
        self.thinking_budget_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("1024")
                .default_value(&record.thinking_budget.to_string())
        }));
        // 回填工具定义
        self.tools = record.tools.clone();
        self.context_menu = None;
    }
}

// ──────────────────────────────────────────────
// UI 渲染组件
// ──────────────────────────────────────────────

/// 模型选择器（Select 下拉列表）
fn model_selector(
    select_state: &Entity<SelectState<SearchableVec<ModelSelectItem>>>,
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
                .child("模型"),
        )
        .child(
            div()
                .w_full()
                .child(Select::new(select_state).placeholder("请选择模型")),
        )
}

/// 参数输入控件
fn param_control(
    label: &'static str,
    min_label: &'static str,
    max_label: &'static str,
    input: &Entity<InputState>,
    style: &ManagementStyle,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(4.0))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(style.list.muted_foreground)
                        .child(label),
                )
                .child(div().w(px(60.0)).child(Input::new(input))),
        )
        .child(
            div()
                .flex()
                .justify_between()
                .text_size(px(10.0))
                .text_color(style.list.muted_foreground)
                .child(min_label)
                .child(max_label),
        )
}

/// 带 tooltip 帮助图标的参数控制
fn param_control_with_tooltip(
    label: &'static str,
    min_label: &'static str,
    max_label: &'static str,
    input: &Entity<InputState>,
    tooltip_text: &'static str,
    style: &ManagementStyle,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(4.0))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(style.list.muted_foreground)
                                .child(label),
                        )
                        .child(
                            div()
                                .cursor(CursorStyle::PointingHand)
                                .child(Icon::new(IconName::Info).xsmall())
                                .hover(|s| s.text_color(style.list.muted_foreground.opacity(1.0))),
                        ),
                )
                .child(div().w(px(60.0)).child(Input::new(input))),
        )
        .child(
            div()
                .flex()
                .justify_between()
                .text_size(px(10.0))
                .text_color(style.list.muted_foreground)
                .child(min_label)
                .child(max_label),
        )
}

/// 左侧 LLM 设置面板
fn settings_panel(
    model_select_state: &Entity<SelectState<SearchableVec<ModelSelectItem>>>,
    temp_input: &Entity<InputState>,
    max_tokens_input: &Entity<InputState>,
    top_p_input: &Entity<InputState>,
    presence_penalty_input: &Entity<InputState>,
    frequency_penalty_input: &Entity<InputState>,
    thinking_budget_input: &Entity<InputState>,
    thinking_enabled: bool,
    entity: Entity<PromptDebugger>,
    style: &ManagementStyle,
) -> impl IntoElement {
    let mut panel = div()
        .w(px(220.0))
        .flex_shrink_0()
        .h_full()
        .p(px(12.0))
        .border_r_1()
        .border_color(style.list.border)
        .overflow_y_scrollbar()
        .child(
            div()
                .text_size(px(14.0))
                .font_weight(FontWeight::BOLD)
                .mb(px(12.0))
                .child("LLM 设定"),
        )
        .child(model_selector(model_select_state, style))
        .child(div().mt(px(12.0)))
        .child(thinking_toggle(thinking_enabled, entity.clone(), style))
        .child(div().mt(px(8.0)))
        .child(param_control_with_tooltip(
            "思考预算",
            "128",
            "8192",
            thinking_budget_input,
            "思考模式下模型用于推理的最大 token 数",
            style,
        ));

    if thinking_enabled {
        panel = panel
            .child(div().mt(px(12.0)))
            .child(param_control_with_tooltip(
                "Temperature",
                "精确",
                "创造",
                temp_input,
                "控制输出随机性：0=确定性输出，1=更随机",
                style,
            ))
            .child(div().mt(px(12.0)))
            .child(param_control_with_tooltip(
                "Top P",
                "0",
                "1",
                top_p_input,
                "核采样：从概率最高的 token 中累计概率达到 P 的集合中采样",
                style,
            ))
            .child(div().mt(px(12.0)))
            .child(param_control_with_tooltip(
                "Presence Penalty",
                "-2",
                "2",
                presence_penalty_input,
                "存在惩罚：已出现的 token 会降低再次出现的概率",
                style,
            ))
            .child(div().mt(px(12.0)))
            .child(param_control_with_tooltip(
                "Frequency Penalty",
                "-2",
                "2",
                frequency_penalty_input,
                "频率惩罚：根据 token 出现频率降低再次出现的概率",
                style,
            ));
    }

    panel
}

/// 思考模式开关
fn thinking_toggle(
    enabled: bool,
    entity: Entity<PromptDebugger>,
    style: &ManagementStyle,
) -> impl IntoElement {
    let main_colors = style.action(ActionRole::Main);
    let neutral_colors = style.action(ActionRole::Neutral);
    let (toggle_bg, toggle_fg) = if enabled {
        (main_colors.background, main_colors.foreground)
    } else {
        (neutral_colors.background, neutral_colors.foreground)
    };
    div()
        .flex()
        .flex_row()
        .gap(px(8.0))
        .items_center()
        .child(
            div()
                .text_size(px(12.0))
                .text_color(style.list.muted_foreground)
                .child("思考模式"),
        )
        .child(
            div()
                .px(px(10.0))
                .py(px(4.0))
                .rounded(px(4.0))
                .bg(toggle_bg)
                .text_color(toggle_fg)
                .cursor(CursorStyle::PointingHand)
                .child(
                    Icon::new(if enabled {
                        IconName::Bot
                    } else {
                        IconName::Settings
                    })
                    .xsmall(),
                )
                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    _ = entity.update(cx, |t, cx| {
                        t.toggle_thinking();
                        cx.notify();
                    });
                }),
        )
}

/// 单条消息卡片
fn message_card(
    msg: &DebugMessage,
    style: &ManagementStyle,
    entity: Entity<PromptDebugger>,
    collapsed: bool,
    editing: bool,
    input: Option<Entity<InputState>>,
) -> impl IntoElement {
    let msg_id = msg.id;
    let role = msg.role;
    let content = msg.content.clone();
    let role_color = match role {
        MessageRole::System => style.action(ActionRole::Main).background,
        MessageRole::User => style.action(ActionRole::Edit).background,
        MessageRole::Assistant => style.action(ActionRole::Neutral).background,
    };

    let collapse_icon = if collapsed {
        IconName::ChevronRight
    } else {
        IconName::ChevronDown
    };
    let edit_icon = if editing {
        IconName::Check
    } else {
        IconName::Replace
    };

    let mut card = div()
        .id(SharedString::from(format!("msg-card-{}", msg.id)))
        .mb(px(8.0))
        .border_1()
        .border_color(style.list.border)
        .rounded(px(6.0))
        .overflow_hidden()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .px(px(10.0))
                .py(px(6.0))
                .bg(role_color)
                .child(
                    div()
                        .text_size(px(12.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(role.label()),
                )
                .child(
                    div()
                        .flex()
                        .gap(px(4.0))
                        .child(
                            div()
                                .cursor(CursorStyle::PointingHand)
                                .hover(|s| s.opacity(0.7))
                                .child(Icon::new(collapse_icon).xsmall())
                                .on_mouse_down(MouseButton::Left, {
                                    let entity = entity.clone();
                                    move |_, _, cx| {
                                        _ = entity.update(cx, |view, cx| {
                                            view.toggle_collapse(msg_id);
                                            cx.notify();
                                        });
                                    }
                                }),
                        )
                        .child(
                            div()
                                .cursor(CursorStyle::PointingHand)
                                .hover(|s| s.opacity(0.7))
                                .child(Icon::new(edit_icon).xsmall())
                                .on_mouse_down(MouseButton::Left, {
                                    let entity = entity.clone();
                                    move |_, _, cx| {
                                        _ = entity.update(cx, |view, cx| {
                                            if editing {
                                                view.save_message_content(msg_id, cx);
                                            } else {
                                                view.toggle_edit(msg_id);
                                            }
                                            cx.notify();
                                        });
                                    }
                                }),
                        )
                        .child(
                            div()
                                .cursor(CursorStyle::PointingHand)
                                .hover(|s| s.opacity(0.7))
                                .child(Icon::new(IconName::ArrowUp).xsmall())
                                .on_mouse_down(MouseButton::Left, {
                                    let entity = entity.clone();
                                    move |_, _, cx| {
                                        _ = entity.update(cx, |view, cx| {
                                            view.move_message_up(msg_id);
                                            cx.notify();
                                        });
                                    }
                                }),
                        )
                        .child(
                            div()
                                .cursor(CursorStyle::PointingHand)
                                .hover(|s| s.opacity(0.7))
                                .child(Icon::new(IconName::ArrowDown).xsmall())
                                .on_mouse_down(MouseButton::Left, {
                                    let entity = entity.clone();
                                    move |_, _, cx| {
                                        _ = entity.update(cx, |view, cx| {
                                            view.move_message_down(msg_id);
                                            cx.notify();
                                        });
                                    }
                                }),
                        )
                        .child(
                            div()
                                .cursor(CursorStyle::PointingHand)
                                .hover(|s| s.opacity(0.7))
                                .child(Icon::new(IconName::Close).xsmall())
                                .on_mouse_down(MouseButton::Left, {
                                    let entity = entity.clone();
                                    move |_, _, cx| {
                                        _ = entity.update(cx, |view, cx| {
                                            view.remove_message(msg_id);
                                            cx.notify();
                                        });
                                    }
                                }),
                        ),
                ),
        );

    if collapsed {
        // 折叠状态：只显示一行摘要
        let summary = if content.chars().count() > 40 {
            format!("{}...", content.chars().take(40).collect::<String>())
        } else {
            content.clone()
        };
        card = card.child(
            div()
                .px(px(10.0))
                .py(px(6.0))
                .text_size(px(12.0))
                .text_color(style.list.muted_foreground)
                .child(summary),
        );
    } else if editing {
        // 编辑状态：显示输入框和保存按钮
        if let Some(input_state) = input {
            card = card.child(
                div()
                    .px(px(10.0))
                    .py(px(8.0))
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(div().w_full().child(Input::new(&input_state)))
                    .child(
                        div().flex().justify_end().gap(px(6.0)).child(
                            div()
                                .px(px(8.0))
                                .py(px(4.0))
                                .bg(style.action(ActionRole::Main).background)
                                .rounded(px(4.0))
                                .text_color(style.action(ActionRole::Main).foreground)
                                .cursor(CursorStyle::PointingHand)
                                .hover(|s| s.opacity(0.8))
                                .child(Icon::new(IconName::Check).xsmall())
                                .on_mouse_down(MouseButton::Left, {
                                    let entity = entity.clone();
                                    move |_, _, cx| {
                                        _ = entity.update(cx, |view, cx| {
                                            view.save_message_content(msg_id, cx);
                                            cx.notify();
                                        });
                                    }
                                }),
                        ),
                    ),
            );
        }
    } else {
        // 正常状态：显示内容
        card = card.child(
            div()
                .px(px(10.0))
                .py(px(8.0))
                .min_h(px(60.0))
                .text_size(px(13.0))
                .child(content),
        );
    }

    card
}

/// 中间消息编辑器（作为方法，可访问折叠/编辑状态）
impl PromptDebugger {
    fn message_editor(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        style: &ManagementStyle,
        entity: Entity<PromptDebugger>,
    ) -> impl IntoElement {
        let mut content = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full()
            .p(px(12.0))
            .overflow_y_scrollbar()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .mb(px(12.0))
                    .child(
                        div()
                            .text_size(px(14.0))
                            .font_weight(FontWeight::BOLD)
                            .child("Messages"),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(style.list.muted_foreground)
                            .child(format!("({})", self.messages.len())),
                    ),
            );

        // 收集状态信息（避免在循环中同时借用 self）
        let msg_states: Vec<(u64, bool, bool)> = self
            .messages
            .iter()
            .map(|msg| {
                let collapsed = self
                    .collapsed_messages
                    .get(&msg.id)
                    .copied()
                    .unwrap_or(false);
                let editing = self.editing_messages.get(&msg.id).copied().unwrap_or(false);
                (msg.id, collapsed, editing)
            })
            .collect();

        // 为需要编辑的消息预创建 InputState
        let mut input_map: HashMap<u64, Entity<InputState>> = HashMap::new();
        for (id, _, editing) in &msg_states {
            if *editing {
                let input = self.get_message_input(*id, window, cx);
                input_map.insert(*id, input);
            }
        }

        for msg in &self.messages {
            let (_, collapsed, editing) =
                msg_states.iter().find(|(id, _, _)| *id == msg.id).unwrap();
            let input = input_map.remove(&msg.id);
            content = content.child(message_card(
                msg,
                style,
                entity.clone(),
                *collapsed,
                *editing,
                input,
            ));
        }

        // 添加消息按钮行
        content = content.child(
            div()
                .flex()
                .gap(px(6.0))
                .mt(px(8.0))
                .child(
                    div()
                        .px(px(8.0))
                        .py(px(4.0))
                        .bg(style.action(ActionRole::Main).background)
                        .rounded(px(4.0))
                        .cursor(CursorStyle::PointingHand)
                        .hover(|s| s.bg(style.action(ActionRole::Main).hover))
                        .child(Icon::new(IconName::Settings).xsmall())
                        .on_mouse_down(MouseButton::Left, {
                            let entity = entity.clone();
                            move |_, _, cx| {
                                _ = entity.update(cx, |view, cx| {
                                    view.add_message(MessageRole::System);
                                    cx.notify();
                                });
                            }
                        }),
                )
                .child(
                    div()
                        .px(px(8.0))
                        .py(px(4.0))
                        .bg(style.action(ActionRole::Main).background)
                        .rounded(px(4.0))
                        .cursor(CursorStyle::PointingHand)
                        .hover(|s| s.bg(style.action(ActionRole::Main).hover))
                        .child(Icon::new(IconName::User).xsmall())
                        .on_mouse_down(MouseButton::Left, {
                            let entity = entity.clone();
                            move |_, _, cx| {
                                _ = entity.update(cx, |view, cx| {
                                    view.add_message(MessageRole::User);
                                    cx.notify();
                                });
                            }
                        }),
                )
                .child(
                    div()
                        .px(px(8.0))
                        .py(px(4.0))
                        .bg(style.action(ActionRole::Main).background)
                        .rounded(px(4.0))
                        .cursor(CursorStyle::PointingHand)
                        .hover(|s| s.bg(style.action(ActionRole::Main).hover))
                        .child(Icon::new(IconName::Bot).xsmall())
                        .on_mouse_down(MouseButton::Left, {
                            let entity = entity.clone();
                            move |_, _, cx| {
                                _ = entity.update(cx, |view, cx| {
                                    view.add_message(MessageRole::Assistant);
                                    cx.notify();
                                });
                            }
                        }),
                ),
        );

        // Tools 区域
        content = content.child(
            div().mt(px(16.0)).child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .mb(px(8.0))
                    .child(
                        div()
                            .text_size(px(14.0))
                            .font_weight(FontWeight::BOLD)
                            .child("Tools"),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(style.list.muted_foreground)
                            .child(format!("({})", self.tools.len())),
                    ),
            ),
        );

        // 渲染工具卡片
        let tools_snapshot: Vec<_> = self
            .tools
            .iter()
            .map(|t| {
                (
                    t.id,
                    t.name.clone(),
                    t.description.clone(),
                    t.parameters_json.clone(),
                )
            })
            .collect();
        for (tool_id, tool_name, tool_desc, tool_params) in tools_snapshot {
            let is_editing = self.editing_tools.get(&tool_id).copied().unwrap_or(false);
            let is_collapsed = self.collapsed_tools.get(&tool_id).copied().unwrap_or(false);

            let collapse_icon = if is_collapsed {
                IconName::ChevronRight
            } else {
                IconName::ChevronDown
            };

            let card_header = div()
                .flex()
                .items_center()
                .justify_between()
                .px(px(10.0))
                .py(px(6.0))
                .bg(style.action(ActionRole::Neutral).background)
                .child(
                    div()
                        .text_size(px(12.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(if tool_name.is_empty() {
                            SharedString::from("(未命名工具)")
                        } else {
                            SharedString::from(tool_name.as_str())
                        }),
                )
                .child(
                    div()
                        .flex()
                        .gap(px(4.0))
                        .child(
                            div()
                                .cursor(CursorStyle::PointingHand)
                                .hover(|s| s.opacity(0.7))
                                .child(Icon::new(collapse_icon).xsmall())
                                .on_mouse_down(MouseButton::Left, {
                                    let entity = entity.clone();
                                    move |_, _, cx| {
                                        _ = entity.update(cx, |view, cx| {
                                            view.toggle_collapse_tool(tool_id);
                                            cx.notify();
                                        });
                                    }
                                }),
                        )
                        .child(
                            div()
                                .cursor(CursorStyle::PointingHand)
                                .hover(|s| s.opacity(0.7))
                                .child(
                                    Icon::new(if is_editing {
                                        IconName::Check
                                    } else {
                                        IconName::Replace
                                    })
                                    .xsmall(),
                                )
                                .on_mouse_down(MouseButton::Left, {
                                    let entity = entity.clone();
                                    move |_, window, cx| {
                                        _ = entity.update(cx, |view, cx| {
                                            if is_editing {
                                                view.save_tool_edit(tool_id, cx);
                                            } else {
                                                view.toggle_tool_edit(tool_id);
                                            }
                                            cx.notify();
                                        });
                                    }
                                }),
                        )
                        .child(
                            div()
                                .cursor(CursorStyle::PointingHand)
                                .hover(|s| s.opacity(0.7))
                                .child(Icon::new(IconName::Close).xsmall())
                                .on_mouse_down(MouseButton::Left, {
                                    let entity = entity.clone();
                                    move |_, _, cx| {
                                        _ = entity.update(cx, |view, cx| {
                                            view.remove_tool(tool_id);
                                            cx.notify();
                                        });
                                    }
                                }),
                        ),
                );

            let card_body = if is_editing {
                let (name_input, desc_input, params_input) =
                    self.get_tool_inputs(tool_id, window, cx);
                div()
                    .px(px(10.0))
                    .py(px(8.0))
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(style.list.muted_foreground)
                            .child("名称:"),
                    )
                    .child(Input::new(&name_input))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(style.list.muted_foreground)
                            .child("描述:"),
                    )
                    .child(Input::new(&desc_input))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(style.list.muted_foreground)
                            .child("参数 (JSON Schema):"),
                    )
                    .child(Input::new(&params_input))
            } else {
                div()
                    .px(px(10.0))
                    .py(px(8.0))
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(style.list.muted_foreground)
                            .child("名称:"),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(style.list.foreground)
                            .child(if tool_name.is_empty() {
                                SharedString::from("(空)")
                            } else {
                                SharedString::from(tool_name.as_str())
                            }),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(style.list.muted_foreground)
                            .child("描述:"),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(style.list.foreground)
                            .child(if tool_desc.is_empty() {
                                SharedString::from("(空)")
                            } else {
                                SharedString::from(tool_desc.as_str())
                            }),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(style.list.muted_foreground)
                            .child("参数 (JSON Schema):"),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(style.list.foreground)
                            .font_family("monospace")
                            .whitespace_normal()
                            .child(SharedString::from(tool_params.as_str())),
                    )
            };

            let mut tool_card = div()
                .id(SharedString::from(format!("tool-card-{}", tool_id)))
                .mb(px(8.0))
                .border_1()
                .border_color(style.list.border)
                .rounded(px(6.0))
                .overflow_hidden()
                .child(card_header);

            if !is_collapsed {
                tool_card = tool_card.child(card_body);
            }

            content = content.child(tool_card);
        }

        // 添加工具按钮
        content = content.child(
            div()
                .flex()
                .gap(px(6.0))
                .mt(px(4.0))
                .child(
                    div()
                        .px(px(8.0))
                        .py(px(4.0))
                        .bg(style.action(ActionRole::Main).background)
                        .rounded(px(4.0))
                        .cursor(CursorStyle::PointingHand)
                        .hover(|s| s.bg(style.action(ActionRole::Main).hover))
                        .child(Icon::new(IconName::Plus).xsmall())
                        .on_mouse_down(MouseButton::Left, {
                            let entity = entity.clone();
                            move |_, _, cx| {
                                _ = entity.update(cx, |view, cx| {
                                    view.add_tool();
                                    cx.notify();
                                });
                            }
                        }),
                )
                .child(
                    div()
                        .px(px(8.0))
                        .py(px(4.0))
                        .bg(style.action(ActionRole::Neutral).background)
                        .rounded(px(4.0))
                        .cursor(CursorStyle::PointingHand)
                        .hover(|s| s.bg(style.action(ActionRole::Neutral).hover))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(4.0))
                                .child(Icon::new(IconName::Settings2).xsmall())
                                .child(div().text_size(px(11.0)).child("从函数管理选择")),
                        )
                        .on_mouse_down(MouseButton::Left, {
                            let entity = entity.clone();
                            move |_, _, cx| {
                                _ = entity.update(cx, |view, cx| {
                                    view.toggle_tool_picker();
                                    cx.notify();
                                });
                            }
                        }),
                ),
        );

        // 执行按钮（在滚动区域末尾）
        content = content.child(execute_button(&self.call_state, style, entity.clone()));

        content
    }
}

/// 执行按钮
fn execute_button(
    call_state: &CallState,
    style: &ManagementStyle,
    entity: Entity<PromptDebugger>,
) -> impl IntoElement {
    let is_loading = matches!(call_state, CallState::Loading);
    div().mt(px(12.0)).child(
        div()
            .id("execute-btn")
            .px(px(16.0))
            .py(px(8.0))
            .bg(if is_loading {
                style.action(ActionRole::Disabled).background
            } else {
                style.action(ActionRole::Main).background
            })
            .rounded(px(6.0))
            .cursor(if is_loading {
                CursorStyle::Arrow
            } else {
                CursorStyle::PointingHand
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(if is_loading {
                        Icon::new(IconName::LoaderCircle).small()
                    } else {
                        Icon::new(IconName::Play).small()
                    })
                    .child(div().text_size(px(13.0)).child(if is_loading {
                        "执行中..."
                    } else {
                        "执行"
                    })),
            )
            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                if !is_loading {
                    _ = entity.update(cx, |view, cx| {
                        view.execute_call(cx);
                    });
                }
            }),
    )
}

/// 右侧面板（历史 + 当前结果合并）
fn right_panel(
    call_state: &CallState,
    selected_count: usize,
    history_count: usize,
    history: &[ExecutionRecord],
    selected_ids: &HashSet<u64>,
    context_menu: &Option<(u64, gpui::Point<Pixels>)>,
    style: &ManagementStyle,
    entity: Entity<PromptDebugger>,
) -> impl IntoElement {
    let mut panel = div()
        .w(px(320.0))
        .flex_shrink_0()
        .h_full()
        .p(px(12.0))
        .border_l_1()
        .border_color(style.list.border)
        .overflow_y_scrollbar()
        .flex()
        .flex_col()
        .gap(px(8.0));

    // 当前结果区域
    let result_section = match call_state {
        CallState::Idle => div()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .h(px(80.0))
            .text_color(style.list.muted_foreground)
            .text_size(px(12.0))
            .child("点击「执行」按钮查看响应"),
        CallState::Loading => div()
            .flex()
            .items_center()
            .justify_center()
            .h(px(80.0))
            .text_color(style.list.muted_foreground)
            .text_size(px(12.0))
            .child("加载中..."),
        CallState::Success(text) => {
            let display = if text.chars().count() > 500 {
                format!("{}...", text.chars().take(500).collect::<String>())
            } else {
                text.clone()
            };
            div()
                .p(px(8.0))
                .border_1()
                .border_color(style.action(ActionRole::Edit).background)
                .rounded(px(6.0))
                .text_size(px(12.0))
                .whitespace_normal()
                .child(SharedString::from(display.as_str()))
        }
        CallState::Error(err) => div()
            .p(px(8.0))
            .border_1()
            .border_color(style.action(ActionRole::Delete).foreground)
            .rounded(px(6.0))
            .text_size(px(12.0))
            .text_color(style.action(ActionRole::Delete).foreground)
            .child(err.clone()),
    };

    panel = panel.child(
        div()
            .flex()
            .items_center()
            .justify_between()
            .mb(px(4.0))
            .child(
                div()
                    .text_size(px(14.0))
                    .font_weight(FontWeight::BOLD)
                    .child("历史"),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(style.list.muted_foreground)
                    .child(format!("({} 条)", history_count)),
            ),
    );

    panel = panel.child(result_section);

    // 操作栏
    let toolbar = div()
        .flex()
        .items_center()
        .gap(px(6.0))
        .child(
            div()
                .px(px(8.0))
                .py(px(4.0))
                .bg(style.action(ActionRole::Neutral).background)
                .rounded(px(4.0))
                .cursor(CursorStyle::PointingHand)
                .text_size(px(11.0))
                .text_color(style.action(ActionRole::Neutral).foreground)
                .child("全选")
                .on_mouse_down(MouseButton::Left, {
                    let entity = entity.clone();
                    move |_, _, cx| {
                        _ = entity.update(cx, |view, cx| {
                            view.toggle_select_all();
                            cx.notify();
                        });
                    }
                }),
        )
        .child(
            div()
                .px(px(8.0))
                .py(px(4.0))
                .bg(style.action(ActionRole::Delete).background)
                .rounded(px(4.0))
                .cursor(CursorStyle::PointingHand)
                .text_size(px(11.0))
                .text_color(style.action(ActionRole::Delete).foreground)
                .child("清除")
                .on_mouse_down(MouseButton::Left, {
                    let entity = entity.clone();
                    move |_, _, cx| {
                        _ = entity.update(cx, |view, cx| {
                            view.clear_history();
                            cx.notify();
                        });
                    }
                }),
        );

    let toolbar = if selected_count >= 2 {
        toolbar.child(
            div()
                .ml(px(4.0))
                .px(px(8.0))
                .py(px(4.0))
                .bg(style.action(ActionRole::Main).background)
                .rounded(px(4.0))
                .cursor(CursorStyle::PointingHand)
                .text_size(px(11.0))
                .text_color(style.action(ActionRole::Main).foreground)
                .child(format!("对比({})", selected_count))
                .on_mouse_down(MouseButton::Left, {
                    let entity = entity.clone();
                    move |_, _, cx| {
                        _ = entity.update(cx, |view, cx| {
                            view.start_comparison();
                            cx.notify();
                        });
                    }
                }),
        )
    } else {
        toolbar
    };

    panel = panel.child(toolbar);

    // 历史列表
    if history.is_empty() {
        panel = panel.child(
            div()
                .flex()
                .items_center()
                .justify_center()
                .h(px(100.0))
                .text_color(style.list.muted_foreground)
                .text_size(px(12.0))
                .child("暂无历史记录"),
        );
    } else {
        for record in history.iter().rev() {
            let is_selected = selected_ids.contains(&record.id);
            let status_icon = match &record.result {
                CallState::Success(_) => "✓",
                CallState::Error(_) => "",
                _ => "○",
            };
            let status_color = match &record.result {
                CallState::Success(_) => style.action(ActionRole::Edit).background,
                CallState::Error(_) => style.action(ActionRole::Delete).foreground,
                _ => style.list.muted_foreground,
            };

            let msg_summary = record
                .messages
                .first()
                .map(|m| {
                    let content = if m.content.chars().count() > 30 {
                        format!("{}...", m.content.chars().take(30).collect::<String>())
                    } else {
                        m.content.clone()
                    };
                    format!("[{}] {}", m.role.label(), content)
                })
                .unwrap_or_default();

            let time_str = {
                let secs = record.timestamp;
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let diff = now.saturating_sub(secs);
                if diff < 60 {
                    "刚刚".to_string()
                } else if diff < 3600 {
                    format!("{}分钟前", diff / 60)
                } else if diff < 86400 {
                    format!("{}小时前", diff / 3600)
                } else {
                    format!("{}天前", diff / 86400)
                }
            };

            panel = panel.child(
                div()
                    .id(SharedString::from(format!("history-record-{}", record.id)))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .px(px(8.0))
                    .py(px(6.0))
                    .rounded(px(4.0))
                    .bg(if is_selected {
                        style.action(ActionRole::Main).background.opacity(0.2)
                    } else {
                        style.list.row
                    })
                    .border_1()
                    .border_color(if is_selected {
                        style.action(ActionRole::Main).background
                    } else {
                        style.list.border
                    })
                    .cursor(CursorStyle::PointingHand)
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(status_color)
                            .child(status_icon),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(4.0))
                                    .child(
                                        div()
                                            .text_size(px(11.0))
                                            .text_color(style.list.muted_foreground)
                                            .child(SharedString::from(time_str.as_str())),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.0))
                                            .text_color(style.list.muted_foreground)
                                            .child(SharedString::from(record.model_name.as_str())),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(style.list.foreground)
                                    .truncate()
                                    .child(SharedString::from(msg_summary.as_str())),
                            ),
                    )
                    .on_mouse_down(MouseButton::Left, {
                        let entity = entity.clone();
                        let record_id = record.id;
                        move |_, _, cx| {
                            _ = entity.update(cx, |view, cx| {
                                view.toggle_record_selection(record_id);
                                cx.notify();
                            });
                        }
                    })
                    .on_mouse_down(MouseButton::Right, {
                        let entity = entity.clone();
                        let record_id = record.id;
                        move |event: &MouseDownEvent, _, cx| {
                            _ = entity.update(cx, |view, cx| {
                                view.show_context_menu(record_id, event.position);
                                cx.notify();
                            });
                        }
                    }),
            );
        }
    }

    // 右键菜单
    if let Some((record_id, position)) = context_menu {
        panel = panel.child(
            div()
                .absolute()
                .top(position.y)
                .left(position.x)
                .w(px(120.0))
                .bg(style.list.row)
                .border_1()
                .border_color(style.list.border)
                .rounded(px(6.0))
                .shadow_md()
                .p(px(4.0))
                .on_mouse_down(MouseButton::Left, {
                    let entity = entity.clone();
                    move |_, _, cx| {
                        _ = entity.update(cx, |view, cx| {
                            view.hide_context_menu();
                            cx.notify();
                        });
                    }
                })
                .child(
                    div()
                        .px(px(8.0))
                        .py(px(6.0))
                        .rounded(px(4.0))
                        .cursor(CursorStyle::PointingHand)
                        .hover(|s| s.bg(style.action(ActionRole::Neutral).hover))
                        .text_size(px(12.0))
                        .child("查看")
                        .on_mouse_down(MouseButton::Left, {
                            let entity = entity.clone();
                            let record_id = *record_id;
                            move |_, _, cx| {
                                _ = entity.update(cx, |view, cx| {
                                    view.open_view_record(record_id);
                                    cx.notify();
                                });
                            }
                        }),
                )
                .child(
                    div()
                        .mt(px(4.0))
                        .px(px(8.0))
                        .py(px(6.0))
                        .rounded(px(4.0))
                        .cursor(CursorStyle::PointingHand)
                        .hover(|s| s.bg(style.action(ActionRole::Neutral).hover))
                        .text_size(px(12.0))
                        .child("回填")
                        .on_mouse_down(MouseButton::Left, {
                            let entity = entity.clone();
                            let record_id = *record_id;
                            move |_, window, cx| {
                                _ = entity.update(cx, |view, cx| {
                                    // 先克隆记录，释放不可变借用
                                    if let Some(record) = view
                                        .execution_history
                                        .iter()
                                        .find(|r| r.id == record_id)
                                        .cloned()
                                    {
                                        view.apply_record(&record, window, cx);
                                    }
                                    cx.notify();
                                });
                            }
                        }),
                ),
        );
    }

    panel
}

/// 单条记录卡片（查看模式）
fn single_record_card(record: &ExecutionRecord, style: &ManagementStyle) -> impl IntoElement {
    let status_text = match &record.result {
        CallState::Success(text) => text.clone(),
        CallState::Error(err) => err.clone(),
        _ => "无结果".to_string(),
    };
    let msg_summary = record
        .messages
        .iter()
        .map(|m| format!("[{}] {}", m.role.label(), m.content))
        .collect::<Vec<_>>()
        .join("\n");
    let tools_summary = if record.tools.is_empty() {
        "无工具".to_string()
    } else {
        record
            .tools
            .iter()
            .map(|t| format!("{}: {}", t.name, t.description))
            .collect::<Vec<_>>()
            .join("\n")
    };

    div()
        .p(px(12.0))
        .border_1()
        .border_color(style.list.border)
        .rounded(px(6.0))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(
                            div()
                                .text_size(px(14.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(SharedString::from(record.model_name.as_str())),
                        )
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(style.list.muted_foreground)
                                .child(format!(
                                    "T={:.2} Max={} TopP={:.2}",
                                    record.temperature, record.max_tokens, record.top_p
                                )),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.0))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(style.list.muted_foreground)
                                .child("消息:"),
                        )
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(style.list.foreground)
                                .whitespace_normal()
                                .child(SharedString::from(msg_summary.as_str())),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.0))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(style.list.muted_foreground)
                                .child("工具:"),
                        )
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(style.list.foreground)
                                .whitespace_normal()
                                .child(SharedString::from(tools_summary.as_str())),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.0))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(style.list.muted_foreground)
                                .child("结果:"),
                        )
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(style.list.foreground)
                                .whitespace_normal()
                                .child(SharedString::from(status_text.as_str())),
                        ),
                ),
        )
}

/// 获取记录在某个维度的文本值
fn get_dimension_value(record: &ExecutionRecord, dim: &str) -> String {
    match dim {
        "模型" => record.model_name.clone(),
        "参数" => format!(
            "T={:.2} Max={} TopP={:.2}",
            record.temperature, record.max_tokens, record.top_p
        ),
        "消息" => record
            .messages
            .iter()
            .map(|m| format!("[{}] {}", m.role.label(), m.content))
            .collect::<Vec<_>>()
            .join("\n"),
        "工具" => {
            if record.tools.is_empty() {
                "无工具".to_string()
            } else {
                record
                    .tools
                    .iter()
                    .map(|t| format!("{}: {}", t.name, t.description))
                    .collect::<Vec<_>>()
                    .join("\n")
            }
        }
        "结果" => match &record.result {
            CallState::Success(t) => t.clone(),
            CallState::Error(e) => e.clone(),
            _ => "无结果".to_string(),
        },
        _ => String::new(),
    }
}

/// 检查某个维度在所有记录中是否完全相同
fn all_same(records: &[ExecutionRecord], dim: &str) -> bool {
    if records.len() < 2 {
        return true;
    }
    let first = get_dimension_value(&records[0], dim);
    records
        .iter()
        .skip(1)
        .all(|r| get_dimension_value(r, dim) == first)
}

/// 对比表格（2+ 条记录）
/// 第 1 列：对比维度，后续列：每条记录的数据，最多 20 列
fn comparison_table(
    records: &[ExecutionRecord],
    style: &ManagementStyle,
    show_only_diff: bool,
) -> impl IntoElement {
    let col_count = records.len().min(20) + 1; // +1 为维度列
    let col_width = px(300.0);
    let label_col_width = px(100.0);
    let table_width = label_col_width + col_width * (col_count - 1) as f32;

    // 维度标签
    let dimensions: Vec<&str> = vec!["模型", "参数", "消息", "工具", "结果"];

    // 表头行
    let header_row = div()
        .flex()
        .border_b_1()
        .border_color(style.list.border)
        .child(
            div()
                .w(label_col_width)
                .flex_shrink_0()
                .px(px(8.0))
                .py(px(8.0))
                .text_size(px(12.0))
                .font_weight(FontWeight::BOLD)
                .text_color(style.list.foreground)
                .child("维度"),
        );
    let header_row = records.iter().take(20).fold(header_row, |acc, record| {
        acc.child(
            div()
                .w(col_width)
                .flex_shrink_0()
                .px(px(8.0))
                .py(px(8.0))
                .text_size(px(12.0))
                .font_weight(FontWeight::BOLD)
                .text_color(style.list.foreground)
                .truncate()
                .child(SharedString::from(record.model_name.as_str())),
        )
    });

    // 数据行
    let mut table = div().w(table_width).flex().flex_col().child(header_row);

    for dim in &dimensions {
        // 仅看差异模式：跳过所有记录值相同的维度
        if show_only_diff && all_same(records, dim) {
            continue;
        }

        let row = div()
            .flex()
            .border_b_1()
            .border_color(style.list.border)
            .child(
                div()
                    .w(label_col_width)
                    .flex_shrink_0()
                    .px(px(8.0))
                    .py(px(8.0))
                    .text_size(px(12.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(style.list.muted_foreground)
                    .child(*dim),
            );

        let row = records.iter().take(20).fold(row, |acc, record| {
            let cell_text = SharedString::from(get_dimension_value(record, dim));
            acc.child(
                div()
                    .w(col_width)
                    .flex_shrink_0()
                    .px(px(8.0))
                    .py(px(8.0))
                    .text_size(px(12.0))
                    .text_color(style.list.foreground)
                    .whitespace_normal()
                    .child(cell_text),
            )
        });

        table = table.child(row);
    }

    table
}

/// 查看/对比模态框
fn view_modal(
    records: &[ExecutionRecord],
    style: &ManagementStyle,
    show_only_diff: bool,
    entity: Entity<PromptDebugger>,
) -> impl IntoElement {
    let is_comparison = records.len() > 1;
    let title = if is_comparison {
        format!("对比 ({} 条记录)", records.len())
    } else {
        "查看执行记录".to_string()
    };

    // 对比模式：最大宽度根据记录数动态调整，最多 20 列
    let modal_width = if is_comparison {
        let cols = records.len().min(20);
        px(200.0 + cols as f32 * 300.0)
    } else {
        px(800.0)
    };

    div()
        .absolute()
        .top_0()
        .bottom_0()
        .left_0()
        .right_0()
        .bg(gpui::rgba(0x00000080))
        .flex()
        .items_center()
        .justify_center()
        .on_mouse_down(MouseButton::Left, {
            let entity = entity.clone();
            move |_, _, cx| {
                _ = entity.update(cx, |view, cx| {
                    view.close_view_modal();
                    cx.notify();
                });
            }
        })
        .child(
            div()
                .w(modal_width)
                .max_h(px(600.0))
                .bg(style.list.row)
                .rounded(px(12.0))
                .shadow_lg()
                .border_1()
                .border_color(style.list.border)
                .flex()
                .flex_col()
                .overflow_hidden()
                .on_mouse_down(MouseButton::Left, |_, _, _| {})
                .child(
                    // 标题栏
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .px(px(16.0))
                        .py(px(12.0))
                        .border_b_1()
                        .border_color(style.list.border)
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(12.0))
                                .child(
                                    div()
                                        .text_size(px(16.0))
                                        .font_weight(FontWeight::BOLD)
                                        .child(title),
                                )
                                // 仅看差异复选框（仅对比模式显示）
                                .child(if is_comparison {
                                    let checked = show_only_diff;
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(4.0))
                                        .cursor(CursorStyle::PointingHand)
                                        .on_mouse_down(MouseButton::Left, {
                                            let entity = entity.clone();
                                            move |_, _, cx| {
                                                _ = entity.update(cx, |view, cx| {
                                                    view.toggle_show_only_diff();
                                                    cx.notify();
                                                });
                                            }
                                        })
                                        .child(
                                            div()
                                                .w(px(14.0))
                                                .h(px(14.0))
                                                .border_1()
                                                .border_color(style.list.border)
                                                .rounded(px(2.0))
                                                .bg(if checked {
                                                    style.action(ActionRole::Main).background
                                                } else {
                                                    style.list.row
                                                })
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .child(if checked {
                                                    div()
                                                        .text_size(px(10.0))
                                                        .text_color(gpui::white())
                                                        .child("✓")
                                                } else {
                                                    div()
                                                }),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(12.0))
                                                .text_color(style.list.muted_foreground)
                                                .child("仅看差异"),
                                        )
                                        .into_any_element()
                                } else {
                                    div().into_any_element()
                                }),
                        )
                        .child(
                            div()
                                .cursor(CursorStyle::PointingHand)
                                .hover(|s| s.opacity(0.7))
                                .child(Icon::new(IconName::Close).small())
                                .on_mouse_down(MouseButton::Left, {
                                    let entity = entity.clone();
                                    move |_, _, cx| {
                                        _ = entity.update(cx, |view, cx| {
                                            view.close_view_modal();
                                            cx.notify();
                                        });
                                    }
                                }),
                        ),
                )
                .child(
                    // 内容区域
                    div()
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scrollbar()
                        .px(px(16.0))
                        .py(px(12.0))
                        .child(if is_comparison {
                            comparison_table(records, style, show_only_diff).into_any_element()
                        } else {
                            single_record_card(&records[0], style).into_any_element()
                        }),
                ),
        )
}

// ──────────────────────────────────────────────
// API 调用
// ──────────────────────────────────────────────

impl PromptDebugger {
    fn execute_call(&mut self, cx: &mut Context<Self>) {
        // 从 InputState 读取最新参数值
        if let Some(ref input) = self.temp_input {
            if let Ok(v) = input.read(cx).text().to_string().parse::<f64>() {
                self.temperature = v;
            }
        }
        if let Some(ref input) = self.max_tokens_input {
            if let Ok(v) = input.read(cx).text().to_string().parse::<u32>() {
                self.max_tokens = v;
            }
        }
        if let Some(ref input) = self.top_p_input {
            if let Ok(v) = input.read(cx).text().to_string().parse::<f64>() {
                self.top_p = v;
            }
        }
        if let Some(ref input) = self.presence_penalty_input {
            if let Ok(v) = input.read(cx).text().to_string().parse::<f64>() {
                self.presence_penalty = v;
            }
        }
        if let Some(ref input) = self.frequency_penalty_input {
            if let Ok(v) = input.read(cx).text().to_string().parse::<f64>() {
                self.frequency_penalty = v;
            }
        }
        if let Some(ref input) = self.thinking_budget_input {
            if let Ok(v) = input.read(cx).text().to_string().parse::<u32>() {
                self.thinking_budget_tokens = v;
            }
        }

        let model = match self
            .selected_model_id
            .and_then(|id| self.models.iter().find(|m| m.id == id))
        {
            Some(m) => m.clone(),
            None => {
                self.call_state = CallState::Error("请先选择一个模型".into());
                cx.notify();
                return;
            }
        };

        let provider = model
            .provider_id
            .and_then(|pid| self.providers.iter().find(|p| p.id == pid))
            .cloned();

        let base_url = provider
            .as_ref()
            .map(|p| p.base_url.clone())
            .unwrap_or_default();

        let token = provider.as_ref().and_then(|p| {
            p.token_encrypted
                .as_ref()
                .and_then(|enc| self.crypto.decrypt(enc).ok())
                .and_then(|bytes| String::from_utf8(bytes).ok())
        });

        if base_url.is_empty() {
            self.call_state = CallState::Error("所选模型未配置 Provider base_url".into());
            cx.notify();
            return;
        }

        // 构建消息体
        let messages: Vec<serde_json::Value> = self
            .messages
            .iter()
            .map(|m| {
                serde_json::json!({
                    "role": m.role.api_role(),
                    "content": m.content,
                })
            })
            .collect();

        // 思考模式下不发送 temperature、top_p 等参数
        let mut body = serde_json::json!({
            "model": model.name,
            "messages": messages,
            "max_tokens": self.max_tokens,
        });

        // 添加 tools（Function call）
        if !self.tools.is_empty() {
            // 检测是否为 Anthropic 格式（根据 provider 名称判断）
            let is_anthropic = provider
                .as_ref()
                .map(|p| p.name.to_lowercase().contains("anthropic"))
                .unwrap_or(false);

            if is_anthropic {
                // Anthropic 格式：tools 数组直接包含 name, description, input_schema
                let tools_json: Vec<serde_json::Value> = self
                    .tools
                    .iter()
                    .filter_map(|t| {
                        let schema: serde_json::Value =
                            serde_json::from_str(&t.parameters_json).ok()?;
                        Some(serde_json::json!({
                            "name": t.name,
                            "description": t.description,
                            "input_schema": schema,
                        }))
                    })
                    .collect();
                if !tools_json.is_empty() {
                    body["tools"] = serde_json::json!(tools_json);
                }
            } else {
                // OpenAI 格式：tools 数组包含 type: "function" 和 function 对象
                let tools_json: Vec<serde_json::Value> = self
                    .tools
                    .iter()
                    .filter_map(|t| {
                        let schema: serde_json::Value =
                            serde_json::from_str(&t.parameters_json).ok()?;
                        Some(serde_json::json!({
                            "type": "function",
                            "function": {
                                "name": t.name,
                                "description": t.description,
                                "parameters": schema,
                            }
                        }))
                    })
                    .collect();
                if !tools_json.is_empty() {
                    body["tools"] = serde_json::json!(tools_json);
                }
            }
        }

        if self.thinking_enabled {
            body["thinking"] = serde_json::json!({
                "type": "enabled",
                "budget_tokens": self.thinking_budget_tokens,
            });
        } else {
            body["thinking"] = serde_json::json!({"type": "disabled"});
            body["temperature"] = serde_json::json!(self.temperature);
            body["top_p"] = serde_json::json!(self.top_p);
            body["presence_penalty"] = serde_json::json!(self.presence_penalty);
            body["frequency_penalty"] = serde_json::json!(self.frequency_penalty);
        }

        self.call_state = CallState::Loading;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let client = reqwest::Client::new();
            let url = if base_url.ends_with("/v1") || base_url.ends_with("/v1/") {
                if base_url.ends_with('/') {
                    format!("{}chat/completions", base_url)
                } else {
                    format!("{}/chat/completions", base_url)
                }
            } else if base_url.ends_with('/') {
                format!("{}v1/chat/completions", base_url)
            } else {
                format!("{}/v1/chat/completions", base_url)
            };

            let mut request = client.post(&url).header("Content-Type", "application/json");

            if let Some(ref token) = token {
                request = request.header("Authorization", format!("Bearer {}", token));
            }

            let result = request.json(&body).send().await;

            let response_text = match result {
                Ok(resp) => {
                    let status = resp.status();
                    let text = resp.text().await.unwrap_or_default();
                    if status.is_success() {
                        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                            let message = json
                                .get("choices")
                                .and_then(|c| c.as_array())
                                .and_then(|arr| arr.first())
                                .and_then(|first| first.get("message"));

                            let content = message
                                .and_then(|msg| msg.get("content"))
                                .and_then(|c| c.as_str())
                                .unwrap_or("")
                                .to_string();

                            // 思考模式下提取 reasoning_content
                            let reasoning = message
                                .and_then(|msg| msg.get("reasoning_content"))
                                .and_then(|c| c.as_str())
                                .unwrap_or("")
                                .to_string();

                            if reasoning.is_empty() {
                                Ok(content)
                            } else {
                                Ok(format!(
                                    "【思考过程】\n{}\n\n【最终回复】\n{}",
                                    reasoning, content
                                ))
                            }
                        } else {
                            Ok(text)
                        }
                    } else {
                        Err(format!("HTTP {}: {}", status, text))
                    }
                }
                Err(e) => Err(format!("请求失败: {}", e)),
            };

            _ = this.update(cx, |view, cx| {
                let is_success = matches!(&response_text, Ok(_));
                view.call_state = match response_text {
                    Ok(text) => CallState::Success(text),
                    Err(err) => CallState::Error(err),
                };
                view.save_execution_record();
                // 执行成功时自动弹出查看窗口
                if is_success {
                    if let Some(last_record) = view.execution_history.last() {
                        view.view_records = vec![last_record.clone()];
                        view.show_view_modal = true;
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}

// ──────────────────────────────────────────────
// Render
// ──────────────────────────────────────────────

impl Render for PromptDebugger {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let style = ManagementStyle::from_theme(cx.theme());
        self.style = style;

        // 懒初始化 InputState
        if self.temp_input.is_none() {
            self.temp_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("0.70")
                    .default_value("0.70")
            }));
        }
        if self.max_tokens_input.is_none() {
            self.max_tokens_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("2048")
                    .default_value("2048")
            }));
        }
        if self.top_p_input.is_none() {
            self.top_p_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("1.00")
                    .default_value("1.00")
            }));
        }
        if self.presence_penalty_input.is_none() {
            self.presence_penalty_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("0.0")
                    .default_value("0.0")
            }));
        }
        if self.frequency_penalty_input.is_none() {
            self.frequency_penalty_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("0.0")
                    .default_value("0.0")
            }));
        }
        if self.thinking_budget_input.is_none() {
            self.thinking_budget_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("1024")
                    .default_value("1024")
            }));
        }

        // 懒初始化模型下拉列表
        self.ensure_model_select_state(window, cx);

        let temp_input = self.temp_input.clone().unwrap();
        let max_tokens_input = self.max_tokens_input.clone().unwrap();
        let top_p_input = self.top_p_input.clone().unwrap();
        let presence_penalty_input = self.presence_penalty_input.clone().unwrap();
        let frequency_penalty_input = self.frequency_penalty_input.clone().unwrap();
        let thinking_budget_input = self.thinking_budget_input.clone().unwrap();
        let model_select_state = self.model_select_state.clone().unwrap();

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .min_h_0()
                    .child(settings_panel(
                        &model_select_state,
                        &temp_input,
                        &max_tokens_input,
                        &top_p_input,
                        &presence_penalty_input,
                        &frequency_penalty_input,
                        &thinking_budget_input,
                        self.thinking_enabled,
                        cx.entity(),
                        &style,
                    ))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .child(self.message_editor(
                                window,
                                cx,
                                &style,
                                cx.entity(),
                            )),
                    )
                    .child(right_panel(
                        &self.call_state,
                        self.selected_record_ids.len(),
                        self.execution_history.len(),
                        &self.execution_history,
                        &self.selected_record_ids,
                        &self.context_menu,
                        &style,
                        cx.entity(),
                    )),
            )
            .child(if self.show_view_modal {
                view_modal(
                    &self.view_records,
                    &style,
                    self.show_only_diff,
                    cx.entity(),
                ).into_any_element()
            } else {
                div().into_any_element()
            })
            .child(if self.show_tool_picker {
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left_0()
                    .right_0()
                    .bg(gpui::rgba(0x00000080))
                    .flex()
                    .items_center()
                    .justify_center()
                    .on_mouse_down(MouseButton::Left, {
                        let entity = cx.entity();
                        move |_, _, cx| {
                            _ = entity.update(cx, |view, cx| {
                                view.show_tool_picker = false;
                                cx.notify();
                            });
                        }
                    })
                    .child(
                        div()
                            .w(px(600.0))
                            .max_h(px(500.0))
                            .bg(cx.theme().background)
                            .rounded(px(12.0))
                            .shadow_lg()
                            .border_1()
                            .border_color(style.list.border)
                            .flex()
                            .flex_col()
                            .overflow_hidden()
                            .on_mouse_down(MouseButton::Left, |_, _, _| {})
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .px(px(16.0))
                                    .py(px(12.0))
                                    .border_b_1()
                                    .border_color(style.list.border)
                                    .child(
                                        div()
                                            .text_size(px(16.0))
                                            .font_weight(FontWeight::BOLD)
                                            .child("从函数管理选择工具"),
                                    )
                                    .child(
                                        div()
                                            .cursor(CursorStyle::PointingHand)
                                            .hover(|s| s.opacity(0.7))
                                            .child(Icon::new(IconName::Close).small())
                                            .on_mouse_down(MouseButton::Left, {
                                                let entity = cx.entity();
                                                move |_, _, cx| {
                                                    _ = entity.update(cx, |view, cx| {
                                                        view.show_tool_picker = false;
                                                        cx.notify();
                                                    });
                                                }
                                            }),
                                    ),
                            )
                            .child(
                                div()
                                    .px(px(16.0))
                                    .py(px(12.0))
                                    .border_b_1()
                                    .border_color(style.list.border)
                                    .child(
                                        div()
                                            .w_full()
                                            .child(
                                                Input::new(
                                                    &cx.new(|cx| {
                                                        InputState::new(window, cx)
                                                            .placeholder("搜索工具名称...")
                                                    }),
                                                ),
                                            ),
                                    ),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_h_0()
                                    .overflow_y_scrollbar()
                                    .px(px(16.0))
                                    .py(px(8.0))
                                    .child(
                                            self.available_tools
                                                .iter()
                                                .filter(|t| {
                                                    self.tool_picker_search.is_empty()
                                                        || t.name
                                                            .to_lowercase()
                                                            .contains(&self.tool_picker_search.to_lowercase())
                                                })
                                                .fold(div(), |acc, tool| {
                                                    let tool_name = tool.name.clone();
                                                    let tool_identifier = tool.identifier.clone();
                                                    let tool_description = tool.description.clone();
                                                    let tool_id = tool.id;
                                                    acc.child(
                                                        div()
                                                            .id(SharedString::from(format!(
                                                                "picker-tool-{}",
                                                                tool_id
                                                            )))
                                                            .mb(px(8.0))
                                                            .p(px(12.0))
                                                            .border_1()
                                                            .border_color(style.list.border)
                                                            .rounded(px(6.0))
                                                            .cursor(CursorStyle::PointingHand)
                                                            .hover(|s| {
                                                                s.bg(style.action(ActionRole::Neutral).hover)
                                                            })
                                                            .on_mouse_down(MouseButton::Left, {
                                                                let entity = cx.entity();
                                                                move |_, _, cx| {
                                                                    _ = entity.update(
                                                                        cx,
                                                                        |view, cx| {
                                                                            view.add_tool_from_management(
                                                                                tool_id,
                                                                            );
                                                                            view.show_tool_picker = false;
                                                                            cx.notify();
                                                                        },
                                                                    );
                                                                }
                                                            })
                                                            .child(
                                                                div()
                                                                    .flex()
                                                                    .items_center()
                                                                    .justify_between()
                                                                    .mb(px(4.0))
                                                                    .child(
                                                                        div()
                                                                            .text_size(px(14.0))
                                                                            .font_weight(FontWeight::SEMIBOLD)
                                                                            .child(SharedString::from(tool_name.as_str())),
                                                                    )
                                                                    .child(
                                                                        div()
                                                                            .text_size(px(12.0))
                                                                            .text_color(
                                                                                style.list.muted_foreground,
                                                                            )
                                                                            .child(SharedString::from(tool_identifier.as_str())),
                                                                    ),
                                                            )
                                                            .child(
                                                                div()
                                                                    .text_size(px(12.0))
                                                                    .text_color(
                                                                        style.list.muted_foreground,
                                                                    )
                                                                    .child(SharedString::from(tool_description.as_str())),
                                                            ),
                                                    )
                                                }),
                                        ),
                            ),
                    )
                    .into_any_element()
            } else {
                div().into_any_element()
            })
    }
}

#[cfg(test)]
mod tests {
    use super::{DebugMessage, MessageRole};

    #[test]
    fn message_role_labels() {
        assert_eq!(MessageRole::System.label(), "System");
        assert_eq!(MessageRole::User.label(), "User");
        assert_eq!(MessageRole::Assistant.label(), "Assistant");
    }

    #[test]
    fn message_role_api_roles() {
        assert_eq!(MessageRole::System.api_role(), "system");
        assert_eq!(MessageRole::User.api_role(), "user");
        assert_eq!(MessageRole::Assistant.api_role(), "assistant");
    }

    #[test]
    fn add_and_remove_messages() {
        let mut msgs: Vec<DebugMessage> = vec![];
        let mut next_id = 1u64;

        let id = next_id;
        next_id += 1;
        msgs.push(DebugMessage {
            id,
            role: MessageRole::User,
            content: "hi".into(),
        });
        assert_eq!(msgs.len(), 1);

        msgs.retain(|m| m.id != id);
        assert_eq!(msgs.len(), 0);
    }

    #[test]
    fn move_message_up_and_down() {
        let mut msgs = vec![
            DebugMessage {
                id: 1,
                role: MessageRole::System,
                content: "a".into(),
            },
            DebugMessage {
                id: 2,
                role: MessageRole::User,
                content: "b".into(),
            },
            DebugMessage {
                id: 3,
                role: MessageRole::Assistant,
                content: "c".into(),
            },
        ];

        if let Some(pos) = msgs.iter().position(|m| m.id == 2) {
            if pos > 0 {
                msgs.swap(pos, pos - 1);
            }
        }
        assert_eq!(msgs[0].id, 2);
        assert_eq!(msgs[1].id, 1);

        if let Some(pos) = msgs.iter().position(|m| m.id == 2) {
            if pos + 1 < msgs.len() {
                msgs.swap(pos, pos + 1);
            }
        }
        assert_eq!(msgs[0].id, 1);
        assert_eq!(msgs[1].id, 2);
    }
}

#[cfg(test)]
mod geometry_tests {
    use gpui::{
        Context, InteractiveElement, IntoElement, ParentElement, Render, Styled, TestAppContext,
        VisualTestContext, Window, div, px, size,
    };

    struct PromptDebuggerTestView;

    impl Render for PromptDebuggerTestView {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .flex()
                .flex_row()
                .size_full()
                .child(
                    div()
                        .w(px(220.0))
                        .flex_shrink_0()
                        .debug_selector(|| "SETTINGS_PANEL".to_owned()),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .debug_selector(|| "MESSAGE_EDITOR".to_owned()),
                )
                .child(
                    div()
                        .w(px(320.0))
                        .flex_shrink_0()
                        .debug_selector(|| "RESULTS_PANEL".to_owned()),
                )
        }
    }

    #[gpui::test]
    fn three_column_layout_dimensions(cx: &mut TestAppContext) {
        let window = cx.open_window(size(px(1200.0), px(700.0)), |_, _| PromptDebuggerTestView);
        cx.run_until_parked();

        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let settings = cx
            .debug_bounds("SETTINGS_PANEL")
            .expect("settings panel bounds");
        let editor = cx
            .debug_bounds("MESSAGE_EDITOR")
            .expect("message editor bounds");
        let results = cx
            .debug_bounds("RESULTS_PANEL")
            .expect("results panel bounds");

        assert_eq!(settings.size.width, px(220.0));
        assert_eq!(results.size.width, px(320.0));
        assert_eq!(settings.right(), editor.left());
        assert_eq!(editor.right(), results.left());
        assert_eq!(settings.top(), results.top());
    }
}
