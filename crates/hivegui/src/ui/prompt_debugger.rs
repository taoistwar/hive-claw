//! LLM 提示词调试工具 — 三栏布局：LLM 设置 / 消息编辑 / 结果展示。

use gpui::*;
use gpui_component::{ActiveTheme as _, Icon, IconName, Sizable, select::{Select, SelectState, SearchableVec}};
use gpui_component::input::{Input, InputState};
use gpui_component::scroll::ScrollableElement;
use crate::datasource::llm_store::{LlmModel, LlmProvider, LlmStore};
use crate::datasource::Crypto;
use crate::ui::management_style::{ActionRole, ManagementStyle};
use std::collections::HashMap;

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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
        &[MessageRole::System, MessageRole::User, MessageRole::Assistant]
    }
}

/// 一条调试消息
#[derive(Debug, Clone)]
pub struct DebugMessage {
    pub id: u64,
    pub role: MessageRole,
    pub content: String,
}

/// API 调用状态
#[derive(Debug, Clone)]
pub enum CallState {
    Idle,
    Loading,
    Success(String),
    Error(String),
}

pub struct PromptDebugger {
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
}

impl PromptDebugger {
    pub fn new(cx: &mut Context<Self>, llm_store: LlmStore) -> Self {
        let crypto = llm_store.crypto().clone();
        let style = ManagementStyle::from_theme(cx.theme());
        let temp_input = None;
        let max_tokens_input = None;
        let top_p_input = None;
        let presence_penalty_input = None;
        let frequency_penalty_input = None;
        let thinking_budget_input = None;
        let mut this = Self {
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
        };
        this.load_data(cx);
        this
    }

    fn load_data(&mut self, cx: &mut Context<Self>) {
        let store = self.llm_store.clone();
        cx.spawn(async move |this, cx| {
            let models = store.list_models().await.unwrap_or_default();
            let providers = store.list_providers().await.unwrap_or_default();
            _ = this.update(cx, |this, cx| {
                this.models = models;
                this.providers = providers;
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
        let select_state = cx.new(|cx| {
            SelectState::new(items, initial_index, window, cx).searchable(true)
        });

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

    fn get_message_input(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) -> Entity<InputState> {
        self.message_inputs.entry(id).or_insert_with(|| {
            let content = self.messages.iter().find(|m| m.id == id)
                .map(|m| m.content.clone())
                .unwrap_or_default();
            cx.new(|cx| InputState::new(window, cx).multi_line(true).auto_grow(1, 5) .default_value(&content))
        }).clone()
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
                .child(
                    div()
                        .w(px(60.0))
                        .child(Input::new(input)),
                ),
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
                .child(
                    div()
                        .w(px(60.0))
                        .child(Input::new(input)),
                ),
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
                    Icon::new(if enabled { IconName::Bot } else { IconName::Settings })
                        .xsmall()
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

    let collapse_icon = if collapsed { IconName::ChevronRight } else { IconName::ChevronDown };
    let edit_icon = if editing { IconName::Check } else { IconName::Replace };

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
                                            view.toggle_edit(msg_id);
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
        let summary = if content.len() > 40 {
            format!("{}...", &content[..40])
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
                    .child(
                        div()
                            .w_full()
                            .child(Input::new(&input_state)),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(px(6.0))
                            .child(
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
        let msg_states: Vec<(u64, bool, bool)> = self.messages.iter().map(|msg| {
            let collapsed = self.collapsed_messages.get(&msg.id).copied().unwrap_or(false);
            let editing = self.editing_messages.get(&msg.id).copied().unwrap_or(false);
            (msg.id, collapsed, editing)
        }).collect();

        // 为需要编辑的消息预创建 InputState
        let mut input_map: HashMap<u64, Entity<InputState>> = HashMap::new();
        for (id, _, editing) in &msg_states {
            if *editing {
                let input = self.get_message_input(*id, window, cx);
                input_map.insert(*id, input);
            }
        }

        for msg in &self.messages {
            let (_, collapsed, editing) = msg_states.iter().find(|(id, _, _)| *id == msg.id).unwrap();
            let input = input_map.remove(&msg.id);
            content = content.child(message_card(msg, style, entity.clone(), *collapsed, *editing, input));
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
    div()
        .mt(px(12.0))
        .child(
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
                        .child(
                            if is_loading {
                                Icon::new(IconName::LoaderCircle).small()
                            } else {
                                Icon::new(IconName::Play).small()
                            }
                        )
                        .child(
                            div()
                                .text_size(px(13.0))
                                .child(if is_loading { "执行中..." } else { "执行" })
                        ),
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

/// 右侧结果面板
fn results_panel(call_state: &CallState, style: &ManagementStyle) -> impl IntoElement {
    let content = match call_state {
        CallState::Idle => div()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .flex_1()
            .text_color(style.list.muted_foreground)
            .child(
                div()
                    .text_size(px(24.0))
                    .mb(px(8.0))
                    .child("▷"),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .child("点击「执行」按钮查看响应"),
            )
            .into_any_element(),
        CallState::Loading => div()
            .flex()
            .items_center()
            .justify_center()
            .flex_1()
            .text_color(style.list.muted_foreground)
            .child("加载中...")
            .into_any_element(),
        CallState::Success(text) => div()
            .text_size(px(13.0))
            .whitespace_normal()
            .child(text.clone())
            .into_any_element(),
        CallState::Error(err) => div()
            .text_size(px(13.0))
            .text_color(style.action(ActionRole::Delete).foreground)
            .child(err.clone())
            .into_any_element(),
    };

    div()
        .w(px(320.0))
        .flex_shrink_0()
        .h_full()
        .p(px(12.0))
        .border_l_1()
        .border_color(style.list.border)
        .overflow_y_scrollbar()
        .child(
            div()
                .text_size(px(14.0))
                .font_weight(FontWeight::BOLD)
                .mb(px(12.0))
                .child("Results"),
        )
        .child(content)
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

        let token = provider.and_then(|p| {
            p.token_encrypted
                .and_then(|enc| self.crypto.decrypt(&enc).ok())
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

            let mut request = client
                .post(&url)
                .header("Content-Type", "application/json");

            if let Some(ref token) = token {
                request = request.header("Authorization", format!("Bearer {}", token));
            }

            let result = request.json(&body).send().await;

            let response_text = match result {
                Ok(resp) => {
                    let status = resp.status();
                    let text = resp.text().await.unwrap_or_default();
                    if status.is_success() {
                        if let Ok(json) =
                            serde_json::from_str::<serde_json::Value>(&text)
                        {
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
                                Ok(format!("【思考过程】\n{}\n\n【最终回复】\n{}", reasoning, content))
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
                view.call_state = match response_text {
                    Ok(text) => CallState::Success(text),
                    Err(err) => CallState::Error(err),
                };
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
                            ))
                            .child(
                                div()
                                    .px(px(12.0))
                                    .pb(px(12.0))
                                    .child(execute_button(
                                        &self.call_state,
                                        &style,
                                        cx.entity(),
                                    )),
                            ),
                    )
                    .child(results_panel(&self.call_state, &style)),
            )
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
        msgs.push(DebugMessage { id, role: MessageRole::User, content: "hi".into() });
        assert_eq!(msgs.len(), 1);

        msgs.retain(|m| m.id != id);
        assert_eq!(msgs.len(), 0);
    }

    #[test]
    fn move_message_up_and_down() {
        let mut msgs = vec![
            DebugMessage { id: 1, role: MessageRole::System, content: "a".into() },
            DebugMessage { id: 2, role: MessageRole::User, content: "b".into() },
            DebugMessage { id: 3, role: MessageRole::Assistant, content: "c".into() },
        ];

        if let Some(pos) = msgs.iter().position(|m| m.id == 2) {
            if pos > 0 { msgs.swap(pos, pos - 1); }
        }
        assert_eq!(msgs[0].id, 2);
        assert_eq!(msgs[1].id, 1);

        if let Some(pos) = msgs.iter().position(|m| m.id == 2) {
            if pos + 1 < msgs.len() { msgs.swap(pos, pos + 1); }
        }
        assert_eq!(msgs[0].id, 1);
        assert_eq!(msgs[1].id, 2);
    }
}

#[cfg(test)]
mod geometry_tests {
    use gpui::{
        div, px, size, Context, InteractiveElement, IntoElement, ParentElement, Render, Styled,
        TestAppContext, VisualTestContext, Window,
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
        let settings = cx.debug_bounds("SETTINGS_PANEL").expect("settings panel bounds");
        let editor = cx.debug_bounds("MESSAGE_EDITOR").expect("message editor bounds");
        let results = cx.debug_bounds("RESULTS_PANEL").expect("results panel bounds");

        assert_eq!(settings.size.width, px(220.0));
        assert_eq!(results.size.width, px(320.0));
        assert_eq!(settings.right(), editor.left());
        assert_eq!(editor.right(), results.left());
        assert_eq!(settings.top(), results.top());
    }
}
