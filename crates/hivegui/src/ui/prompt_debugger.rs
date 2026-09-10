//! LLM 提示词调试工具 — 三栏布局：LLM 设置 / 消息编辑 / 结果展示。

use crate::datasource::entity_store::Tool as DbTool;
use crate::datasource::llm_store::{LlmModel, LlmPreset, LlmProvider, LlmStore};
use crate::datasource::{Crypto, Store};
use crate::ui::management_style::{ActionRole, ManagementStyle};
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::input::{Input, InputState, Textarea, TextareaState};
use gpui_component::notification::Notification;
use gpui_component::scroll::ScrollableElement;
use gpui_component::{
    ActiveTheme as _, Icon, IconName, Root, Sizable, WindowExt,
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

/// Preset 选择器条目
#[derive(Debug, Clone)]
struct PresetSelectItem {
    id: i64,
    label: String,
}

impl gpui_component::searchable_list::SearchableListItem for PresetSelectItem {
    type Value = i64;

    fn title(&self) -> SharedString {
        SharedString::from(self.label.clone())
    }

    fn value(&self) -> &Self::Value {
        &self.id
    }
}

fn models_for_preset(models: &[LlmModel], preset_id: Option<i64>) -> Vec<&LlmModel> {
    let Some(preset_id) = preset_id else {
        return Vec::new();
    };

    models
        .iter()
        .filter(|model| model.preset_id == Some(preset_id))
        .collect()
}

fn valid_model_selection(
    models: &[LlmModel],
    preset_id: Option<i64>,
    selected_model_id: Option<i64>,
) -> Option<i64> {
    selected_model_id.filter(|selected_model_id| {
        models_for_preset(models, preset_id)
            .iter()
            .any(|model| model.id == *selected_model_id)
    })
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

/// 持久化到 `global_configs` 的键名，用于记住左侧 LLM 设定表单。
const PROMPT_DEBUGGER_SETTINGS_KEY: &str = "prompt_debugger_state";

/// 历史记录列表每页显示的条数。
const HISTORY_PAGE_SIZE: usize = 10;

/// 左侧表单的持久化快照。
///
/// 仅记录「配置类」字段（Preset/Model 选择 + 数值参数 + 思考模式），
/// 不包含 Messages / Tools 这类会话内容，避免下次打开时误带入旧对话。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
struct SavedDebuggerSettings {
    selected_preset_id: Option<i64>,
    selected_model_id: Option<i64>,
    temperature: f64,
    max_tokens: u32,
    top_p: f64,
    presence_penalty: f64,
    frequency_penalty: f64,
    thinking_enabled: bool,
    thinking_budget_tokens: u32,
}

pub struct PromptDebugger {
    store: Entity<Store>,
    llm_store: LlmStore,
    crypto: Crypto,
    // 左侧：LLM 设置
    presets: Vec<LlmPreset>,
    models: Vec<LlmModel>,
    providers: Vec<LlmProvider>,
    selected_preset_id: Option<i64>,
    selected_model_id: Option<i64>,
    preset_select_state: Option<Entity<SelectState<SearchableVec<PresetSelectItem>>>>,
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
    tool_inputs: HashMap<
        u64,
        (
            Entity<InputState>,
            Entity<TextareaState>,
            Entity<TextareaState>,
        ),
    >, // name, desc, params
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
    message_inputs: HashMap<u64, Entity<TextareaState>>,
    // 执行历史
    execution_history: Vec<ExecutionRecord>,
    next_record_id: u64,
    selected_record_ids: HashSet<u64>,
    comparing_records: Vec<ExecutionRecord>,
    history_scroll: ScrollHandle,
    /// 历史记录列表当前页码（从 0 开始）。
    history_page: usize,
    // 右键菜单（记录 ID + 窗口坐标）
    context_menu: Option<(u64, Point<Pixels>)>,
    // 查看/对比弹出窗口
    show_view_modal: bool,
    view_records: Vec<ExecutionRecord>,
    view_scroll: ScrollHandle,
    // 弹窗内容 Tab：格式化 / JSON
    view_modal_tab: ViewModalTab,
    show_only_diff: bool,
    /// 查看/对比弹窗内「只读可选中文本」的状态缓存（key → TextareaState）。
    ///
    /// GPUI 0.2 的 `div` 文本不支持鼠标拖选（只有输入类组件可选中），
    /// 弹窗正文因此改用只读 `Textarea` 承载：外观仍是纯文本，但可以拖选、
    /// Ctrl/Cmd+C 复制、右键「复制 / 全选」。状态按 key 复用，避免每次重绘
    /// 都新建实体。
    view_text_states: HashMap<String, Entity<TextareaState>>,
    // 左侧表单校验提示（点击执行但左侧未设置时显示）
    form_error: Option<String>,
}

impl PromptDebugger {
    pub fn new(cx: &mut Context<Self>, store: Entity<Store>, llm_store: LlmStore) -> Self {
        let mut this = Self::new_unloaded(cx, store, llm_store);
        // 先尝试回填上次保存的左侧 LLM 设定，再加载 Preset/Model 列表。
        this.load_settings(cx);
        this.load_history();
        this
    }

    fn new_unloaded(cx: &mut Context<Self>, store: Entity<Store>, llm_store: LlmStore) -> Self {
        let crypto = llm_store.crypto().clone();
        let style = ManagementStyle::from_theme(cx.theme());
        let temp_input = None;
        let max_tokens_input = None;
        let top_p_input = None;
        let presence_penalty_input = None;
        let frequency_penalty_input = None;
        let thinking_budget_input = None;
        Self {
            store,
            llm_store,
            crypto,
            presets: vec![],
            models: vec![],
            providers: vec![],
            selected_preset_id: None,
            selected_model_id: None,
            preset_select_state: None,
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
            history_page: 0,
            context_menu: None,
            show_view_modal: false,
            view_records: vec![],
            view_scroll: ScrollHandle::default(),
            view_modal_tab: ViewModalTab::Formatted,
            show_only_diff: false,
            view_text_states: HashMap::new(),
            form_error: None,
        }
    }

    fn load_data(&mut self, cx: &mut Context<Self>) {
        self.load_available_tools(cx);
        self.reload_presets_and_models(cx);
    }

    /// 异步加载上一次保存的左侧 LLM 设定并回填表单。
    ///
    /// 读取到快照后先写回各字段，再触发 `load_data` 重新拉取
    /// Preset/Model 列表（列表加载完成后会根据回填的选中 ID 重建下拉）。
    fn load_settings(&mut self, cx: &mut Context<Self>) {
        let store = self.store.read(cx).clone();
        let key = PROMPT_DEBUGGER_SETTINGS_KEY.to_string();
        cx.spawn(async move |this, cx| {
            let saved = store.get_global_config(&key).await.ok().flatten();
            if let Some(json) = saved
                && let Ok(settings) = serde_json::from_str::<SavedDebuggerSettings>(&json)
            {
                _ = this.update(cx, |this, cx| {
                    this.apply_saved_settings(settings);
                    this.load_data(cx);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// 把持久化快照写回视图字段。数值输入框会在下次渲染时按字段值重建，
    /// 因此这里把它们置 `None` 以强制重建并显示回填值。
    fn apply_saved_settings(&mut self, settings: SavedDebuggerSettings) {
        self.selected_preset_id = settings.selected_preset_id;
        self.selected_model_id = settings.selected_model_id;
        self.temperature = settings.temperature;
        self.max_tokens = settings.max_tokens;
        self.top_p = settings.top_p;
        self.presence_penalty = settings.presence_penalty;
        self.frequency_penalty = settings.frequency_penalty;
        self.thinking_enabled = settings.thinking_enabled;
        self.thinking_budget_tokens = settings.thinking_budget_tokens;

        self.temp_input = None;
        self.max_tokens_input = None;
        self.top_p_input = None;
        self.presence_penalty_input = None;
        self.frequency_penalty_input = None;
        self.thinking_budget_input = None;
    }

    /// 把当前左侧 LLM 设定持久化，供下次打开时回填。
    fn save_settings(&self, cx: &mut Context<Self>) {
        let settings = SavedDebuggerSettings {
            selected_preset_id: self.selected_preset_id,
            selected_model_id: self.selected_model_id,
            temperature: self.temperature,
            max_tokens: self.max_tokens,
            top_p: self.top_p,
            presence_penalty: self.presence_penalty,
            frequency_penalty: self.frequency_penalty,
            thinking_enabled: self.thinking_enabled,
            thinking_budget_tokens: self.thinking_budget_tokens,
        };
        let Ok(json) = serde_json::to_string(&settings) else {
            return;
        };
        let store = self.store.read(cx).clone();
        // 记录本次写入命中的数据库文件，便于在报错时定位到具体库
        // （例如 `global_configs` 缺列导致的
        // "no column found for name: deletable"）。
        let database_path = store.database_path().to_path_buf();
        let key = PROMPT_DEBUGGER_SETTINGS_KEY.to_string();
        cx.spawn(async move |_, _| {
            if let Err(err) = store.upsert_global_config(&key, &key, "json", &json).await {
                tracing::error!(
                    target: "hivegui::ui::prompt_debugger",
                    operation = "save_settings",
                    outcome = "error",
                    config_key = %key,
                    database = %database_path.display(),
                    error = %err,
                );
            }
        })
        .detach();
    }

    /// 刷新 Preset/Model/Provider 列表。
    ///
    /// 适用于用户在其他页面（如 LLM 管理、数据源）对 Preset 或
    /// Model 做了变更后，切回本视图时重新拉取，避免下拉中残留
    /// 过时数据。
    pub fn reload_presets_and_models(&mut self, cx: &mut Context<Self>) {
        let llm_store = self.llm_store.clone();
        let current_preset_id = self.selected_preset_id;
        let current_model_id = self.selected_model_id;
        cx.spawn(async move |this, cx| {
            let presets = llm_store.list_presets().await.unwrap_or_default();
            let models = llm_store.list_models().await.unwrap_or_default();
            let providers = llm_store.list_providers().await.unwrap_or_default();
            _ = this.update(cx, |this, cx| {
                this.presets = presets;
                this.models = models;
                this.providers = providers;
                // 保留当前选中的 Preset/Model（若仍存在），否则清空以避免指向已删除项。
                if !this
                    .presets
                    .iter()
                    .any(|preset| Some(preset.id) == current_preset_id)
                {
                    this.selected_preset_id = None;
                } else {
                    this.selected_preset_id = current_preset_id;
                }
                this.selected_model_id =
                    valid_model_selection(&this.models, this.selected_preset_id, current_model_id);
                // 数据可能变化，让下拉控件按新列表重建。
                this.preset_select_state = None;
                this.model_select_state = None;
                this.loaded = true;
                cx.notify();
            });
        })
        .detach();
    }

    /// 加载可用工具列表，仅在初次进入视图时调用一次。
    fn load_available_tools(&mut self, cx: &mut Context<Self>) {
        let store = self.store.read(cx).clone();
        cx.spawn(async move |this, cx| {
            let available_tools = DbTool::list(store.pool(), None, 100, 0)
                .await
                .unwrap_or_default();
            _ = this.update(cx, |this, cx| {
                this.available_tools = available_tools;
                this.style = ManagementStyle::from_theme(cx.theme());
                cx.notify();
            });
        })
        .detach();
    }

    /// 懒初始化 Preset 下拉列表（需要 &mut Window）
    fn ensure_preset_select_state(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.preset_select_state.is_some() {
            return;
        }

        let items: SearchableVec<PresetSelectItem> = SearchableVec::new(
            self.presets
                .iter()
                .map(|preset| PresetSelectItem {
                    id: preset.id,
                    label: if preset.is_default != 0 {
                        format!("{}（默认）", preset.name)
                    } else {
                        preset.name.clone()
                    },
                })
                .collect::<Vec<_>>(),
        );
        let initial_index = self
            .selected_preset_id
            .and_then(|id| self.presets.iter().position(|preset| preset.id == id))
            .map(|index| gpui_component::IndexPath::default().row(index));
        let select_state =
            cx.new(|cx| SelectState::new(items, initial_index, window, cx).searchable(true));

        cx.subscribe_in(&select_state, window, Self::on_preset_select)
            .detach();

        self.preset_select_state = Some(select_state);
    }

    /// 懒初始化模型下拉列表（需要 &mut Window）
    fn ensure_model_select_state(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.model_select_state.is_some() {
            return;
        }
        let models = models_for_preset(&self.models, self.selected_preset_id);
        let items: SearchableVec<ModelSelectItem> = SearchableVec::new(
            models
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
            .and_then(|sid| models.iter().position(|m| m.id == sid))
            .map(|ix| gpui_component::IndexPath::default().row(ix));
        let select_state =
            cx.new(|cx| SelectState::new(items, initial_index, window, cx).searchable(true));

        // 订阅 SelectEvent，当用户选择模型时更新 selected_model_id
        cx.subscribe_in(&select_state, window, Self::on_model_select)
            .detach();

        self.model_select_state = Some(select_state);
    }

    /// Preset 选择事件处理。切换 Preset 时清除旧模型，避免跨 Preset 使用模型。
    fn on_preset_select(
        &mut self,
        _: &Entity<SelectState<SearchableVec<PresetSelectItem>>>,
        event: &gpui_component::select::SelectEvent<SearchableVec<PresetSelectItem>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let gpui_component::select::SelectEvent::Confirm(preset_id) = event;
        let preset_id =
            (*preset_id).filter(|id| self.presets.iter().any(|preset| preset.id == *id));
        if self.selected_preset_id != preset_id {
            self.selected_preset_id = preset_id;
            self.selected_model_id = None;
            self.model_select_state = None;
        }
        self.form_error = None;
        cx.notify();
    }

    /// 模型选择事件处理
    fn on_model_select(
        &mut self,
        _: &Entity<SelectState<SearchableVec<ModelSelectItem>>>,
        event: &gpui_component::select::SelectEvent<SearchableVec<ModelSelectItem>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let gpui_component::select::SelectEvent::Confirm(model_id) = event;
        self.selected_model_id =
            valid_model_selection(&self.models, self.selected_preset_id, *model_id);
        self.form_error = None;
        cx.notify();
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
        if let Some(pos) = self.messages.iter().position(|m| m.id == id)
            && pos > 0
        {
            self.messages.swap(pos, pos - 1);
        }
    }

    fn move_message_down(&mut self, id: u64) {
        if let Some(pos) = self.messages.iter().position(|m| m.id == id)
            && pos + 1 < self.messages.len()
        {
            self.messages.swap(pos, pos + 1);
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
        if *editing && let Some(collapsed) = self.collapsed_tools.get_mut(&id) {
            *collapsed = false;
        }
    }

    fn get_tool_inputs(
        &mut self,
        id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (
        Entity<InputState>,
        Entity<TextareaState>,
        Entity<TextareaState>,
    ) {
        self.tool_inputs
            .entry(id)
            .or_insert_with(|| {
                let tool = self.tools.iter().find(|t| t.id == id);
                let name = tool.map(|t| t.name.clone()).unwrap_or_default();
                let desc = tool.map(|t| t.description.clone()).unwrap_or_default();
                let params = tool.map(|t| t.parameters_json.clone()).unwrap_or_default();
                let name_input = cx.new(|cx| InputState::new(window, cx).default_value(&name));
                let desc_input = cx.new(|cx| TextareaState::new(window, cx).default_value(&desc));
                let params_input = cx.new(|cx| {
                    TextareaState::new(window, cx)
                        .auto_grow(3, 8)
                        .default_value(&params)
                });
                (name_input, desc_input, params_input)
            })
            .clone()
    }

    fn save_tool_edit(&mut self, id: u64, cx: &mut Context<Self>) {
        if let Some((name_input, desc_input, params_input)) = self.tool_inputs.get(&id) {
            let new_name = name_input.read(cx).value().to_string();
            let new_desc = desc_input.read(cx).value().to_string();
            let new_params = params_input.read(cx).value().to_string();
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
        if *editing && let Some(collapsed) = self.collapsed_messages.get_mut(&id) {
            *collapsed = false;
        }
    }

    fn get_message_input(
        &mut self,
        id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<TextareaState> {
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
                    TextareaState::new(window, cx)
                        .auto_grow(1, 5)
                        .default_value(&content)
                })
            })
            .clone()
    }

    fn save_message_content(&mut self, id: u64, cx: &mut Context<Self>) {
        if let Some(input) = self.message_inputs.get(&id) {
            let new_content = input.read(cx).value().to_string();
            self.update_message_content(id, new_content);
        }
        if let Some(editing) = self.editing_messages.get_mut(&id) {
            *editing = false;
        }
    }

    /// 获取历史文件路径
    ///
    /// 单元测试下改写到临时目录，避免测试覆盖真实的历史记录文件。
    fn history_file_path() -> PathBuf {
        if cfg!(test) {
            return std::env::temp_dir().join("hivegui-prompt-debug-history-test.json");
        }
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        PathBuf::from(home)
            .join(".hiveclaw")
            .join("prompt_debug_history.json")
    }

    /// 加载历史记录
    fn load_history(&mut self) {
        let path = Self::history_file_path();
        if let Ok(content) = std::fs::read_to_string(&path)
            && let Ok(records) = serde_json::from_str::<Vec<ExecutionRecord>>(&content)
        {
            self.execution_history = records;
            self.next_record_id = self
                .execution_history
                .iter()
                .map(|r| r.id + 1)
                .max()
                .unwrap_or(1);
            self.history_page = 0;
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
        // 新记录展示在最前，回到第一页确保可见。
        self.history_page = 0;
        self.save_history();
    }

    /// 历史记录总页数（至少 1 页）。
    fn history_total_pages(&self) -> usize {
        self.execution_history
            .len()
            .div_ceil(HISTORY_PAGE_SIZE)
            .max(1)
    }

    /// 当前页码（已按总页数收敛，避免删除记录后停在空页）。
    fn current_history_page(&self) -> usize {
        self.history_page.min(self.history_total_pages() - 1)
    }

    fn prev_history_page(&mut self) {
        let current = self.current_history_page();
        self.history_page = current.saturating_sub(1);
    }

    fn next_history_page(&mut self) {
        let current = self.current_history_page();
        self.history_page = (current + 1).min(self.history_total_pages() - 1);
    }

    /// 切换记录选中状态
    fn toggle_record_selection(&mut self, id: u64) {
        if self.selected_record_ids.contains(&id) {
            self.selected_record_ids.remove(&id);
        } else {
            self.selected_record_ids.insert(id);
        }
    }

    /// 全选全部历史记录
    fn select_all_records(&mut self) {
        self.selected_record_ids = self.execution_history.iter().map(|r| r.id).collect();
    }

    /// 取消全选
    fn deselect_all_records(&mut self) {
        self.selected_record_ids.clear();
    }

    /// 反选：已选中的取消，未选中的选中
    fn invert_record_selection(&mut self) {
        for record in &self.execution_history {
            if self.selected_record_ids.contains(&record.id) {
                self.selected_record_ids.remove(&record.id);
            } else {
                self.selected_record_ids.insert(record.id);
            }
        }
    }

    /// 删除单条历史记录
    fn delete_record(&mut self, record_id: u64) {
        self.execution_history.retain(|r| r.id != record_id);
        self.selected_record_ids.remove(&record_id);
        self.comparing_records.retain(|r| r.id != record_id);
        self.view_records.retain(|r| r.id != record_id);
        if self.view_records.is_empty() {
            self.show_view_modal = false;
        }
        self.context_menu = None;
        self.history_page = self.current_history_page();
        self.save_history();
    }

    /// 删除选中的历史记录（未选中任何记录时不做任何事）
    fn delete_selected_records(&mut self) {
        if self.selected_record_ids.is_empty() {
            return;
        }
        self.execution_history
            .retain(|r| !self.selected_record_ids.contains(&r.id));
        self.comparing_records
            .retain(|r| !self.selected_record_ids.contains(&r.id));
        self.view_records
            .retain(|r| !self.selected_record_ids.contains(&r.id));
        if self.view_records.is_empty() {
            self.show_view_modal = false;
        }
        self.selected_record_ids.clear();
        self.context_menu = None;
        self.history_page = self.current_history_page();
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
        self.view_modal_tab = ViewModalTab::Formatted;
        // 正文用的只读 TextareaState 随弹窗一起释放，避免长期驻留。
        self.view_text_states.clear();
    }

    /// 切换仅看差异
    fn toggle_show_only_diff(&mut self) {
        self.show_only_diff = !self.show_only_diff;
    }

    fn show_context_menu(&mut self, record_id: u64, position: Point<Pixels>) {
        self.context_menu = Some((record_id, position));
    }

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
                .default_value(record.temperature.to_string())
        }));
        self.max_tokens_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("2048")
                .default_value(record.max_tokens.to_string())
        }));
        self.top_p_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("1.00")
                .default_value(record.top_p.to_string())
        }));
        self.thinking_budget_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("1024")
                .default_value(record.thinking_budget.to_string())
        }));
        // 回填工具定义
        self.tools = record.tools.clone();
        self.context_menu = None;
    }
}

// ──────────────────────────────────────────────
// UI 渲染组件
// ──────────────────────────────────────────────

/// Preset 选择器（Select 下拉列表）
fn preset_selector(
    select_state: &Entity<SelectState<SearchableVec<PresetSelectItem>>>,
    style: &ManagementStyle,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(4.0))
        .debug_selector(|| "PROMPT_DEBUGGER_PRESET_SELECTOR".to_owned())
        .child(
            div()
                .text_size(px(12.0))
                .text_color(style.list.muted_foreground)
                .child("Preset"),
        )
        .child(
            div()
                .w_full()
                .debug_selector(|| "PROMPT_DEBUGGER_PRESET_SELECT_TRIGGER".to_owned())
                .child(Select::new(select_state).placeholder("请选择 Preset")),
        )
}

/// 模型选择器（Select 下拉列表）
fn model_selector(
    select_state: &Entity<SelectState<SearchableVec<ModelSelectItem>>>,
    preset_selected: bool,
    style: &ManagementStyle,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(4.0))
        .debug_selector(|| "PROMPT_DEBUGGER_MODEL_SELECTOR".to_owned())
        .child(
            div()
                .text_size(px(12.0))
                .text_color(style.list.muted_foreground)
                .child("模型"),
        )
        .child(
            div()
                .w_full()
                .debug_selector(|| "PROMPT_DEBUGGER_MODEL_SELECT_TRIGGER".to_owned())
                .child(
                    Select::new(select_state)
                        .placeholder(if preset_selected {
                            "请选择模型"
                        } else {
                            "请先选择 Preset"
                        })
                        .disabled(!preset_selected),
                ),
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
    _tooltip_text: &'static str,
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
    preset_select_state: &Entity<SelectState<SearchableVec<PresetSelectItem>>>,
    model_select_state: &Entity<SelectState<SearchableVec<ModelSelectItem>>>,
    preset_selected: bool,
    temp_input: &Entity<InputState>,
    _max_tokens_input: &Entity<InputState>,
    top_p_input: &Entity<InputState>,
    presence_penalty_input: &Entity<InputState>,
    frequency_penalty_input: &Entity<InputState>,
    thinking_budget_input: &Entity<InputState>,
    thinking_enabled: bool,
    entity: Entity<PromptDebugger>,
    style: &ManagementStyle,
    form_error: Option<&str>,
) -> impl IntoElement {
    let title = div()
        .text_size(px(14.0))
        .font_weight(FontWeight::BOLD)
        .mb(px(12.0))
        .child("LLM 设定");

    let mut panel = div()
        .w(px(220.0))
        .flex_shrink_0()
        .h_full()
        .p(px(12.0))
        .border_r_1()
        .border_color(style.list.border)
        .overflow_y_scrollbar()
        .child(title)
        .when_some(form_error, |panel, msg| {
            panel.child(
                div()
                    .id("prompt-debugger-form-error")
                    .debug_selector(|| "PROMPT_DEBUGGER_FORM_ERROR".to_owned())
                    .mt(px(8.0))
                    .mb(px(4.0))
                    .p(px(8.0))
                    .border_1()
                    .border_color(style.action(ActionRole::Delete).foreground)
                    .rounded(px(6.0))
                    .text_color(style.action(ActionRole::Delete).foreground)
                    .text_size(px(12.0))
                    .child(msg.to_string()),
            )
        })
        .child(preset_selector(preset_select_state, style))
        .child(div().mt(px(12.0)))
        .child(model_selector(model_select_state, preset_selected, style))
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
                    entity.update(cx, |t, cx| {
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
    input: Option<Entity<TextareaState>>,
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
                                        entity.update(cx, |view, cx| {
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
                                        entity.update(cx, |view, cx| {
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
                                        entity.update(cx, |view, cx| {
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
                                        entity.update(cx, |view, cx| {
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
                                        entity.update(cx, |view, cx| {
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
                    .child(div().w_full().child(Textarea::new(&input_state)))
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
                                        entity.update(cx, |view, cx| {
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
        let mut input_map: HashMap<u64, Entity<TextareaState>> = HashMap::new();
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
                                entity.update(cx, |view, cx| {
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
                                entity.update(cx, |view, cx| {
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
                                entity.update(cx, |view, cx| {
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
                                        entity.update(cx, |view, cx| {
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
                                    move |_, _window, cx| {
                                        entity.update(cx, |view, cx| {
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
                                        entity.update(cx, |view, cx| {
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
                    .child(Textarea::new(&desc_input))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(style.list.muted_foreground)
                            .child("参数 (JSON Schema):"),
                    )
                    .child(Textarea::new(&params_input))
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
                                entity.update(cx, |view, cx| {
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
                                entity.update(cx, |view, cx| {
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
            .debug_selector(|| "PROMPT_DEBUGGER_EXECUTE_BTN".to_owned())
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
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                if !is_loading {
                    entity.update(cx, |view, cx| {
                        view.execute_call(window, cx);
                    });
                }
            }),
    )
}

/// 历史面板里的小型操作按钮。
///
/// `enabled = false` 时按钮变灰且不触发回调（例如未选中任何记录时的「清除」）。
fn history_action_button(
    label: SharedString,
    role: ActionRole,
    enabled: bool,
    selector: &'static str,
    style: &ManagementStyle,
    entity: &Entity<PromptDebugger>,
    action: impl Fn(&mut PromptDebugger, &mut Context<PromptDebugger>) + 'static,
) -> Div {
    div()
        .px(px(8.0))
        .py(px(4.0))
        .bg(style.action(role).background)
        .rounded(px(4.0))
        .cursor(if enabled {
            CursorStyle::PointingHand
        } else {
            CursorStyle::Arrow
        })
        .text_size(px(11.0))
        .text_color(style.action(role).foreground)
        .opacity(if enabled { 1.0 } else { 0.45 })
        .debug_selector(move || selector.to_owned())
        .child(label)
        .on_mouse_down(MouseButton::Left, {
            let entity = entity.clone();
            move |_, _, cx| {
                if !enabled {
                    return;
                }
                entity.update(cx, |view, cx| {
                    action(view, cx);
                    cx.notify();
                });
            }
        })
}

/// 右侧面板（历史 + 当前结果合并）
fn right_panel(
    _call_state: &CallState,
    selected_count: usize,
    history_count: usize,
    history_page: usize,
    history: &[ExecutionRecord],
    selected_ids: &HashSet<u64>,
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

    // 当前结果预览已移除：执行结果统一在「查看执行记录」弹窗中展示。
    // 历史列表见下方。

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

    // 操作栏：全选 / 反选 / 清除选中
    let all_selected = history_count > 0 && selected_count == history_count;
    let mut toolbar = div()
        .flex()
        .items_center()
        .flex_wrap()
        .gap(px(6.0))
        .child(history_action_button(
            SharedString::from(if all_selected {
                "取消全选"
            } else {
                "全选"
            }),
            ActionRole::Neutral,
            history_count > 0,
            "PROMPT_HISTORY_SELECT_ALL",
            style,
            &entity,
            |view, _cx| {
                if view.selected_record_ids.len() == view.execution_history.len() {
                    view.deselect_all_records();
                } else {
                    view.select_all_records();
                }
            },
        ))
        .child(history_action_button(
            SharedString::from("反选"),
            ActionRole::Neutral,
            history_count > 0,
            "PROMPT_HISTORY_INVERT_SELECTION",
            style,
            &entity,
            |view, _cx| view.invert_record_selection(),
        ))
        .child(history_action_button(
            SharedString::from(if selected_count > 0 {
                format!("清除({selected_count})")
            } else {
                "清除".to_string()
            }),
            ActionRole::Delete,
            selected_count > 0,
            "PROMPT_HISTORY_CLEAR_SELECTED",
            style,
            &entity,
            |view, _cx| view.delete_selected_records(),
        ));

    if selected_count >= 2 {
        toolbar = toolbar.child(history_action_button(
            SharedString::from(format!("对比({selected_count})")),
            ActionRole::Main,
            true,
            "PROMPT_HISTORY_COMPARE",
            style,
            &entity,
            |view, _cx| view.start_comparison(),
        ));
    }

    panel = panel.child(toolbar);

    // 分页：每页 HISTORY_PAGE_SIZE 条，页码从 0 开始。
    let total_pages = history.len().div_ceil(HISTORY_PAGE_SIZE).max(1);
    let page = history_page.min(total_pages - 1);
    // 记录按「由旧到新」存放，展示为「由新到旧」分页：第 0 页是最新的 HISTORY_PAGE_SIZE 条。
    let page_end = history.len().saturating_sub(page * HISTORY_PAGE_SIZE);
    let page_start = page_end.saturating_sub(HISTORY_PAGE_SIZE);
    let page_records = &history[page_start..page_end];

    // 历史列表（当前页，最新的排在最前）
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
        for record in page_records.iter().rev() {
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
                    .debug_selector({
                        let record_id = record.id;
                        move || format!("PROMPT_HISTORY_RECORD_{record_id}")
                    })
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
                            entity.update(cx, |view, cx| {
                                view.toggle_record_selection(record_id);
                                cx.notify();
                            });
                        }
                    })
                    .on_mouse_down(MouseButton::Right, {
                        let entity = entity.clone();
                        let record_id = record.id;
                        move |event: &MouseDownEvent, _, cx| {
                            entity.update(cx, |view, cx| {
                                view.show_context_menu(record_id, event.position);
                                cx.notify();
                            });
                        }
                    }),
            );
        }
    }

    // 分页条：仅在多页时显示
    if total_pages > 1 {
        let can_prev = page > 0;
        let can_next = page + 1 < total_pages;
        panel = panel.child(
            div()
                .mt_auto()
                .pt(px(8.0))
                .border_t_1()
                .border_color(style.list.border)
                .flex()
                .items_center()
                .justify_between()
                .gap(px(6.0))
                .child(history_action_button(
                    SharedString::from("上一页"),
                    ActionRole::Neutral,
                    can_prev,
                    "PROMPT_HISTORY_PREV_PAGE",
                    style,
                    &entity,
                    |view, _cx| view.prev_history_page(),
                ))
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(style.list.muted_foreground)
                        .debug_selector(|| "PROMPT_HISTORY_PAGE_INFO".to_owned())
                        .child(SharedString::from(format!(
                            "{}/{} 页",
                            page + 1,
                            total_pages
                        ))),
                )
                .child(history_action_button(
                    SharedString::from("下一页"),
                    ActionRole::Neutral,
                    can_next,
                    "PROMPT_HISTORY_NEXT_PAGE",
                    style,
                    &entity,
                    |view, _cx| view.next_history_page(),
                )),
        );
    }

    panel
}

/// 历史记录右键菜单。
///
/// `MouseDownEvent::position` 是窗口坐标，因此菜单必须挂在窗口级浮层中；如果把它作为
/// 右侧滚动面板的绝对子元素，坐标会被右栏原点再次偏移并被滚动区域裁剪。
fn history_context_menu(
    record_id: u64,
    position: Point<Pixels>,
    style: &ManagementStyle,
    entity: Entity<PromptDebugger>,
    window: &Window,
) -> impl IntoElement {
    let dismiss_on_left = entity.downgrade();
    let dismiss_on_right = entity.downgrade();
    let entity_for_view = entity.downgrade();
    let entity_for_apply = entity.downgrade();
    let entity_for_delete = entity.downgrade();
    let view_selector = format!("PROMPT_HISTORY_VIEW_{record_id}");
    let apply_selector = format!("PROMPT_HISTORY_APPLY_{record_id}");
    let delete_selector = format!("PROMPT_HISTORY_DELETE_{record_id}");

    deferred(
        anchored().child(
            div()
                .w(window.bounds().size.width)
                .h(window.bounds().size.height)
                .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    cx.stop_propagation();
                    _ = dismiss_on_left.update(cx, |view, cx| {
                        view.hide_context_menu();
                        cx.notify();
                    });
                })
                .on_mouse_down(MouseButton::Right, move |_, _, cx| {
                    cx.stop_propagation();
                    _ = dismiss_on_right.update(cx, |view, cx| {
                        view.hide_context_menu();
                        cx.notify();
                    });
                })
                .child(
                    anchored()
                        .position(position)
                        .snap_to_window_with_margin(px(8.0))
                        .child(
                            div()
                                .w(px(120.0))
                                .bg(style.list.row)
                                .border_1()
                                .border_color(style.list.border)
                                .rounded(px(6.0))
                                .shadow_md()
                                .p(px(4.0))
                                .debug_selector(|| "PROMPT_HISTORY_CONTEXT_MENU".to_owned())
                                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                    cx.stop_propagation();
                                })
                                .on_mouse_down(MouseButton::Right, |_, _, cx| {
                                    cx.stop_propagation();
                                })
                                .child(
                                    div()
                                        .px(px(8.0))
                                        .py(px(6.0))
                                        .rounded(px(4.0))
                                        .cursor(CursorStyle::PointingHand)
                                        .hover(|this| {
                                            this.bg(style.action(ActionRole::Neutral).hover)
                                        })
                                        .text_size(px(12.0))
                                        .debug_selector(move || view_selector.clone())
                                        .child("查看")
                                        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                            _ = entity_for_view.update(cx, |view, cx| {
                                                view.open_view_record(record_id);
                                                cx.notify();
                                            });
                                        }),
                                )
                                .child(
                                    div()
                                        .mt(px(4.0))
                                        .px(px(8.0))
                                        .py(px(6.0))
                                        .rounded(px(4.0))
                                        .cursor(CursorStyle::PointingHand)
                                        .hover(|this| {
                                            this.bg(style.action(ActionRole::Neutral).hover)
                                        })
                                        .text_size(px(12.0))
                                        .debug_selector(move || apply_selector.clone())
                                        .child("回填")
                                        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                                            _ = entity_for_apply.update(cx, |view, cx| {
                                                if let Some(record) = view
                                                    .execution_history
                                                    .iter()
                                                    .find(|record| record.id == record_id)
                                                    .cloned()
                                                {
                                                    view.apply_record(&record, window, cx);
                                                } else {
                                                    view.hide_context_menu();
                                                }
                                                cx.notify();
                                            });
                                        }),
                                )
                                .child(
                                    div()
                                        .mt(px(4.0))
                                        .px(px(8.0))
                                        .py(px(6.0))
                                        .rounded(px(4.0))
                                        .cursor(CursorStyle::PointingHand)
                                        .hover(|this| {
                                            this.bg(style.action(ActionRole::Delete).hover)
                                        })
                                        .text_size(px(12.0))
                                        .text_color(style.action(ActionRole::Delete).foreground)
                                        .debug_selector(move || delete_selector.clone())
                                        .child("删除")
                                        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                            _ = entity_for_delete.update(cx, |view, cx| {
                                                view.delete_record(record_id);
                                                cx.notify();
                                            });
                                        }),
                                ),
                        ),
                ),
        ),
    )
    .with_priority(1)
}

/// 单条记录卡片（查看模式）
fn single_record_card(
    record: &ExecutionRecord,
    style: &ManagementStyle,
    states: &mut HashMap<String, Entity<TextareaState>>,
    window: &mut Window,
    cx: &mut Context<PromptDebugger>,
) -> impl IntoElement {
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
    let params_summary = format!(
        "T={:.2} Max={} TopP={:.2}",
        record.temperature, record.max_tokens, record.top_p
    );

    div()
        .debug_selector(|| "PROMPT_HISTORY_VIEW_CARD".to_owned())
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
                            view_selectable_text(
                                states,
                                format!("card:{}:model", record.id),
                                &record.model_name,
                                style,
                                window,
                                cx,
                            )
                            .text_size(px(14.0))
                            .font_weight(FontWeight::SEMIBOLD),
                        )
                        .child(
                            view_selectable_text(
                                states,
                                format!("card:{}:params", record.id),
                                &params_summary,
                                style,
                                window,
                                cx,
                            )
                            .text_color(style.list.muted_foreground),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.0))
                        .child(
                            view_selectable_text(
                                states,
                                format!("card:{}:label-messages", record.id),
                                "消息:",
                                style,
                                window,
                                cx,
                            )
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(style.list.muted_foreground),
                        )
                        .child(view_selectable_text(
                            states,
                            format!("card:{}:messages", record.id),
                            &msg_summary,
                            style,
                            window,
                            cx,
                        )),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.0))
                        .child(
                            view_selectable_text(
                                states,
                                format!("card:{}:label-tools", record.id),
                                "工具:",
                                style,
                                window,
                                cx,
                            )
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(style.list.muted_foreground),
                        )
                        .child(view_selectable_text(
                            states,
                            format!("card:{}:tools", record.id),
                            &tools_summary,
                            style,
                            window,
                            cx,
                        )),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.0))
                        .child(
                            view_selectable_text(
                                states,
                                format!("card:{}:label-result", record.id),
                                "结果:",
                                style,
                                window,
                                cx,
                            )
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(style.list.muted_foreground),
                        )
                        .child(view_selectable_text(
                            states,
                            format!("card:{}:result", record.id),
                            &status_text,
                            style,
                            window,
                            cx,
                        )),
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

/// 弹窗正文只读文本的最大行数（`auto_grow` 上限）。
///
/// 输入框内部没有滚轮处理器，内容一旦超过 `max_rows` 就会被裁掉且无法滚动，
/// 所以这里取一个足够大的值让文本一直长高，纵向滚动交给弹窗自己的滚动容器。
const VIEW_TEXT_MAX_ROWS: usize = 2000;

/// 弹窗正文里一段「可选中 / 可复制」的只读文本。
///
/// GPUI 0.2 的 `div` 文本不支持鼠标拖选（只有输入类组件可以），所以弹窗正文
/// 一律改用只读 `Textarea` 承载：关掉 appearance / border 后外观与纯文本一致，
/// 但可以鼠标拖选、Ctrl/Cmd+C 复制，也能用右键菜单的「复制 / 全选」。
///
/// `key` 用于在 `states` 中复用同一个 `TextareaState`——每次重绘都新建实体的
/// 话，选中状态会被重置；内容变化时同步一次即可。
///
/// 返回具体类型 `Textarea` 而不是 `impl IntoElement`：后者在 2024 edition 会
/// 捕获 `states` 的可变借用，导致同一个 `states` 无法连续构造多个文本块。
fn view_selectable_text(
    states: &mut HashMap<String, Entity<TextareaState>>,
    key: String,
    text: &str,
    style: &ManagementStyle,
    window: &mut Window,
    cx: &mut Context<PromptDebugger>,
) -> Textarea {
    let state = match states.get(&key) {
        Some(state) => {
            if state.read(cx).value().as_str() != text {
                state.update(cx, |state, cx| {
                    state.set_value(text.to_string(), window, cx);
                });
            }
            state.clone()
        }
        None => {
            let state = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .default_value(text.to_string())
                    .auto_grow(1, VIEW_TEXT_MAX_ROWS)
            });
            states.insert(key, state.clone());
            state
        }
    };

    Textarea::new(&state)
        .appearance(false)
        .bordered(false)
        .readonly(true)
        .text_size(px(12.0))
        .text_color(style.list.foreground)
        .w_full()
}

/// 对比表格（2+ 条记录）
/// 第 1 列：对比维度，后续列：每条记录的数据，最多 20 列
fn comparison_table(
    records: &[ExecutionRecord],
    style: &ManagementStyle,
    show_only_diff: bool,
    states: &mut HashMap<String, Entity<TextareaState>>,
    window: &mut Window,
    cx: &mut Context<PromptDebugger>,
) -> impl IntoElement {
    let col_count = records.len().min(20) + 1; // +1 为维度列
    let col_width = px(300.0);
    let label_col_width = px(100.0);
    let table_width = label_col_width + col_width * (col_count - 1) as f32;

    // 维度标签。「模型」不放这里：表头行已经按列标注了各条记录的模型名，
    // 再列一行会导致表头与「模型」行内容完全重复。
    let dimensions: Vec<&str> = vec!["参数", "消息", "工具", "结果"];

    // 序号行（表头之上）：标注每条记录是第几条
    let index_row = div()
        .flex()
        .border_b_1()
        .border_color(style.list.border)
        .child(
            div()
                .w(label_col_width)
                .flex_shrink_0()
                .px(px(8.0))
                .py(px(6.0))
                .child(
                    view_selectable_text(
                        states,
                        "cmp:index:label".to_string(),
                        "序号",
                        style,
                        window,
                        cx,
                    )
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(style.list.muted_foreground),
                ),
        );
    let index_row = records
        .iter()
        .take(20)
        .enumerate()
        .fold(index_row, |acc, (idx, record)| {
            acc.child(
                div()
                    .w(col_width)
                    .flex_shrink_0()
                    .px(px(8.0))
                    .py(px(6.0))
                    .child(
                        view_selectable_text(
                            states,
                            format!("cmp:index:{}", record.id),
                            &format!("#{}", idx + 1),
                            style,
                            window,
                            cx,
                        )
                        .font_weight(FontWeight::BOLD),
                    ),
            )
        });

    // 表头行：每列标注该条记录的模型名
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
                .child(
                    view_selectable_text(
                        states,
                        "cmp:header:label".to_string(),
                        "维度",
                        style,
                        window,
                        cx,
                    )
                    .font_weight(FontWeight::BOLD),
                ),
        );
    let header_row = records.iter().take(20).fold(header_row, |acc, record| {
        acc.child(
            div()
                .w(col_width)
                .flex_shrink_0()
                .px(px(8.0))
                .py(px(8.0))
                .child(
                    view_selectable_text(
                        states,
                        format!("cmp:header:{}", record.id),
                        &record.model_name,
                        style,
                        window,
                        cx,
                    )
                    .font_weight(FontWeight::BOLD),
                ),
        )
    });

    // 数据行
    let mut table = div()
        .w(table_width)
        .flex()
        .flex_col()
        .child(index_row)
        .child(header_row);

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
                    .child(
                        view_selectable_text(
                            states,
                            format!("cmp:dim:{dim}"),
                            dim,
                            style,
                            window,
                            cx,
                        )
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(style.list.muted_foreground),
                    ),
            );

        let row = records.iter().take(20).fold(row, |acc, record| {
            // 单元格正文用只读输入框渲染，保证可以拖选 / 复制。
            let cell_text = get_dimension_value(record, dim);
            let key = format!("cmp:{}:{}", record.id, dim);
            acc.child(
                div()
                    .w(col_width)
                    .flex_shrink_0()
                    .px(px(8.0))
                    .py(px(8.0))
                    .child(view_selectable_text(
                        states, key, &cell_text, style, window, cx,
                    )),
            )
        });

        table = table.child(row);
    }

    table
}

/// 拼接记录为可拷贝到剪贴板的纯文本。
///
/// 正文已经用只读输入框渲染、可以逐段拖选复制，这个「复制」按钮提供的是
/// 一键带走整组记录（含维度标题与分节分隔）的快捷方式。
fn format_records_for_copy(records: &[ExecutionRecord]) -> String {
    if records.len() > 1 {
        let mut out = String::new();
        out.push_str(&format!("对比 ({} 条记录)\n", records.len()));
        out.push_str(&"─".repeat(40));
        out.push('\n');
        for (idx, record) in records.iter().enumerate() {
            out.push_str(&format!("\n[{}] {}\n", idx + 1, record.model_name));
            out.push_str(&format!(
                "参数: T={:.2} Max={} TopP={:.2}\n",
                record.temperature, record.max_tokens, record.top_p
            ));
            let messages = record
                .messages
                .iter()
                .map(|m| format!("[{}] {}", m.role.label(), m.content))
                .collect::<Vec<_>>()
                .join("\n");
            out.push_str(&format!("消息:\n{messages}\n"));
            let tools = if record.tools.is_empty() {
                "无工具".to_string()
            } else {
                record
                    .tools
                    .iter()
                    .map(|t| format!("- {}: {}", t.name, t.description))
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            out.push_str(&format!("工具:\n{tools}\n"));
            let result = match &record.result {
                CallState::Success(t) => t.clone(),
                CallState::Error(e) => e.clone(),
                _ => "无结果".to_string(),
            };
            out.push_str(&format!("结果:\n{result}\n"));
            out.push_str(&"─".repeat(40));
            out.push('\n');
        }
        out
    } else if let Some(record) = records.first() {
        let mut out = String::new();
        out.push_str(&format!("模型: {}\n", record.model_name));
        out.push_str(&format!(
            "参数: T={:.2} Max={} TopP={:.2}\n",
            record.temperature, record.max_tokens, record.top_p
        ));
        let messages = record
            .messages
            .iter()
            .map(|m| format!("[{}] {}", m.role.label(), m.content))
            .collect::<Vec<_>>()
            .join("\n");
        out.push_str(&format!("消息:\n{messages}\n"));
        let tools = if record.tools.is_empty() {
            "无工具".to_string()
        } else {
            record
                .tools
                .iter()
                .map(|t| format!("- {}: {}", t.name, t.description))
                .collect::<Vec<_>>()
                .join("\n")
        };
        out.push_str(&format!("工具:\n{tools}\n"));
        let result = match &record.result {
            CallState::Success(t) => t.clone(),
            CallState::Error(e) => e.clone(),
            _ => "无结果".to_string(),
        };
        out.push_str(&format!("结果:\n{result}\n"));
        out
    } else {
        String::new()
    }
}

/// 查看/对比模态框
#[allow(clippy::too_many_arguments)]
fn view_modal(
    records: &[ExecutionRecord],
    style: &ManagementStyle,
    tab: ViewModalTab,
    show_only_diff: bool,
    view_scroll: &ScrollHandle,
    entity: Entity<PromptDebugger>,
    max_panel_width: Pixels,
    text_states: &mut HashMap<String, Entity<TextareaState>>,
    window: &mut Window,
    cx: &mut Context<PromptDebugger>,
) -> impl IntoElement {
    let is_comparison = records.len() > 1;
    let title = if is_comparison {
        format!("对比 ({} 条记录)", records.len())
    } else {
        "查看执行记录".to_string()
    };

    // 预先拼接用于"复制"按钮的文本（避免在 click handler 内再次构造）。
    // 复制按钮跟随当前 Tab：格式化 → 文本摘要；JSON → 输入/输出/Metadata 分节。
    let copy_text = match tab {
        ViewModalTab::Formatted => format_records_for_copy(records),
        ViewModalTab::Json => format_records_for_json_copy(records),
    };
    // 对比时表格宽度 = 维度列 100 + 每条记录 300（另加左右内边距 32）。
    // 弹窗宽度按此计算，但不超过窗口可视宽度的 92%，超宽部分由内容区的
    // 横向滚动条承载，避免选择多条记录时弹窗被挤出屏幕。
    let modal_width = if is_comparison {
        let cols = records.len().min(20);
        px(132.0 + cols as f32 * 300.0).min(max_panel_width)
    } else {
        px(800.0).min(max_panel_width)
    };

    div()
        .absolute()
        .top_0()
        .bottom_0()
        .left_0()
        .right_0()
        .debug_selector(|| "PROMPT_HISTORY_VIEW_MODAL".to_owned())
        // 必须遮挡后面的主界面：gpui 的滚轮事件会沿冒泡链继续传给祖先的滚动
        // 容器，弹窗内容滚到尽头后（甚至滚动弹窗空白处）会把后面的提示词主
        // 界面一起滚走。`occlude()` 让遮罩下层所有 hitbox 的
        // `should_handle_scroll()` 返回 false，从而只滚动弹窗自己。
        // 面板本身是遮罩的子元素（在其之后绘制），不受影响。
        .occlude()
        .bg(gpui::rgba(0x00000080))
        .flex()
        .items_center()
        .justify_center()
        .on_mouse_down(MouseButton::Left, {
            let entity = entity.clone();
            move |_, _, cx| {
                entity.update(cx, |view, cx| {
                    view.close_view_modal();
                    cx.notify();
                });
            }
        })
        .child(
            div()
                .w(modal_width)
                .max_h(px(600.0))
                .debug_selector(|| "PROMPT_HISTORY_VIEW_PANEL".to_owned())
                .bg(style.list.row)
                .rounded(px(12.0))
                .shadow_lg()
                .border_1()
                .border_color(style.list.border)
                .flex()
                .flex_col()
                .overflow_hidden()
                // 阻止面板内的点击冒泡到外层 overlay（其 on_mouse_down 会关闭
                // 弹窗）。必须在冒泡阶段拦截：capture_any_mouse_down 在 capture
                // 阶段（由外向内）stop_propagation，会连带吞掉子元素（复制/
                // 关闭按钮、仅看差异、滚动条）的点击处理器，导致全部无响应。
                // on_any_mouse_down 在冒泡阶段执行：子元素先处理自己的点击，
                // 随后面板拦下，点击面板外遮罩仍可关闭弹窗。
                .on_any_mouse_down(|_, _, cx| {
                    cx.stop_propagation();
                })
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
                                                entity.update(cx, |view, cx| {
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
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .child(view_modal_tabs(tab, entity.clone(), style))
                                .child(
                                    // 复制按钮：把整组记录的文本写入剪贴板，
                                    // 弥补 GPUI 0.2 暂不支持 div 内文本拖选的限制。
                                    div()
                                        .debug_selector(|| "PROMPT_HISTORY_VIEW_COPY".to_owned())
                                        .flex()
                                        .items_center()
                                        .gap(px(4.0))
                                        .cursor(CursorStyle::PointingHand)
                                        .hover(|s| s.opacity(0.7))
                                        .child(Icon::new(IconName::Copy).small())
                                        .child(
                                            div()
                                                .text_size(px(12.0))
                                                .text_color(style.list.muted_foreground)
                                                .child("复制"),
                                        )
                                        .on_mouse_down(MouseButton::Left, {
                                            let text = copy_text.clone();
                                            move |_, _, cx| {
                                                cx.write_to_clipboard(ClipboardItem::new_string(
                                                    text.clone(),
                                                ));
                                                cx.stop_propagation();
                                            }
                                        }),
                                )
                                .child(
                                    div()
                                        .debug_selector(|| "PROMPT_HISTORY_VIEW_CLOSE".to_owned())
                                        .cursor(CursorStyle::PointingHand)
                                        .hover(|s| s.opacity(0.7))
                                        .child(Icon::new(IconName::Close).small())
                                        .on_mouse_down(MouseButton::Left, {
                                            let entity = entity.clone();
                                            move |_, _, cx| {
                                                entity.update(cx, |view, cx| {
                                                    view.close_view_modal();
                                                    cx.notify();
                                                });
                                            }
                                        }),
                                ),
                        ),
                )
                .child(
                    // 内容区域。不能用 `overflow_y_scrollbar()`：其 Scrollable
                    // 包装器把高度全部换成百分比（size_full/min_h_full），在
                    // 这种自动高度（仅 max_h）的面板里内容贡献为 0，会把弹窗
                    // 塌缩成只剩标题栏的空白窗口。原生 `overflow_y_scroll()`
                    // 让内容高度正常传导给面板（同 llm_config 弹窗模式）。
                    div()
                        .id("prompt-view-modal-scroll")
                        .debug_selector(|| "PROMPT_HISTORY_VIEW_SCROLL".to_owned())
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .overflow_x_scroll()
                        .track_scroll(view_scroll)
                        .vertical_scrollbar(view_scroll)
                        .horizontal_scrollbar(view_scroll)
                        .px(px(16.0))
                        .py(px(12.0))
                        .child(match tab {
                            ViewModalTab::Formatted => {
                                if is_comparison {
                                    comparison_table(
                                        records,
                                        style,
                                        show_only_diff,
                                        text_states,
                                        window,
                                        cx,
                                    )
                                    .into_any_element()
                                } else {
                                    single_record_card(&records[0], style, text_states, window, cx)
                                        .into_any_element()
                                }
                            }
                            ViewModalTab::Json => {
                                json_tab_view(records, style, text_states, window, cx)
                                    .into_any_element()
                            }
                        }),
                ),
        )
}

/// 查看弹窗内容 Tab
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ViewModalTab {
    /// 格式化文本卡片（现有视图）
    Formatted,
    /// 输入 / 输出 / Metadata 的 JSON 分节视图
    Json,
}

impl ViewModalTab {
    fn label(self) -> &'static str {
        match self {
            ViewModalTab::Formatted => "格式化",
            ViewModalTab::Json => "JSON",
        }
    }
}

/// 标题栏右侧的「格式化 | JSON」分段切换。
fn view_modal_tabs(
    active: ViewModalTab,
    entity: Entity<PromptDebugger>,
    style: &ManagementStyle,
) -> Div {
    div()
        .flex()
        .items_center()
        .rounded(px(6.0))
        .border_1()
        .border_color(style.list.border)
        .overflow_hidden()
        .children(
            [ViewModalTab::Formatted, ViewModalTab::Json]
                .into_iter()
                .map(move |tab| {
                    let is_active = tab == active;
                    let entity = entity.clone();
                    let id = match tab {
                        ViewModalTab::Formatted => "prompt-view-tab-formatted",
                        ViewModalTab::Json => "prompt-view-tab-json",
                    };
                    div()
                        .id(SharedString::from(id))
                        .debug_selector(move || {
                            format!(
                                "PROMPT_VIEW_TAB_{}",
                                if tab == ViewModalTab::Formatted {
                                    "FORMATTED"
                                } else {
                                    "JSON"
                                }
                            )
                        })
                        .px(px(10.0))
                        .py(px(3.0))
                        .text_size(px(12.0))
                        .cursor(CursorStyle::PointingHand)
                        .bg(if is_active {
                            style.list.hover
                        } else {
                            style.list.row
                        })
                        .text_color(if is_active {
                            style.list.foreground
                        } else {
                            style.list.muted_foreground
                        })
                        .hover(|s| s.text_color(style.list.foreground))
                        .child(tab.label())
                        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                            entity.update(cx, |view, cx| {
                                if view.view_modal_tab != tab {
                                    view.view_modal_tab = tab;
                                    cx.notify();
                                }
                            });
                        })
                }),
        )
}

/// JSON Tab：每条记录一组「输入 / 输出 / Metadata」分节。
fn json_tab_view(
    records: &[ExecutionRecord],
    style: &ManagementStyle,
    states: &mut HashMap<String, Entity<TextareaState>>,
    window: &mut Window,
    cx: &mut Context<PromptDebugger>,
) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(16.0))
        .children(records.iter().enumerate().map(|(idx, record)| {
            json_record_group(idx, records.len(), record, style, states, window, cx)
        }))
}

/// 单条记录的 JSON 分节组（多条记录对比时先标注序号与模型名）。
fn json_record_group(
    idx: usize,
    total: usize,
    record: &ExecutionRecord,
    style: &ManagementStyle,
    states: &mut HashMap<String, Entity<TextareaState>>,
    window: &mut Window,
    cx: &mut Context<PromptDebugger>,
) -> Div {
    let (input, output, metadata) = record_json_sections(record);
    let base = idx + 1;
    let group = div().flex().flex_col().gap(px(12.0));
    let group = if total > 1 {
        let title = format!("#{} {}", base, record.model_name);
        group.child(
            view_selectable_text(
                states,
                format!("json:{}:title", record.id),
                &title,
                style,
                window,
                cx,
            )
            .text_size(px(13.0))
            .font_weight(FontWeight::SEMIBOLD),
        )
    } else {
        group
    };
    group
        .child(json_section(
            "输入",
            &input,
            format!("PROMPT_VIEW_JSON_INPUT_{base}"),
            record.id,
            style,
            states,
            window,
            cx,
        ))
        .child(json_section(
            "输出",
            &output,
            format!("PROMPT_VIEW_JSON_OUTPUT_{base}"),
            record.id,
            style,
            states,
            window,
            cx,
        ))
        .child(json_section(
            "Metadata",
            &metadata,
            format!("PROMPT_VIEW_JSON_METADATA_{base}"),
            record.id,
            style,
            states,
            window,
            cx,
        ))
}

/// 单个 JSON 分节：节标题 + 独立复制按钮 + 等宽文本面板。
#[allow(clippy::too_many_arguments)]
fn json_section(
    label: &str,
    text: &str,
    base: String,
    record_id: u64,
    style: &ManagementStyle,
    states: &mut HashMap<String, Entity<TextareaState>>,
    window: &mut Window,
    cx: &mut Context<PromptDebugger>,
) -> Div {
    let copy_text = text.to_string();
    let copy_id = format!("{base}_COPY");
    let panel_selector = format!("{base}_PANEL");
    div()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    view_selectable_text(
                        states,
                        format!("json:{record_id}:label:{label}"),
                        label,
                        style,
                        window,
                        cx,
                    )
                    .text_size(px(13.0))
                    .font_weight(FontWeight::SEMIBOLD),
                )
                .child(
                    div()
                        .id(SharedString::from(copy_id.as_str()))
                        .debug_selector(move || copy_id.clone())
                        .cursor(CursorStyle::PointingHand)
                        .hover(|s| s.opacity(0.7))
                        .text_color(style.list.muted_foreground)
                        .child(Icon::new(IconName::Copy).small())
                        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(copy_text.clone()));
                            cx.stop_propagation();
                        }),
                ),
        )
        .child(
            div()
                .debug_selector(move || panel_selector.clone())
                .bg(style.list.muted)
                .border_1()
                .border_color(style.list.border)
                .rounded(px(6.0))
                .p(px(12.0))
                // 正文用只读输入框渲染：等宽字体 + 可选中可复制。
                .child(
                    view_selectable_text(
                        states,
                        format!("json:{record_id}:{label}"),
                        text,
                        style,
                        window,
                        cx,
                    )
                    .font_family("monospace"),
                ),
        )
}

/// 把一条执行记录还原为 输入 / 输出 / Metadata 三段 pretty JSON。
/// 输入按 `execute_call` 的请求体字段还原；输出能解析为 JSON 则直接
/// pretty 打印，否则包一层 `{"text": ...}`。
fn record_json_sections(record: &ExecutionRecord) -> (String, String, String) {
    let pretty = |value: &serde_json::Value| {
        serde_json::to_string_pretty(value).unwrap_or_else(|_| String::new())
    };

    let messages: Vec<serde_json::Value> = record
        .messages
        .iter()
        .map(|m| {
            serde_json::json!({
                "role": m.role.api_role(),
                "content": m.content,
            })
        })
        .collect();
    let mut input = serde_json::json!({
        "model": record.model_name,
        "messages": messages,
        "max_tokens": record.max_tokens,
        "temperature": record.temperature,
        "top_p": record.top_p,
    });
    if !record.tools.is_empty() {
        let tools_json: Vec<serde_json::Value> = record
            .tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": serde_json::from_str::<serde_json::Value>(
                            &t.parameters_json,
                        )
                        .unwrap_or(serde_json::Value::Null),
                    },
                })
            })
            .collect();
        input["tools"] = serde_json::json!(tools_json);
    }
    if record.thinking_enabled {
        input["thinking_budget"] = serde_json::json!(record.thinking_budget);
    }

    let output = match &record.result {
        CallState::Success(text) => match serde_json::from_str::<serde_json::Value>(text) {
            Ok(value) => pretty(&value),
            Err(_) => pretty(&serde_json::json!({ "text": text })),
        },
        CallState::Error(err) => pretty(&serde_json::json!({ "error": { "message": err } })),
        _ => pretty(&serde_json::json!({ "text": "" })),
    };

    let metadata = pretty(&serde_json::json!({
        "model": record.model_name,
        "temperature": record.temperature,
        "max_tokens": record.max_tokens,
        "top_p": record.top_p,
        "thinking_enabled": record.thinking_enabled,
        "thinking_budget": record.thinking_budget,
        "timestamp": record.timestamp,
    }));

    (pretty(&input), output, metadata)
}

/// 截断日志中的响应文本，避免把超长内容写入日志。
///
/// 按字符（而非字节）截断，保证多字节字符不会被切成非法 UTF-8。
fn truncate_for_log(text: &str) -> String {
    const LIMIT: usize = 300;
    if text.chars().count() <= LIMIT {
        return text.to_string();
    }
    let mut out: String = text.chars().take(LIMIT).collect();
    out.push('…');
    out
}

/// 把 `Error::source()` 链拼成一行，供日志使用。
///
/// 很多库（如 `reqwest`）的 `Display` 只有最外层描述，
/// "error sending request for url (...)" 并不包含真正的原因；
/// 展开 source 链才能看到 "Connection refused (os error 111)"、
/// "timed out" 之类的根因。
fn error_chain_for_log(error: &(dyn std::error::Error + 'static)) -> String {
    let mut parts = vec![error.to_string()];
    let mut current = error.source();
    while let Some(cause) = current {
        let text = cause.to_string();
        // 相邻重复会放大日志噪音，去掉。
        if parts.last().map(String::as_str) != Some(text.as_str()) {
            parts.push(text);
        }
        current = cause.source();
    }
    parts.join(" <- ")
}

/// 取错误链最深层的原因；没有 `source()` 时退化为顶层消息。
fn root_cause_for_log(error: &(dyn std::error::Error + 'static)) -> String {
    let mut root = error.to_string();
    let mut current = error.source();
    while let Some(cause) = current {
        root = cause.to_string();
        current = cause.source();
    }
    root
}

/// 把整组记录拼成 JSON Tab 的「复制」文本。
fn format_records_for_json_copy(records: &[ExecutionRecord]) -> String {
    let mut out = String::new();
    for (idx, record) in records.iter().enumerate() {
        if idx > 0 {
            out.push('\n');
            out.push_str(&"─".repeat(40));
            out.push('\n');
        }
        if records.len() > 1 {
            out.push_str(&format!("[{}] {}\n", idx + 1, record.model_name));
        }
        let (input, output, metadata) = record_json_sections(record);
        out.push_str(&format!("输入:\n{input}\n\n"));
        out.push_str(&format!("输出:\n{output}\n\n"));
        out.push_str(&format!("Metadata:\n{metadata}\n"));
    }
    out
}

// ──────────────────────────────────────────────
// API 调用
// ──────────────────────────────────────────────

impl PromptDebugger {
    fn execute_call(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // 每次执行前清掉上一次的校验提示
        self.form_error = None;

        // 从 InputState 读取最新参数值
        if let Some(ref input) = self.temp_input
            && let Ok(v) = input.read(cx).value().to_string().parse::<f64>()
        {
            self.temperature = v;
        }
        if let Some(ref input) = self.max_tokens_input
            && let Ok(v) = input.read(cx).value().to_string().parse::<u32>()
        {
            self.max_tokens = v;
        }
        if let Some(ref input) = self.top_p_input
            && let Ok(v) = input.read(cx).value().to_string().parse::<f64>()
        {
            self.top_p = v;
        }
        if let Some(ref input) = self.presence_penalty_input
            && let Ok(v) = input.read(cx).value().to_string().parse::<f64>()
        {
            self.presence_penalty = v;
        }
        if let Some(ref input) = self.frequency_penalty_input
            && let Ok(v) = input.read(cx).value().to_string().parse::<f64>()
        {
            self.frequency_penalty = v;
        }
        if let Some(ref input) = self.thinking_budget_input
            && let Ok(v) = input.read(cx).value().to_string().parse::<u32>()
        {
            self.thinking_budget_tokens = v;
        }

        let preset_id = match self
            .selected_preset_id
            .filter(|id| self.presets.iter().any(|preset| preset.id == *id))
        {
            Some(id) => id,
            None => {
                let message = "左侧「LLM 设定」未设置：请先选择 Preset".to_string();
                self.form_error = Some(message.clone());
                self.call_state = CallState::Error("请先选择 Preset".into());
                window.push_notification(
                    Notification::error(message).placement(Anchor::BottomRight),
                    cx,
                );
                cx.notify();
                return;
            }
        };

        let model = match self.selected_model_id.and_then(|id| {
            self.models
                .iter()
                .find(|model| model.id == id && model.preset_id == Some(preset_id))
        }) {
            Some(m) => m.clone(),
            None => {
                let message = "左侧「LLM 设定」未设置：请选择当前 Preset 下的模型".to_string();
                self.form_error = Some(message.clone());
                self.call_state = CallState::Error("请选择当前 Preset 下的模型".into());
                window.push_notification(
                    Notification::error(message).placement(Anchor::BottomRight),
                    cx,
                );
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

        // 左侧设定校验通过：把这次使用的配置持久化，供下次打开时回填。
        self.save_settings(cx);

        // 仅用于失败日志的模型名（不包含任何凭据）。
        let log_model_name = model.name.clone();

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
                        // Provider 返回非 2xx：此前只把错误展示在结果窗口里，
                        // 后台没有任何日志。这里补一条结构化日志，便于定位
                        // 鉴权/模型名/额度等失败原因（不记录请求体与凭据）。
                        tracing::error!(
                            target: "hivegui::ui::prompt_debugger",
                            operation = "execute_call",
                            outcome = "http_status_error",
                            url = %url,
                            model = %log_model_name,
                            status = %status,
                            response = %truncate_for_log(&text),
                        );
                        Err(format!("HTTP {}: {}", status, text))
                    }
                }
                Err(e) => {
                    // 传输层失败（连接/超时/TLS 等）：UI 会显示
                    // 「请求失败: ...」，这里同步写一条后端日志。
                    //
                    // `reqwest::Error` 的 `Display` 只有最外层的一句
                    // "error sending request for url (...)"，真正的原因
                    // （Connection refused / timed out / 证书错误）在
                    // `source()` 链里，必须展开才能定位。
                    let chain = error_chain_for_log(&e);
                    let root_cause = root_cause_for_log(&e);
                    tracing::error!(
                        target: "hivegui::ui::prompt_debugger",
                        operation = "execute_call",
                        outcome = "request_failed",
                        url = %url,
                        model = %log_model_name,
                        has_token = token.is_some(),
                        root_cause = %root_cause,
                        error = %chain,
                    );
                    Err(format!("请求失败: {root_cause}（{url}）"))
                }
            };

            _ = this.update(cx, |view, cx| {
                view.finish_execution(response_text, cx);
            });
        })
        .detach();
    }

    /// 执行结束的统一收尾：写入 call_state、落库执行记录，并直接打开查看执行记录窗口。
    /// 无论成功或失败都打开，便于查看执行结果。
    fn finish_execution(&mut self, response_text: Result<String, String>, cx: &mut Context<Self>) {
        self.call_state = match response_text {
            Ok(text) => CallState::Success(text),
            Err(err) => CallState::Error(err),
        };
        self.save_execution_record();
        // 执行完成（无论成功或失败）后，直接打开查看执行记录窗口，便于查看执行结果
        if let Some(last_record) = self.execution_history.last() {
            self.view_records = vec![last_record.clone()];
            self.show_view_modal = true;
        }
        cx.notify();
    }
}

// ──────────────────────────────────────────────
// Render
// ──────────────────────────────────────────────

impl Render for PromptDebugger {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let style = ManagementStyle::from_theme(cx.theme());
        self.style = style;
        // 通知层：校验失败等提示以右下角 toast 形式弹出
        let notification_layer = Root::render_notification_layer(window, cx);

        // 懒初始化 InputState（默认值取自视图字段，已回填的持久化设置会在此体现）
        if self.temp_input.is_none() {
            self.temp_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("0.70")
                    .default_value(self.temperature.to_string())
            }));
        }
        if self.max_tokens_input.is_none() {
            self.max_tokens_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("2048")
                    .default_value(self.max_tokens.to_string())
            }));
        }
        if self.top_p_input.is_none() {
            self.top_p_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("1.00")
                    .default_value(self.top_p.to_string())
            }));
        }
        if self.presence_penalty_input.is_none() {
            self.presence_penalty_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("0.0")
                    .default_value(self.presence_penalty.to_string())
            }));
        }
        if self.frequency_penalty_input.is_none() {
            self.frequency_penalty_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("0.0")
                    .default_value(self.frequency_penalty.to_string())
            }));
        }
        if self.thinking_budget_input.is_none() {
            self.thinking_budget_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("1024")
                    .default_value(self.thinking_budget_tokens.to_string())
            }));
        }

        // 懒初始化 Preset 和模型下拉列表
        self.ensure_preset_select_state(window, cx);
        self.ensure_model_select_state(window, cx);

        let temp_input = self.temp_input.clone().unwrap();
        let max_tokens_input = self.max_tokens_input.clone().unwrap();
        let top_p_input = self.top_p_input.clone().unwrap();
        let presence_penalty_input = self.presence_penalty_input.clone().unwrap();
        let frequency_penalty_input = self.frequency_penalty_input.clone().unwrap();
        let thinking_budget_input = self.thinking_budget_input.clone().unwrap();
        let preset_select_state = self.preset_select_state.clone().unwrap();
        let model_select_state = self.model_select_state.clone().unwrap();

        // 弹窗最大宽度：留出左右各 4% 的遮罩区域，最小 640px。
        let max_panel_width = (window.viewport_size().width * 0.92).max(px(640.0));

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
                        &preset_select_state,
                        &model_select_state,
                        self.selected_preset_id.is_some(),
                        &temp_input,
                        &max_tokens_input,
                        &top_p_input,
                        &presence_penalty_input,
                        &frequency_penalty_input,
                        &thinking_budget_input,
                        self.thinking_enabled,
                        cx.entity(),
                        &style,
                        self.form_error.as_deref(),
                    ))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .child(self.message_editor(window, cx, &style, cx.entity())),
                    )
                    .child(right_panel(
                        &self.call_state,
                        self.selected_record_ids.len(),
                        self.execution_history.len(),
                        self.history_page,
                        &self.execution_history,
                        &self.selected_record_ids,
                        &style,
                        cx.entity(),
                    )),
            )
            .child(if self.show_view_modal {
                let entity = cx.entity();
                view_modal(
                    &self.view_records,
                    &style,
                    self.view_modal_tab,
                    self.show_only_diff,
                    &self.view_scroll,
                    entity,
                    max_panel_width,
                    &mut self.view_text_states,
                    window,
                    cx,
                )
                .into_any_element()
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
                            entity.update(cx, |view, cx| {
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
                            // 与查看弹窗同理：冒泡阶段拦下面板内点击，
                            // 避免误触遮罩层把整个选择器关掉。
                            .on_any_mouse_down(|_, _, cx| {
                                cx.stop_propagation();
                            })
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
                                                    entity.update(cx, |view, cx| {
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
                                    .child(div().w_full().child(Input::new(&cx.new(|cx| {
                                        InputState::new(window, cx).placeholder("搜索工具名称...")
                                    })))),
                            )
                            .child(
                                div()
                                    .id("prompt-tool-picker-scroll")
                                    .debug_selector(|| "PROMPT_TOOL_PICKER_SCROLL".to_owned())
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .min_h_0()
                                    .overflow_y_scroll()
                                    .track_scroll(&self.tool_picker_scroll)
                                    .vertical_scrollbar(&self.tool_picker_scroll)
                                    .px(px(16.0))
                                    .py(px(8.0))
                                    .child(
                                        self.available_tools
                                            .iter()
                                            .filter(|t| {
                                                self.tool_picker_search.is_empty()
                                                    || t.name.to_lowercase().contains(
                                                        &self.tool_picker_search.to_lowercase(),
                                                    )
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
                                                            s.bg(style
                                                                .action(ActionRole::Neutral)
                                                                .hover)
                                                        })
                                                        .on_mouse_down(MouseButton::Left, {
                                                            let entity = cx.entity();
                                                            move |_, _, cx| {
                                                                entity.update(cx, |view, cx| {
                                                                    view.add_tool_from_management(
                                                                        tool_id,
                                                                    );
                                                                    view.show_tool_picker = false;
                                                                    cx.notify();
                                                                });
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
                                                                        .font_weight(
                                                                            FontWeight::SEMIBOLD,
                                                                        )
                                                                        .child(SharedString::from(
                                                                            tool_name.as_str(),
                                                                        )),
                                                                )
                                                                .child(
                                                                    div()
                                                                        .text_size(px(12.0))
                                                                        .text_color(
                                                                            style
                                                                                .list
                                                                                .muted_foreground,
                                                                        )
                                                                        .child(SharedString::from(
                                                                            tool_identifier
                                                                                .as_str(),
                                                                        )),
                                                                ),
                                                        )
                                                        .child(
                                                            div()
                                                                .text_size(px(12.0))
                                                                .text_color(
                                                                    style.list.muted_foreground,
                                                                )
                                                                .child(SharedString::from(
                                                                    tool_description.as_str(),
                                                                )),
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
            .child(if let Some((record_id, position)) = self.context_menu {
                history_context_menu(record_id, position, &style, cx.entity(), window)
                    .into_any_element()
            } else {
                div().into_any_element()
            })
            .children(notification_layer)
    }
}

#[cfg(test)]
mod tests {
    use super::{DebugMessage, MessageRole, models_for_preset, valid_model_selection};
    use crate::datasource::llm_store::LlmModel;

    fn model(id: i64, preset_id: i64, priority: i32) -> LlmModel {
        LlmModel {
            id,
            name: format!("model-{id}"),
            preset_id: Some(preset_id),
            provider_id: Some(1),
            priority,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

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
        let id = 1u64;

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
        let mut msgs = [
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

        if let Some(pos) = msgs.iter().position(|m| m.id == 2)
            && pos > 0
        {
            msgs.swap(pos, pos - 1);
        }
        assert_eq!(msgs[0].id, 2);
        assert_eq!(msgs[1].id, 1);

        if let Some(pos) = msgs.iter().position(|m| m.id == 2)
            && pos + 1 < msgs.len()
        {
            msgs.swap(pos, pos + 1);
        }
        assert_eq!(msgs[0].id, 1);
        assert_eq!(msgs[1].id, 2);
    }

    #[test]
    fn model_selection_requires_a_preset_and_filters_to_its_models() {
        let models = vec![model(11, 1, 0), model(12, 1, 10), model(21, 2, 0)];

        assert!(models_for_preset(&models, None).is_empty());
        assert_eq!(
            models_for_preset(&models, Some(1))
                .iter()
                .map(|model| model.id)
                .collect::<Vec<_>>(),
            vec![11, 12]
        );
        assert_eq!(
            models_for_preset(&models, Some(2))
                .iter()
                .map(|model| model.id)
                .collect::<Vec<_>>(),
            vec![21]
        );
    }

    #[test]
    fn switching_preset_invalidates_the_previous_model() {
        let models = vec![model(11, 1, 0), model(21, 2, 0)];

        assert_eq!(valid_model_selection(&models, Some(1), Some(11)), Some(11));
        assert_eq!(valid_model_selection(&models, Some(2), Some(11)), None);
        assert_eq!(valid_model_selection(&models, None, Some(11)), None);
    }
}

#[cfg(test)]
mod geometry_tests {
    use super::{
        CallState, DebugMessage, ExecutionRecord, MessageRole, ModelSelectItem, PresetSelectItem,
        PromptDebugger, model_selector, preset_selector,
    };
    use crate::datasource::{Store, llm_store::LlmStore};
    use crate::ui::management_style::ManagementStyle;
    use gpui::{
        AppContext, Context, Entity, Focusable, InteractiveElement, IntoElement, Modifiers,
        MouseButton, ParentElement, Render, Styled, TestAppContext, VisualTestContext, Window, div,
        px, size,
    };
    use gpui_component::{
        ActiveTheme, Root,
        select::{SearchableVec, SelectState},
    };

    struct PromptDebuggerTestView;

    struct LlmSelectionTestView {
        preset: Entity<SelectState<SearchableVec<PresetSelectItem>>>,
        model: Entity<SelectState<SearchableVec<ModelSelectItem>>>,
    }

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

    impl Render for LlmSelectionTestView {
        fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let style = ManagementStyle::from_theme(cx.theme());
            div()
                .w(px(220.0))
                .flex()
                .flex_col()
                .gap(px(12.0))
                .child(preset_selector(&self.preset, &style))
                .child(model_selector(&self.model, false, &style))
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

    #[gpui::test]
    fn preset_selector_precedes_and_gates_model_selector(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::theme::init(cx);
            gpui_component::init(cx);
        });
        let window = cx.open_window(size(px(320.0), px(240.0)), |window, cx| {
            let preset = cx.new(|cx| {
                SelectState::new(
                    SearchableVec::new(vec![PresetSelectItem {
                        id: 1,
                        label: "default".to_string(),
                    }]),
                    None,
                    window,
                    cx,
                )
            });
            let model = cx.new(|cx| {
                SelectState::new(
                    SearchableVec::new(vec![ModelSelectItem {
                        id: 11,
                        label: "model-11".to_string(),
                    }]),
                    None,
                    window,
                    cx,
                )
            });
            LlmSelectionTestView { preset, model }
        });
        cx.run_until_parked();

        let typed_window = window;
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let preset = cx
            .debug_bounds("PROMPT_DEBUGGER_PRESET_SELECTOR")
            .expect("preset selector bounds");
        let model = cx
            .debug_bounds("PROMPT_DEBUGGER_MODEL_SELECTOR")
            .expect("model selector bounds");
        let model_trigger = cx
            .debug_bounds("PROMPT_DEBUGGER_MODEL_SELECT_TRIGGER")
            .expect("model select trigger bounds");

        assert!(preset.bottom() <= model.top());
        cx.simulate_click(model_trigger.center(), Modifiers::default());
        cx.run_until_parked();

        let model_is_focused = typed_window
            .update(&mut cx, |view, window, cx| {
                view.model.read(cx).focus_handle(cx).is_focused(window)
            })
            .expect("read model focus state");
        assert!(!model_is_focused, "disabled model selector accepted focus");
    }

    #[gpui::test]
    fn history_context_menu_opens_the_saved_result(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::theme::init(cx);
            gpui_component::init(cx);
        });

        let temp_dir = tempfile::tempdir().expect("create temporary data directory");
        let runtime = tokio::runtime::Runtime::new().expect("create Tokio runtime");
        let store = runtime
            .block_on(Store::new(temp_dir.path()))
            .expect("create test store");
        let llm_store = LlmStore::new(store.pool().clone(), store.crypto().clone());
        let _runtime_guard = runtime.enter();
        let store = cx.new(|_| store);

        let window = cx.open_window(size(px(1200.0), px(700.0)), move |_, cx| {
            let mut view = PromptDebugger::new_unloaded(cx, store.clone(), llm_store.clone());
            view.loaded = true;
            view.execution_history = vec![ExecutionRecord {
                id: 42,
                timestamp: 1,
                model_name: "history-model".to_string(),
                temperature: 0.7,
                max_tokens: 2048,
                top_p: 1.0,
                thinking_enabled: false,
                thinking_budget: 1024,
                messages: vec![DebugMessage {
                    id: 1,
                    role: MessageRole::User,
                    content: "history prompt".to_string(),
                }],
                tools: vec![],
                result: CallState::Success("historical result".to_string()),
            }];
            view
        });
        cx.run_until_parked();

        let typed_window = window;
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let history_row = cx
            .debug_bounds("PROMPT_HISTORY_RECORD_42")
            .expect("history row bounds");

        cx.simulate_mouse_down(
            history_row.center(),
            MouseButton::Right,
            Modifiers::default(),
        );
        cx.run_until_parked();

        let view_item = cx
            .debug_bounds("PROMPT_HISTORY_VIEW_42")
            .expect("history context-menu view item");
        let apply_item = cx
            .debug_bounds("PROMPT_HISTORY_APPLY_42")
            .expect("history context-menu apply item");
        for item in [view_item, apply_item] {
            assert!(item.left() >= px(0.0));
            assert!(item.top() >= px(0.0));
            assert!(item.right() <= px(1200.0));
            assert!(item.bottom() <= px(700.0));
        }

        cx.simulate_click(view_item.center(), Modifiers::default());
        cx.run_until_parked();

        let (show_view_modal, viewed_record_id, viewed_result) = typed_window
            .update(&mut cx, |view, _, _| {
                let record = view.view_records.first();
                (
                    view.show_view_modal,
                    record.map(|record| record.id),
                    record.and_then(|record| match &record.result {
                        CallState::Success(result) => Some(result.clone()),
                        _ => None,
                    }),
                )
            })
            .expect("read history view state");
        assert!(show_view_modal);
        assert_eq!(viewed_record_id, Some(42));
        assert_eq!(viewed_result.as_deref(), Some("historical result"));
        assert!(cx.debug_bounds("PROMPT_HISTORY_VIEW_MODAL").is_some());
    }

    #[gpui::test]
    fn completion_opens_result_view_on_success(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::theme::init(cx);
            gpui_component::init(cx);
        });

        let temp_dir = tempfile::tempdir().expect("create temporary data directory");
        let runtime = tokio::runtime::Runtime::new().expect("create Tokio runtime");
        let store = runtime
            .block_on(Store::new(temp_dir.path()))
            .expect("create test store");
        let llm_store = LlmStore::new(store.pool().clone(), store.crypto().clone());
        let _runtime_guard = runtime.enter();
        let store = cx.new(|_| store);

        let window = cx.open_window(size(px(1200.0), px(700.0)), move |_, cx| {
            PromptDebugger::new_unloaded(cx, store.clone(), llm_store.clone())
        });
        cx.run_until_parked();

        let typed_window = window;
        let mut cx = VisualTestContext::from_window(window.into(), cx);

        // 模拟执行成功收尾
        typed_window
            .update(&mut cx, |view, _, cx| {
                view.finish_execution(Ok("模型返回内容".to_string()), cx);
            })
            .expect("invoke finish_execution");
        cx.run_until_parked();

        let (show_modal, count, is_success) = typed_window
            .update(&mut cx, |view, _, _| {
                (
                    view.show_view_modal,
                    view.view_records.len(),
                    view.view_records
                        .first()
                        .map(|r| matches!(r.result, CallState::Success(_)))
                        .unwrap_or(false),
                )
            })
            .expect("read completion state");
        assert!(show_modal, "执行成功后应自动弹出结果查看窗口");
        assert_eq!(count, 1);
        assert!(is_success);
        assert!(
            cx.debug_bounds("PROMPT_HISTORY_VIEW_MODAL").is_some(),
            "结果查看窗口应渲染在界面上"
        );
    }

    #[gpui::test]
    fn completion_opens_result_view_on_error(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::theme::init(cx);
            gpui_component::init(cx);
        });

        let temp_dir = tempfile::tempdir().expect("create temporary data directory");
        let runtime = tokio::runtime::Runtime::new().expect("create Tokio runtime");
        let store = runtime
            .block_on(Store::new(temp_dir.path()))
            .expect("create test store");
        let llm_store = LlmStore::new(store.pool().clone(), store.crypto().clone());
        let _runtime_guard = runtime.enter();
        let store = cx.new(|_| store);

        let window = cx.open_window(size(px(1200.0), px(700.0)), move |_, cx| {
            PromptDebugger::new_unloaded(cx, store.clone(), llm_store.clone())
        });
        cx.run_until_parked();

        let typed_window = window;
        let mut cx = VisualTestContext::from_window(window.into(), cx);

        // 模拟执行失败收尾：失败时也应弹出以便查看错误结果
        typed_window
            .update(&mut cx, |view, _, cx| {
                view.finish_execution(Err("HTTP 500: 服务异常".to_string()), cx);
            })
            .expect("invoke finish_execution");
        cx.run_until_parked();

        let (show_modal, is_error) = typed_window
            .update(&mut cx, |view, _, _| {
                (
                    view.show_view_modal,
                    view.view_records
                        .first()
                        .map(|r| matches!(r.result, CallState::Error(_)))
                        .unwrap_or(false),
                )
            })
            .expect("read completion state");
        assert!(show_modal, "执行失败后也应自动弹出结果查看窗口");
        assert!(is_error, "查看窗口应展示错误结果");
        assert!(
            cx.debug_bounds("PROMPT_HISTORY_VIEW_MODAL").is_some(),
            "结果查看窗口应渲染在界面上"
        );
    }

    #[gpui::test]
    fn history_view_modal_body_expands_with_content(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::theme::init(cx);
            gpui_component::init(cx);
        });

        let temp_dir = tempfile::tempdir().expect("create temporary data directory");
        let runtime = tokio::runtime::Runtime::new().expect("create Tokio runtime");
        let store = runtime
            .block_on(Store::new(temp_dir.path()))
            .expect("create test store");
        let llm_store = LlmStore::new(store.pool().clone(), store.crypto().clone());
        let _runtime_guard = runtime.enter();
        let store = cx.new(|_| store);

        let window = cx.open_window(size(px(1200.0), px(700.0)), move |_, cx| {
            let mut view = PromptDebugger::new_unloaded(cx, store.clone(), llm_store.clone());
            view.loaded = true;
            view.view_records = vec![ExecutionRecord {
                id: 7,
                timestamp: 1,
                model_name: "history-model".to_string(),
                temperature: 0.7,
                max_tokens: 2048,
                top_p: 1.0,
                thinking_enabled: false,
                thinking_budget: 1024,
                messages: vec![DebugMessage {
                    id: 1,
                    role: MessageRole::User,
                    content: "history prompt".to_string(),
                }],
                tools: vec![],
                result: CallState::Success("historical result".to_string()),
            }];
            view.show_view_modal = true;
            view
        });
        cx.run_until_parked();

        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let overlay = cx
            .debug_bounds("PROMPT_HISTORY_VIEW_MODAL")
            .expect("view modal overlay bounds");
        let panel = cx
            .debug_bounds("PROMPT_HISTORY_VIEW_PANEL")
            .expect("view modal panel bounds");
        let body = cx
            .debug_bounds("PROMPT_HISTORY_VIEW_SCROLL")
            .expect("view modal body bounds");
        let card = cx
            .debug_bounds("PROMPT_HISTORY_VIEW_CARD")
            .expect("record card bounds");

        // 回归：内容区此前用 overflow_y_scrollbar()（Scrollable 包装器把高度
        // 换成百分比链条），在自动高度（仅 max_h）面板里内容贡献为 0，弹窗
        // 塌缩成只剩标题栏的空白窗口。
        assert!(
            panel.size.height > px(150.0),
            "view modal collapsed to title bar: {:?}",
            panel.size
        );
        assert!(
            body.size.height > px(60.0),
            "body collapsed: {:?}",
            body.size
        );
        assert!(
            card.size.height > px(0.0),
            "card collapsed: {:?}",
            card.size
        );
        assert!(body.top() >= panel.top());
        assert!(body.bottom() <= panel.bottom());
        assert!(card.bottom() <= body.bottom());
        assert!(panel.size.height <= px(600.0), "panel must respect max_h");
        assert!(panel.top() >= overlay.top());
    }

    #[gpui::test]
    fn history_view_modal_close_button_closes_the_modal(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::theme::init(cx);
            gpui_component::init(cx);
        });

        let temp_dir = tempfile::tempdir().expect("create temporary data directory");
        let runtime = tokio::runtime::Runtime::new().expect("create Tokio runtime");
        let store = runtime
            .block_on(Store::new(temp_dir.path()))
            .expect("create test store");
        let llm_store = LlmStore::new(store.pool().clone(), store.crypto().clone());
        let _runtime_guard = runtime.enter();
        let store = cx.new(|_| store);

        let window = cx.open_window(size(px(1200.0), px(700.0)), move |_, cx| {
            let mut view = PromptDebugger::new_unloaded(cx, store.clone(), llm_store.clone());
            view.loaded = true;
            view.view_records = vec![ExecutionRecord {
                id: 7,
                timestamp: 1,
                model_name: "history-model".to_string(),
                temperature: 0.7,
                max_tokens: 2048,
                top_p: 1.0,
                thinking_enabled: false,
                thinking_budget: 1024,
                messages: vec![DebugMessage {
                    id: 1,
                    role: MessageRole::User,
                    content: "history prompt".to_string(),
                }],
                tools: vec![],
                result: CallState::Success("historical result".to_string()),
            }];
            view.show_view_modal = true;
            view
        });
        cx.run_until_parked();

        let typed_window = window;
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let close = cx
            .debug_bounds("PROMPT_HISTORY_VIEW_CLOSE")
            .expect("close button bounds");

        cx.simulate_click(close.center(), Modifiers::default());
        cx.run_until_parked();

        let show_view_modal = typed_window
            .update(&mut cx, |view, _, _| view.show_view_modal)
            .expect("read state after close click");
        assert!(!show_view_modal, "close button must close the modal");
    }

    #[gpui::test]
    fn history_view_modal_copy_button_writes_clipboard(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::theme::init(cx);
            gpui_component::init(cx);
        });

        let temp_dir = tempfile::tempdir().expect("create temporary data directory");
        let runtime = tokio::runtime::Runtime::new().expect("create Tokio runtime");
        let store = runtime
            .block_on(Store::new(temp_dir.path()))
            .expect("create test store");
        let llm_store = LlmStore::new(store.pool().clone(), store.crypto().clone());
        let _runtime_guard = runtime.enter();
        let store = cx.new(|_| store);

        let window = cx.open_window(size(px(1200.0), px(700.0)), move |_, cx| {
            let mut view = PromptDebugger::new_unloaded(cx, store.clone(), llm_store.clone());
            view.loaded = true;
            view.view_records = vec![ExecutionRecord {
                id: 7,
                timestamp: 1,
                model_name: "history-model".to_string(),
                temperature: 0.7,
                max_tokens: 2048,
                top_p: 1.0,
                thinking_enabled: false,
                thinking_budget: 1024,
                messages: vec![DebugMessage {
                    id: 1,
                    role: MessageRole::User,
                    content: "history prompt".to_string(),
                }],
                tools: vec![],
                result: CallState::Success("historical result".to_string()),
            }];
            view.show_view_modal = true;
            view
        });
        cx.run_until_parked();

        let typed_window = window;
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let copy = cx
            .debug_bounds("PROMPT_HISTORY_VIEW_COPY")
            .expect("copy button bounds");

        cx.simulate_click(copy.center(), Modifiers::default());
        cx.run_until_parked();

        let clipboard = typed_window
            .update(&mut cx, |_, _, cx| {
                cx.read_from_clipboard().and_then(|item| item.text())
            })
            .expect("read clipboard after copy click")
            .unwrap_or_default();
        assert!(
            clipboard.contains("history-model"),
            "clipboard should contain the model name, got {clipboard:?}"
        );
        assert!(
            clipboard.contains("historical result"),
            "clipboard should contain the result, got {clipboard:?}"
        );
    }

    #[gpui::test]
    fn history_view_modal_json_tab_shows_sections_and_copies_input(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::theme::init(cx);
            gpui_component::init(cx);
        });

        let temp_dir = tempfile::tempdir().expect("create temporary data directory");
        let runtime = tokio::runtime::Runtime::new().expect("create Tokio runtime");
        let store = runtime
            .block_on(Store::new(temp_dir.path()))
            .expect("create test store");
        let llm_store = LlmStore::new(store.pool().clone(), store.crypto().clone());
        let _runtime_guard = runtime.enter();
        let store = cx.new(|_| store);

        let window = cx.open_window(size(px(1200.0), px(700.0)), move |_, cx| {
            let mut view = PromptDebugger::new_unloaded(cx, store.clone(), llm_store.clone());
            view.loaded = true;
            view.view_records = vec![ExecutionRecord {
                id: 7,
                timestamp: 1,
                model_name: "history-model".to_string(),
                temperature: 0.7,
                max_tokens: 2048,
                top_p: 1.0,
                thinking_enabled: false,
                thinking_budget: 1024,
                messages: vec![DebugMessage {
                    id: 1,
                    role: MessageRole::User,
                    content: "history prompt".to_string(),
                }],
                tools: vec![],
                result: CallState::Success("historical result".to_string()),
            }];
            view.show_view_modal = true;
            view
        });
        cx.run_until_parked();

        let typed_window = window;
        let mut cx = VisualTestContext::from_window(window.into(), cx);

        // 切到 JSON Tab
        let json_tab = cx
            .debug_bounds("PROMPT_VIEW_TAB_JSON")
            .expect("json tab bounds");
        cx.simulate_click(json_tab.center(), Modifiers::default());
        cx.run_until_parked();

        let input = cx
            .debug_bounds("PROMPT_VIEW_JSON_INPUT_1_PANEL")
            .expect("input panel bounds");
        let output = cx
            .debug_bounds("PROMPT_VIEW_JSON_OUTPUT_1_PANEL")
            .expect("output panel bounds");
        let metadata = cx
            .debug_bounds("PROMPT_VIEW_JSON_METADATA_1_PANEL")
            .expect("metadata panel bounds");

        assert!(input.size.height > px(0.0), "input panel collapsed");
        assert!(output.size.height > px(0.0), "output panel collapsed");
        assert!(metadata.size.height > px(0.0), "metadata panel collapsed");
        assert!(input.bottom() <= output.top(), "input must precede output");
        assert!(
            output.bottom() <= metadata.top(),
            "output must precede metadata"
        );

        // 分节复制按钮：复制的是该节的 pretty JSON
        let copy_input = cx
            .debug_bounds("PROMPT_VIEW_JSON_INPUT_1_COPY")
            .expect("input copy button bounds");
        cx.simulate_click(copy_input.center(), Modifiers::default());
        cx.run_until_parked();

        let clipboard = typed_window
            .update(&mut cx, |_, _, cx| {
                cx.read_from_clipboard().and_then(|item| item.text())
            })
            .expect("read clipboard after section copy click")
            .unwrap_or_default();
        assert!(
            clipboard.contains("\"model\": \"history-model\""),
            "input section clipboard should contain the request model, got {clipboard:?}"
        );
        assert!(
            clipboard.contains("\"content\": \"history prompt\""),
            "input section clipboard should contain the message, got {clipboard:?}"
        );
        assert!(
            !clipboard.contains("historical result"),
            "input section copy must not include the output, got {clipboard:?}"
        );
    }

    #[gpui::test]
    fn history_comparison_modal_renders_selectable_text(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::theme::init(cx);
            gpui_component::init(cx);
        });

        let temp_dir = tempfile::tempdir().expect("create temporary data directory");
        let runtime = tokio::runtime::Runtime::new().expect("create Tokio runtime");
        let store = runtime
            .block_on(Store::new(temp_dir.path()))
            .expect("create test store");
        let llm_store = LlmStore::new(store.pool().clone(), store.crypto().clone());
        let _runtime_guard = runtime.enter();
        let store = cx.new(|_| store);

        let window = cx.open_window(size(px(1200.0), px(700.0)), move |_, cx| {
            let mut view = PromptDebugger::new_unloaded(cx, store.clone(), llm_store.clone());
            view.loaded = true;
            view.view_records = vec![
                ExecutionRecord {
                    id: 7,
                    timestamp: 1,
                    model_name: "first-model".to_string(),
                    temperature: 0.7,
                    max_tokens: 2048,
                    top_p: 1.0,
                    thinking_enabled: false,
                    thinking_budget: 1024,
                    messages: vec![DebugMessage {
                        id: 1,
                        role: MessageRole::User,
                        content: "first prompt".to_string(),
                    }],
                    tools: vec![],
                    result: CallState::Success("first result".to_string()),
                },
                ExecutionRecord {
                    id: 8,
                    timestamp: 2,
                    model_name: "second-model".to_string(),
                    temperature: 0.7,
                    max_tokens: 2048,
                    top_p: 1.0,
                    thinking_enabled: false,
                    thinking_budget: 1024,
                    messages: vec![DebugMessage {
                        id: 1,
                        role: MessageRole::User,
                        content: "second prompt".to_string(),
                    }],
                    tools: vec![],
                    result: CallState::Success("second result".to_string()),
                },
            ];
            view.show_view_modal = true;
            view
        });
        cx.run_until_parked();

        let typed_window = window;
        let mut cx = VisualTestContext::from_window(window.into(), cx);

        let body = cx
            .debug_bounds("PROMPT_HISTORY_VIEW_SCROLL")
            .expect("comparison body bounds");
        assert!(
            body.size.height > px(60.0),
            "comparison body collapsed: {:?}",
            body.size
        );

        // 回归：对比表格此前用纯 `div` 渲染文本，鼠标拖选和复制都不可用；
        // 现在每个单元格都由一个只读 TextareaState 承载。
        let cells = typed_window
            .update(&mut cx, |view, _window, cx| {
                view.view_text_states
                    .iter()
                    .map(|(key, state)| (key.clone(), state.read(cx).value().to_string()))
                    .collect::<Vec<_>>()
            })
            .expect("read comparison text states");

        let first = cells
            .iter()
            .find(|(key, _)| key == "cmp:7:结果")
            .unwrap_or_else(|| panic!("record 7 result cell missing, got {cells:?}"));
        assert_eq!(first.1, "first result");
        let second = cells
            .iter()
            .find(|(key, _)| key == "cmp:8:结果")
            .unwrap_or_else(|| panic!("record 8 result cell missing, got {cells:?}"));
        assert_eq!(second.1, "second result");

        // 序号行 / 表头行 / 维度标签此前也是纯 `div`，同样要能选中复制。
        for (key, expected) in [
            ("cmp:index:label", "序号"),
            ("cmp:index:7", "#1"),
            ("cmp:index:8", "#2"),
            ("cmp:header:label", "维度"),
            ("cmp:header:7", "first-model"),
            ("cmp:header:8", "second-model"),
            ("cmp:dim:结果", "结果"),
        ] {
            let (_, value) = cells
                .iter()
                .find(|(cell_key, _)| cell_key == key)
                .unwrap_or_else(|| panic!("{key} missing, got {cells:?}"));
            assert_eq!(value, expected, "unexpected text for {key}");
        }

        // 正文必须只读：允许选中复制，但不能把历史记录改掉。
        let all_readonly = typed_window
            .update(&mut cx, |view, _window, cx| {
                view.view_text_states
                    .values()
                    .all(|state| !state.read(cx).base_state().read(cx).is_editable())
            })
            .expect("read comparison text readonly flags");
        assert!(all_readonly, "comparison text must be read-only");
    }

    #[gpui::test]
    fn execute_without_preset_shows_left_form_error(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::theme::init(cx);
            gpui_component::init(cx);
        });

        let temp_dir = tempfile::tempdir().expect("create temporary data directory");
        let runtime = tokio::runtime::Runtime::new().expect("create Tokio runtime");
        let store = runtime
            .block_on(Store::new(temp_dir.path()))
            .expect("create test store");
        let llm_store = LlmStore::new(store.pool().clone(), store.crypto().clone());
        let _runtime_guard = runtime.enter();
        let store = cx.new(|_| store);

        let prompt =
            cx.new(|cx| PromptDebugger::new_unloaded(cx, store.clone(), llm_store.clone()));
        let window = cx.open_window(size(px(1200.0), px(700.0)), {
            let prompt = prompt.clone();
            // 通知系统依赖窗口根为 Root（生产中即如此），测试中显式包裹
            move |window, cx| Root::new(prompt, window, cx)
        });
        cx.run_until_parked();

        let mut cx = VisualTestContext::from_window(window.into(), cx);

        // 模拟点击「执行」按钮（无 Preset/模型），触发校验失败路径
        let execute_btn = cx
            .debug_bounds("PROMPT_DEBUGGER_EXECUTE_BTN")
            .expect("execute button rendered");
        cx.simulate_click(execute_btn.center(), Modifiers::default());
        cx.run_until_parked();

        let form_error = prompt.read_with(&cx, |view, _| view.form_error.clone());
        assert_eq!(
            form_error.as_deref(),
            Some("左侧「LLM 设定」未设置：请先选择 Preset")
        );
        // 提示横幅应渲染在左侧面板中
        assert!(
            cx.debug_bounds("PROMPT_DEBUGGER_FORM_ERROR").is_some(),
            "form error banner should be visible"
        );

        // 右下角应弹出一条自动消失（-notification 默认 5 秒）的 toast 通知
        let notification_count = cx.update(|window, cx| match window.root::<Root>() {
            Some(Some(root)) => root.read(cx).notification.read(cx).notifications().len(),
            _ => 0,
        });
        assert!(
            notification_count >= 1,
            "a bottom-right notification toast should be shown"
        );
    }

    #[gpui::test]
    fn selecting_preset_or_model_clears_left_form_error(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::theme::init(cx);
            gpui_component::init(cx);
        });

        let temp_dir = tempfile::tempdir().expect("create temporary data directory");
        let runtime = tokio::runtime::Runtime::new().expect("create Tokio runtime");
        let store = runtime
            .block_on(Store::new(temp_dir.path()))
            .expect("create test store");
        let llm_store = LlmStore::new(store.pool().clone(), store.crypto().clone());
        let _runtime_guard = runtime.enter();
        let store = cx.new(|_| store);

        let window = cx.open_window(size(px(1200.0), px(700.0)), move |_, cx| {
            let mut view = PromptDebugger::new_unloaded(cx, store.clone(), llm_store.clone());
            // 模拟此前已因未选择而弹出的提示
            view.form_error = Some("左侧「LLM 设定」未设置：请先选择 Preset".to_string());
            view
        });
        cx.run_until_parked();

        let typed_window = window;
        let mut cx = VisualTestContext::from_window(window.into(), cx);

        // 选择 Preset 后提示应被清除
        typed_window
            .update(&mut cx, |view, window, cx| {
                let state = view.preset_select_state.clone().unwrap();
                view.on_preset_select(
                    &state,
                    &gpui_component::select::SelectEvent::Confirm(Some(1)),
                    window,
                    cx,
                );
            })
            .expect("invoke on_preset_select");
        cx.run_until_parked();

        let form_error = typed_window
            .update(&mut cx, |view, _, _| view.form_error.clone())
            .expect("read form_error after preset select");
        assert!(
            form_error.is_none(),
            "selecting a preset should clear the form error"
        );
    }

    fn history_record(id: u64) -> ExecutionRecord {
        ExecutionRecord {
            id,
            timestamp: id,
            model_name: format!("model-{id}"),
            temperature: 0.7,
            max_tokens: 2048,
            top_p: 1.0,
            thinking_enabled: false,
            thinking_budget: 1024,
            messages: vec![DebugMessage {
                id: 1,
                role: MessageRole::User,
                content: format!("prompt-{id}"),
            }],
            tools: vec![],
            result: CallState::Success(format!("result-{id}")),
        }
    }

    /// 打开一个带 `count` 条历史记录的调试页。
    ///
    /// 返回的 `TempDir` / `Runtime` 必须由调用方持有到用例结束，
    /// 否则数据库文件会在用例执行期间被提前清理。
    #[allow(clippy::type_complexity)]
    fn open_debugger_with_history(
        cx: &mut TestAppContext,
        count: u64,
    ) -> (
        gpui::WindowHandle<PromptDebugger>,
        tempfile::TempDir,
        &'static tokio::runtime::Runtime,
    ) {
        cx.update(|cx| {
            gpui_component::theme::init(cx);
            gpui_component::init(cx);
        });

        let temp_dir = tempfile::tempdir().expect("create temporary data directory");
        // 泄漏 Runtime：sqlx 的后台任务需要 tokio 上下文在用例期间一直有效。
        let runtime: &'static tokio::runtime::Runtime = Box::leak(Box::new(
            tokio::runtime::Runtime::new().expect("create Tokio runtime"),
        ));
        let store = runtime
            .block_on(Store::new(temp_dir.path()))
            .expect("create test store");
        let llm_store = LlmStore::new(store.pool().clone(), store.crypto().clone());
        std::mem::forget(runtime.enter());
        let store = cx.new(|_| store);

        let window = cx.open_window(size(px(1200.0), px(700.0)), move |_, cx| {
            let mut view = PromptDebugger::new_unloaded(cx, store.clone(), llm_store.clone());
            view.loaded = true;
            view.execution_history = (1..=count).map(history_record).collect();
            view
        });
        (window, temp_dir, runtime)
    }

    #[gpui::test]
    fn history_panel_pagination_selection_and_deletion(cx: &mut TestAppContext) {
        let (window, _temp_dir, _runtime) = open_debugger_with_history(cx, 12);
        cx.run_until_parked();

        let typed_window = window;
        let mut cx = VisualTestContext::from_window(window.into(), cx);

        // ── 分页：每页 10 条，最新的排在最前 ──
        assert!(cx.debug_bounds("PROMPT_HISTORY_RECORD_12").is_some());
        assert!(cx.debug_bounds("PROMPT_HISTORY_RECORD_3").is_some());
        assert!(
            cx.debug_bounds("PROMPT_HISTORY_RECORD_2").is_none(),
            "第二页的记录不应出现在第一页"
        );

        let next_page = cx
            .debug_bounds("PROMPT_HISTORY_NEXT_PAGE")
            .expect("next page button");
        cx.simulate_click(next_page.center(), Modifiers::default());
        cx.run_until_parked();

        assert_eq!(
            typed_window
                .update(&mut cx, |view, _, _| view.current_history_page())
                .expect("read current page"),
            1
        );
        assert!(cx.debug_bounds("PROMPT_HISTORY_RECORD_2").is_some());
        assert!(
            cx.debug_bounds("PROMPT_HISTORY_RECORD_12").is_none(),
            "翻页后第一页的记录不应再渲染"
        );

        let prev_page = cx
            .debug_bounds("PROMPT_HISTORY_PREV_PAGE")
            .expect("prev page button");
        cx.simulate_click(prev_page.center(), Modifiers::default());
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("PROMPT_HISTORY_RECORD_12").is_some(),
            "返回上一页后应重新看到最新记录"
        );

        // ── 全选 / 反选 ──
        let select_all = cx
            .debug_bounds("PROMPT_HISTORY_SELECT_ALL")
            .expect("select all button");
        cx.simulate_click(select_all.center(), Modifiers::default());
        cx.run_until_parked();
        assert_eq!(
            typed_window
                .update(&mut cx, |view, _, _| view.selected_record_ids.len())
                .expect("read selected count"),
            12
        );

        let invert = cx
            .debug_bounds("PROMPT_HISTORY_INVERT_SELECTION")
            .expect("invert selection button");
        cx.simulate_click(invert.center(), Modifiers::default());
        cx.run_until_parked();
        assert_eq!(
            typed_window
                .update(&mut cx, |view, _, _| view.selected_record_ids.len())
                .expect("read selected count after invert"),
            0
        );

        cx.simulate_click(invert.center(), Modifiers::default());
        cx.run_until_parked();
        assert_eq!(
            typed_window
                .update(&mut cx, |view, _, _| view.selected_record_ids.len())
                .expect("read selected count after second invert"),
            12
        );

        // ── 清除按钮只删除选中的记录 ──
        typed_window
            .update(&mut cx, |view, _, _| {
                view.deselect_all_records();
                view.toggle_record_selection(1);
                view.toggle_record_selection(2);
            })
            .expect("select records 1 and 2");
        cx.run_until_parked();

        let clear = cx
            .debug_bounds("PROMPT_HISTORY_CLEAR_SELECTED")
            .expect("clear selected button");
        cx.simulate_click(clear.center(), Modifiers::default());
        cx.run_until_parked();

        let remaining = typed_window
            .update(&mut cx, |view, _, _| {
                view.execution_history
                    .iter()
                    .map(|r| r.id)
                    .collect::<Vec<_>>()
            })
            .expect("read remaining ids");
        assert_eq!(remaining, (3..=12).collect::<Vec<u64>>());
        assert_eq!(
            typed_window
                .update(&mut cx, |view, _, _| view.selected_record_ids.len())
                .expect("read selected count after clear"),
            0
        );
        // 记录减少后总页数回到 1，分页条应消失
        assert!(cx.debug_bounds("PROMPT_HISTORY_NEXT_PAGE").is_none());

        // ── 右键菜单删除当前记录 ──
        let row = cx
            .debug_bounds("PROMPT_HISTORY_RECORD_12")
            .expect("history row for record 12");
        cx.simulate_mouse_down(row.center(), MouseButton::Right, Modifiers::default());
        cx.run_until_parked();

        let delete_item = cx
            .debug_bounds("PROMPT_HISTORY_DELETE_12")
            .expect("context menu delete item");
        cx.simulate_click(delete_item.center(), Modifiers::default());
        cx.run_until_parked();

        let remaining = typed_window
            .update(&mut cx, |view, _, _| {
                view.execution_history
                    .iter()
                    .map(|r| r.id)
                    .collect::<Vec<_>>()
            })
            .expect("read remaining ids after delete");
        assert_eq!(remaining, (3..=11).collect::<Vec<u64>>());
        assert!(
            typed_window
                .update(&mut cx, |view, _, _| view.context_menu.is_none())
                .expect("read context menu state"),
            "删除后右键菜单应关闭"
        );
    }
}

#[cfg(test)]
mod error_chain_tests {
    use super::{error_chain_for_log, root_cause_for_log};

    #[derive(Debug)]
    struct Chain {
        msg: &'static str,
        source: Option<Box<Chain>>,
    }

    impl std::fmt::Display for Chain {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(self.msg)
        }
    }

    impl std::error::Error for Chain {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            self.source
                .as_ref()
                .map(|s| s.as_ref() as &(dyn std::error::Error + 'static))
        }
    }

    #[test]
    fn transport_error_chain_exposes_the_root_cause() {
        // 模拟 reqwest 的连接失败：Display 只有最外层一句话，
        // 根因藏在 source 链里。
        let error = Chain {
            msg: "error sending request for url (http://127.0.0.1:11434/v1/chat/completions)",
            source: Some(Box::new(Chain {
                msg: "client error (Connect)",
                source: Some(Box::new(Chain {
                    msg: "Connection refused (os error 111)",
                    source: None,
                })),
            })),
        };

        assert_eq!(
            error_chain_for_log(&error),
            "error sending request for url (http://127.0.0.1:11434/v1/chat/completions) \
             <- client error (Connect) <- Connection refused (os error 111)"
        );
        assert_eq!(
            root_cause_for_log(&error),
            "Connection refused (os error 111)"
        );
    }

    #[test]
    fn error_without_source_falls_back_to_its_own_message() {
        let error = Chain {
            msg: "boom",
            source: None,
        };
        assert_eq!(error_chain_for_log(&error), "boom");
        assert_eq!(root_cause_for_log(&error), "boom");
    }
}
