# LLM 提示词调试工具 实现计划

> **面向 AI 代理的工作者：** 必需子技能：使用 superpowers:subagent-driven-development（推荐）或 superpowers:executing-plans 逐任务实现此计划。步骤使用复选框（`- [ ]`）语法来跟踪进度。

**目标：** 在 HiveGUI 工具 Tab 中新增"LLM 提示词调试"页面，提供三栏布局（LLM 设置 / 消息编辑 / 结果展示），支持真实 API 调用。

**架构：** 新增 `prompt_debugger.rs` 作为主视图，从 `LlmStore` 读取模型/Provider 配置，通过 `reqwest` 发送 OpenAI 兼容格式请求。在 `UtilityView` 中新增 Tab 并调整顺序。

**技术栈：** Rust + gpui + gpui-component + sqlx + reqwest + tokio

---

## 文件清单

| 文件 | 操作 | 职责 |
|------|------|------|
| `crates/hivegui/Cargo.toml` | 修改 | 添加 `reqwest` 依赖 |
| `crates/hivegui/src/ui/prompt_debugger.rs` | 创建 | 提示词调试工具主视图（三栏布局 + 数据模型 + API 调用） |
| `crates/hivegui/src/ui/utility_view.rs` | 修改 | 新增 PromptDebugger Tab，调整顺序（调试在前，数据源在后） |
| `crates/hivegui/src/ui/mod.rs` | 修改 | 导出 `prompt_debugger` 模块 |
| `crates/hivegui/src/ui/app.rs` | 修改 | RootView 中初始化 PromptDebugger 并传入 LlmStore |

---

### 任务 1：添加 reqwest 依赖

**文件：**
- 修改：`crates/hivegui/Cargo.toml`

- [ ] **步骤 1：在 hivegui Cargo.toml 中添加 reqwest 依赖**

在 `[dependencies]` 段中添加：

```toml
reqwest = { workspace = true }
```

- [ ] **步骤 2：验证依赖解析**

运行：`cargo check -p hivegui`
预期：PASS（无新增错误）

- [ ] **步骤 3：Commit**

```bash
git add crates/hivegui/Cargo.toml
git commit -m "feat(hivegui): add reqwest dependency for LLM API calls"
```

---

### 任务 2：创建 PromptDebugger 数据模型和状态

**文件：**
- 创建：`crates/hivegui/src/ui/prompt_debugger.rs`

- [ ] **步骤 1：编写数据模型和结构体定义**

```rust
//! LLM 提示词调试工具 — 三栏布局：LLM 设置 / 消息编辑 / 结果展示。

use std::sync::Arc;

use gpui::*;
use gpui_component::ActiveTheme as _;
use gpui_component::input::{Input, InputState};
use gpui_component::scroll::ScrollableElement;
use gpui_component::tab::{Tab, TabBar};

use crate::datasource::llm_store::{LlmModel, LlmProvider, LlmStore};
use crate::datasource::crypto::Crypto;
use crate::ui::management_style::ManagementStyle;

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
    temperature: f64,
    max_tokens: u32,
    top_p: f64,
    // 中间：消息
    messages: Vec<DebugMessage>,
    next_msg_id: u64,
    // 右侧：结果
    call_state: CallState,
    // UI 状态
    loaded: bool,
    style: ManagementStyle,
}
```

- [ ] **步骤 2：编写 new() 构造函数和 load_data 方法**

```rust
impl PromptDebugger {
    pub fn new(cx: &mut Context<Self>, llm_store: LlmStore) -> Self {
        let crypto = llm_store.crypto().clone();
        let style = ManagementStyle::from_theme(cx.theme());
        let mut this = Self {
            llm_store,
            crypto,
            models: vec![],
            providers: vec![],
            selected_model_id: None,
            temperature: 0.7,
            max_tokens: 2048,
            top_p: 1.0,
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
}
```

- [ ] **步骤 3：编写消息增删改方法**

```rust
impl PromptDebugger {
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
}
```

- [ ] **步骤 4：编写单元测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;

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

        // Add
        let id = next_id;
        next_id += 1;
        msgs.push(DebugMessage { id, role: MessageRole::User, content: "hi".into() });
        assert_eq!(msgs.len(), 1);

        // Remove
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

        // Move id=2 up: [1,2,3] -> [2,1,3]
        if let Some(pos) = msgs.iter().position(|m| m.id == 2) {
            if pos > 0 { msgs.swap(pos, pos - 1); }
        }
        assert_eq!(msgs[0].id, 2);
        assert_eq!(msgs[1].id, 1);

        // Move id=2 down: [2,1,3] -> [1,2,3]
        if let Some(pos) = msgs.iter().position(|m| m.id == 2) {
            if pos + 1 < msgs.len() { msgs.swap(pos, pos + 1); }
        }
        assert_eq!(msgs[0].id, 1);
        assert_eq!(msgs[1].id, 2);
    }
}
```

- [ ] **步骤 5：运行测试验证通过**

运行：`cargo test -p hivegui prompt_debugger::tests -- --nocapture`
预期：4 tests PASS

- [ ] **步骤 6：Commit**

```bash
git add crates/hivegui/src/ui/prompt_debugger.rs
git commit -m "feat(hivegui): add PromptDebugger data model and state management"
```

---

### 任务 3：实现 LLM Settings 面板（左侧栏）

**文件：**
- 修改：`crates/hivegui/src/ui/prompt_debugger.rs`

- [ ] **步骤 1：编写模型下拉选择器函数**

在 `prompt_debugger.rs` 中添加：

```rust
fn model_selector(
    models: &[LlmModel],
    providers: &[LlmProvider],
    selected_id: Option<i64>,
    style: &ManagementStyle,
    entity: Entity<PromptDebugger>,
) -> impl IntoElement {
    let selected_label = selected_id
        .and_then(|id| models.iter().find(|m| m.id == id))
        .map(|m| {
            let provider_label = m.provider_id
                .and_then(|pid| providers.iter().find(|p| p.id == pid))
                .map(|p| p.category.clone())
                .unwrap_or_default();
            if provider_label.is_empty() {
                m.name.clone()
            } else {
                format!("{} ({})", m.name, provider_label)
            }
        })
        .unwrap_or_else(|| "请选择模型".to_string());

    let mut dropdown = div()
        .flex()
        .flex_col()
        .gap(px(2.0))
        .p(px(4.0))
        .bg(style.list.row)
        .border_1()
        .border_color(style.list.border)
        .rounded(px(4.0))
        .max_h(px(200.0))
        .shadow_lg();

    for model in models {
        let model_id = model.id;
        let model_entity = entity.clone();
        dropdown = dropdown.child(
            div()
                .px(px(8.0))
                .py(px(6.0))
                .rounded(px(4.0))
                .text_size(px(13.0))
                .cursor(CursorStyle::PointingHand)
                .hover(|s| s.bg(style.list.hover))
                .child(model.name.clone())
                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    _ = model_entity.update(cx, |view, _cx| {
                        view.selected_model_id = Some(model_id);
                        cx.notify();
                    });
                }),
        );
    }

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
                .id("model-selector-trigger")
                .px(px(8.0))
                .py(px(6.0))
                .bg(style.list.row)
                .border_1()
                .border_color(style.list.border)
                .rounded(px(4.0))
                .text_size(px(13.0))
                .cursor(CursorStyle::PointingHand)
                .child(selected_label)
                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    let dropdown = dropdown.clone();
                    let bounds = cx.bounds_of(&SharedString::from("model-selector-trigger"));
                    // 简化：直接在下拉触发器下方显示
                    cx.spawn(async move |cx| {
                        // 使用 popup 方式显示
                    })
                    .detach();
                }),
        )
}
```

**注意：** gpui-component 没有原生 Select 组件。参考 `llm_config.rs` 中 `relation_selector` 的模式，使用自定义 div 下拉。但为了简化首次实现，使用更简单的方式 — 直接在面板中渲染模型列表作为可点击项（类似 radio group），选中项高亮。

**修正方案 — 使用简化的模型选择器（radio group 风格）：**

```rust
fn model_selector(
    models: &[LlmModel],
    providers: &[LlmProvider],
    selected_id: Option<i64>,
    style: &ManagementStyle,
    entity: Entity<PromptDebugger>,
) -> impl IntoElement {
    let mut container = div()
        .flex()
        .flex_col()
        .gap(px(4.0))
        .child(
            div()
                .text_size(px(12.0))
                .text_color(style.list.muted_foreground)
                .child("模型"),
        );

    if models.is_empty() {
        container = container.child(
            div()
                .text_size(px(12.0))
                .text_color(style.list.muted_foreground)
                .child("请先在 AI 管理中添加模型"),
        );
    } else {
        for model in models {
            let model_id = model.id;
            let is_selected = selected_id == Some(model.id);
            let provider_label = model.provider_id
                .and_then(|pid| providers.iter().find(|p| p.id == pid))
                .map(|p| p.category.clone())
                .unwrap_or_default();
            let label = if provider_label.is_empty() {
                model.name.clone()
            } else {
                format!("{} ({})", model.name, provider_label)
            };
            let model_entity = entity.clone();
            let bg = if is_selected {
                style.list.active
            } else {
                style.list.row
            };
            container = container.child(
                div()
                    .id(SharedString::from(format!("model-option-{}", model.id)))
                    .px(px(8.0))
                    .py(px(6.0))
                    .bg(bg)
                    .border_1()
                    .border_color(if is_selected {
                        style.list.active_border
                    } else {
                        style.list.border
                    })
                    .rounded(px(4.0))
                    .text_size(px(13.0))
                    .cursor(CursorStyle::PointingHand)
                    .hover(|s| s.bg(style.list.hover))
                    .child(label)
                    .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                        _ = model_entity.update(cx, |view, _cx| {
                            view.selected_model_id = Some(model_id);
                            cx.notify();
                        });
                    }),
            );
        }
    }

    container
}
```

- [ ] **步骤 2：编写参数滑块组件**

由于 gpui-component 没有 Slider 组件，使用简化的数字输入 + 标签方式：

```rust
fn param_slider(
    label: &'static str,
    value: f64,
    min: f64,
    max: f64,
    step: f64,
    style: &ManagementStyle,
    on_change: impl Fn(f64) -> gpui::AnyElement + 'static,
) -> impl IntoElement {
    // 简化实现：显示标签 + 当前值 + 用 div 模拟进度条
    let pct = ((value - min) / (max - min)).clamp(0.0, 1.0);
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
                        .text_size(px(12.0))
                        .font_weight(FontWeight::BOLD)
                        .child(format!("{:.2}", value)),
                ),
        )
        .child(
            div()
                .w_full()
                .h(px(4.0))
                .bg(style.list.muted)
                .rounded(px(2.0))
                .child(
                    div()
                        .h_full()
                        .w(px(pct as f32 * 100.0))
                        .bg(style.main.background)
                        .rounded(px(2.0)),
                ),
        )
}
```

**注意：** 上面的 `on_change` 参数在简化版本中暂不使用（因为没有原生滑块交互）。后续迭代可加入鼠标拖拽交互。首次实现使用纯展示 + 数字输入框。

**修正方案 — 参数滑块使用数字输入：**

```rust
fn param_control(
    label: &'static str,
    value: &str,
    min_label: &str,
    max_label: &str,
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
```

- [ ] **步骤 3：编写 settings_panel 渲染函数**

```rust
fn settings_panel(
    models: &[LlmModel],
    providers: &[LlmProvider],
    selected_model_id: Option<i64>,
    temp_input: &Entity<InputState>,
    max_tokens_input: &Entity<InputState>,
    top_p_input: &Entity<InputState>,
    style: &ManagementStyle,
    entity: Entity<PromptDebugger>,
) -> impl IntoElement {
    div()
        .w(px(220.0))
        .flex_shrink_0()
        .h_full()
        .p(px(12.0))
        .border_r_1()
        .border_color(style.list.border)
        .overflow_y_scroll()
        .child(
            div()
                .text_size(px(14.0))
                .font_weight(FontWeight::BOLD)
                .mb(px(12.0))
                .child("LLM 设定"),
        )
        .child(
            model_selector(models, providers, selected_model_id, style, entity),
        )
        .child(div().mt(px(12.0)))
        .child(param_control("Temperature", "0.70", "精确", "创造", temp_input, style))
        .child(div().mt(px(12.0)))
        .child(param_control("Max Tokens", "2048", "1", "8192", max_tokens_input, style))
        .child(div().mt(px(12.0)))
        .child(param_control("Top P", "1.00", "0", "1", top_p_input, style))
}
```

- [ ] **步骤 4：运行编译检查**

运行：`cargo check -p hivegui`
预期：PASS（可能有未使用变量警告，可忽略）

- [ ] **步骤 5：Commit**

```bash
git add crates/hivegui/src/ui/prompt_debugger.rs
git commit -m "feat(hivegui): add LLM settings panel with model selector and params"
```

---

### 任务 4：实现 Message Editor（中间栏）

**文件：**
- 修改：`crates/hivegui/src/ui/prompt_debugger.rs`

- [ ] **步骤 1：编写 MessageCard 组件**

```rust
fn message_card(
    msg: &DebugMessage,
    style: &ManagementStyle,
    entity: Entity<PromptDebugger>,
) -> impl IntoElement {
    let msg_id = msg.id;
    let role = msg.role;
    let content = msg.content.clone();
    let role_color = match role {
        MessageRole::System => style.main.background,
        MessageRole::User => style.edit.background,
        MessageRole::Assistant => style.neutral.background,
    };

    div()
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
                                .text_size(px(11.0))
                                .cursor(CursorStyle::PointingHand)
                                .hover(|s| s.opacity(0.7))
                                .child("↑")
                                .on_mouse_down(MouseButton::Left, {
                                    let entity = entity.clone();
                                    move |_, _, cx| {
                                        _ = entity.update(cx, |view, _cx| {
                                            view.move_message_up(msg_id);
                                            cx.notify();
                                        });
                                    }
                                }),
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .cursor(CursorStyle::PointingHand)
                                .hover(|s| s.opacity(0.7))
                                .child("↓")
                                .on_mouse_down(MouseButton::Left, {
                                    let entity = entity.clone();
                                    move |_, _, cx| {
                                        _ = entity.update(cx, |view, _cx| {
                                            view.move_message_down(msg_id);
                                            cx.notify();
                                        });
                                    }
                                }),
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .cursor(CursorStyle::PointingHand)
                                .hover(|s| s.opacity(0.7))
                                .child("✕")
                                .on_mouse_down(MouseButton::Left, {
                                    let entity = entity.clone();
                                    move |_, _, cx| {
                                        _ = entity.update(cx, |view, _cx| {
                                            view.remove_message(msg_id);
                                            cx.notify();
                                        });
                                    }
                                }),
                        ),
                ),
        )
        .child(
            div()
                .px(px(10.0))
                .py(px(8.0))
                .min_h(px(60.0))
                .text_size(px(13.0))
                .child(content),
        )
}
```

- [ ] **步骤 2：编写 message_editor 渲染函数**

```rust
fn message_editor(
    messages: &[DebugMessage],
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
        .overflow_y_scroll()
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
                        .child(format!("({})", messages.len())),
                ),
        );

    for msg in messages {
        content = content.child(message_card(msg, style, entity.clone()));
    }

    // 添加消息按钮行
    content = content.child(
        div()
            .flex()
            .gap(px(6.0))
            .mt(px(8.0))
            .child(
                div()
                    .px(px(10.0))
                    .py(px(4.0))
                    .text_size(px(12.0))
                    .bg(style.main.background)
                    .rounded(px(4.0))
                    .cursor(CursorStyle::PointingHand)
                    .hover(|s| s.bg(style.main.hover))
                    .child("+ System")
                    .on_mouse_down(MouseButton::Left, {
                        let entity = entity.clone();
                        move |_, _, cx| {
                            _ = entity.update(cx, |view, _cx| {
                                view.add_message(MessageRole::System);
                                cx.notify();
                            });
                        }
                    }),
            )
            .child(
                div()
                    .px(px(10.0))
                    .py(px(4.0))
                    .text_size(px(12.0))
                    .bg(style.main.background)
                    .rounded(px(4.0))
                    .cursor(CursorStyle::PointingHand)
                    .hover(|s| s.bg(style.main.hover))
                    .child("+ User")
                    .on_mouse_down(MouseButton::Left, {
                        let entity = entity.clone();
                        move |_, _, cx| {
                            _ = entity.update(cx, |view, _cx| {
                                view.add_message(MessageRole::User);
                                cx.notify();
                            });
                        }
                    }),
            )
            .child(
                div()
                    .px(px(10.0))
                    .py(px(4.0))
                    .text_size(px(12.0))
                    .bg(style.main.background)
                    .rounded(px(4.0))
                    .cursor(CursorStyle::PointingHand)
                    .hover(|s| s.bg(style.main.hover))
                    .child("+ Assistant")
                    .on_mouse_down(MouseButton::Left, {
                        let entity = entity.clone();
                        move |_, _, cx| {
                            _ = entity.update(cx, |view, _cx| {
                                view.add_message(MessageRole::Assistant);
                                cx.notify();
                            });
                        }
                    }),
            ),
    );

    content
}
```

- [ ] **步骤 3：运行编译检查**

运行：`cargo check -p hivegui`
预期：PASS

- [ ] **步骤 4：Commit**

```bash
git add crates/hivegui/src/ui/prompt_debugger.rs
git commit -m "feat(hivegui): add message editor with add/remove/reorder"
```

---

### 任务 5：实现 Results Panel（右侧栏）和 API 调用

**文件：**
- 修改：`crates/hivegui/src/ui/prompt_debugger.rs`

- [ ] **步骤 1：编写 API 调用方法**

```rust
impl PromptDebugger {
    fn execute_call(&mut self, cx: &mut Context<Self>) {
        let model = match self.selected_model_id
            .and_then(|id| self.models.iter().find(|m| m.id == id))
        {
            Some(m) => m.clone(),
            None => {
                self.call_state = CallState::Error("请先选择一个模型".into());
                cx.notify();
                return;
            }
        };

        let provider = model.provider_id
            .and_then(|pid| self.providers.iter().find(|p| p.id == pid))
            .cloned();

        let base_url = provider.as_ref()
            .map(|p| p.base_url.clone())
            .unwrap_or_default();

        let token = provider.and_then(|p| {
            p.token_encrypted.and_then(|enc| {
                self.crypto.decrypt(&enc).ok()
            }).and_then(|bytes| {
                String::from_utf8(bytes).ok()
            })
        });

        if base_url.is_empty() {
            self.call_state = CallState::Error("所选模型未配置 Provider base_url".into());
            cx.notify();
            return;
        }

        // 构建消息体
        let messages: Vec<serde_json::Value> = self.messages.iter().map(|m| {
            serde_json::json!({
                "role": m.role.api_role(),
                "content": m.content,
            })
        }).collect();

        let body = serde_json::json!({
            "model": model.name,
            "messages": messages,
            "temperature": self.temperature,
            "max_tokens": self.max_tokens,
            "top_p": self.top_p,
        });

        self.call_state = CallState::Loading;
        cx.notify();

        let store_crypto = self.crypto.clone();
        cx.spawn(async move |this, cx| {
            let client = reqwest::Client::new();
            let url = if base_url.ends_with('/') {
                format!("{}v1/chat/completions", base_url)
            } else {
                format!("{}/v1/chat/completions", base_url)
            };

            let mut request = client.post(&url)
                .header("Content-Type", "application/json");

            if let Some(ref token) = token {
                request = request.header("Authorization", format!("Bearer {}", token));
            }

            let result = request
                .json(&body)
                .send()
                .await;

            let response_text = match result {
                Ok(resp) => {
                    let status = resp.status();
                    let text = resp.text().await.unwrap_or_default();
                    if status.is_success() {
                        // 解析响应提取 content
                        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                            let content = json
                                .get("choices")
                                .and_then(|c| c.as_array())
                                .and_then(|arr| arr.first())
                                .and_then(|first| first.get("message"))
                                .and_then(|msg| msg.get("content"))
                                .and_then(|c| c.as_str())
                                .unwrap_or(&text)
                                .to_string();
                            Ok(content)
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
```

- [ ] **步骤 2：编写 results_panel 渲染函数**

```rust
fn results_panel(
    call_state: &CallState,
    style: &ManagementStyle,
) -> impl IntoElement {
    div()
        .w(px(320.0))
        .flex_shrink_0()
        .h_full()
        .p(px(12.0))
        .border_l_1()
        .border_color(style.list.border)
        .overflow_y_scroll()
        .child(
            div()
                .text_size(px(14.0))
                .font_weight(FontWeight::BOLD)
                .mb(px(12.0))
                .child("Results"),
        )
        .child(match call_state {
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
                .whitespace_pre_wrap()
                .child(text.as_str())
                .into_any_element(),
            CallState::Error(err) => div()
                .text_size(px(13.0))
                .text_color(style.delete.foreground)
                .child(err.as_str())
                .into_any_element(),
        })
}
```

- [ ] **步骤 3：编写 execute_button 组件**

```rust
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
                    style.disabled.background
                } else {
                    style.main.background
                })
                .rounded(px(6.0))
                .text_size(px(13.0))
                .cursor(if is_loading {
                    CursorStyle::Default
                } else {
                    CursorStyle::PointingHand
                })
                .child(if is_loading { "执行中..." } else { "▷ 执行" })
                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    if !is_loading {
                        _ = entity.update(cx, |view, cx| {
                            view.execute_call(cx);
                        });
                    }
                }),
        )
}
```

- [ ] **步骤 4：运行编译检查**

运行：`cargo check -p hivegui`
预期：PASS

- [ ] **步骤 5：Commit**

```bash
git add crates/hivegui/src/ui/prompt_debugger.rs
git commit -m "feat(hivegui): add results panel and LLM API call execution"
```

---

### 任务 6：实现 Render trait 和三栏布局

**文件：**
- 修改：`crates/hivegui/src/ui/prompt_debugger.rs`

- [ ] **步骤 1：添加 InputState 字段到 PromptDebugger struct**

在 struct 定义中添加：

```rust
    temp_input: Option<Entity<InputState>>,
    max_tokens_input: Option<Entity<InputState>>,
    top_p_input: Option<Entity<InputState>>,
```

在 `new()` 中初始化：

```rust
    let temp_input = cx.new(|cx| InputState::new("0.70", cx));
    let max_tokens_input = cx.new(|cx| InputState::new("2048", cx));
    let top_p_input = cx.new(|cx| InputState::new("1.00", cx));
```

并保存到 struct 字段。

- [ ] **步骤 2：实现 Render trait**

```rust
impl Render for PromptDebugger {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let style = ManagementStyle::from_theme(cx.theme());
        self.style = style;

        let temp_input = self.temp_input.clone().unwrap();
        let max_tokens_input = self.max_tokens_input.clone().unwrap();
        let top_p_input = self.top_p_input.clone().unwrap();

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
                        &self.models,
                        &self.providers,
                        self.selected_model_id,
                        &temp_input,
                        &max_tokens_input,
                        &top_p_input,
                        &style,
                        cx.entity(),
                    ))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .child(
                                message_editor(
                                    &self.messages,
                                    &style,
                                    cx.entity(),
                                ),
                            )
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
                    .child(results_panel(
                        &self.call_state,
                        &style,
                    )),
            )
    }
}
```

- [ ] **步骤 3：添加 InputState 事件监听，同步参数值**

在 `new()` 中为每个 InputState 添加事件监听：

```rust
    let temp_input = cx.new(|cx| InputState::new("0.70", cx));
    let max_tokens_input = cx.new(|cx| InputState::new("2048", cx));
    let top_p_input = cx.new(|cx| InputState::new("1.00", cx));

    // 监听参数变化
    cx.subscribe(&temp_input, |this, _, _, cx| {
        // 从 input 读取值更新 temperature
    }).detach();
```

**简化方案：** 首次实现不实时同步 InputState → struct 字段，而是在 `execute_call` 时从 InputState 读取当前值。修改 `execute_call` 方法签名，接收 InputState 参数，或在 execute 时读取。

**最终方案：** 在 `execute_call` 中从 InputState 读取值：

```rust
fn execute_call(&mut self, cx: &mut Context<Self>) {
    // 从 InputState 读取最新值
    if let Some(ref input) = self.temp_input {
        if let Ok(v) = input.read(cx).text().parse::<f64>() {
            self.temperature = v;
        }
    }
    if let Some(ref input) = self.max_tokens_input {
        if let Ok(v) = input.read(cx).text().parse::<u32>() {
            self.max_tokens = v;
        }
    }
    if let Some(ref input) = self.top_p_input {
        if let Ok(v) = input.read(cx).text().parse::<f64>() {
            self.top_p = v;
        }
    }
    // ... 后续 API 调用逻辑
}
```

- [ ] **步骤 4：运行编译检查**

运行：`cargo check -p hivegui`
预期：PASS

- [ ] **步骤 5：Commit**

```bash
git add crates/hivegui/src/ui/prompt_debugger.rs
git commit -m "feat(hivegui): implement PromptDebugger render with three-column layout"
```

---

### 任务 7：集成到 UtilityView 和 App

**文件：**
- 修改：`crates/hivegui/src/ui/mod.rs`
- 修改：`crates/hivegui/src/ui/utility_view.rs`
- 修改：`crates/hivegui/src/ui/app.rs`

- [ ] **步骤 1：在 mod.rs 中导出新模块**

在 `crates/hivegui/src/ui/mod.rs` 中添加：

```rust
pub mod prompt_debugger;
```

- [ ] **步骤 2：修改 UtilityView 添加 PromptDebugger Tab**

修改 `crates/hivegui/src/ui/utility_view.rs`：

```rust
//! 工具视图 - 包含 LLM 提示词调试和数据源等桌面辅助工具。

use crate::datasource::Store;
use crate::datasource::llm_store::LlmStore;
use crate::ui::datasource_view::DataSourceView;
use crate::ui::prompt_debugger::PromptDebugger;
use gpui::*;
use gpui_component::ActiveTheme as _;
use gpui_component::tab::{Tab, TabBar};

pub struct UtilityView {
    active_tab: usize,
    prompt_debugger: Entity<PromptDebugger>,
    datasource_view: Entity<DataSourceView>,
}

impl UtilityView {
    pub fn new(cx: &mut Context<Self>, store: Entity<Store>, llm_store: LlmStore) -> Self {
        Self {
            active_tab: 0,
            prompt_debugger: cx.new(|cx| PromptDebugger::new(cx, llm_store)),
            datasource_view: cx.new(|cx| DataSourceView::new(store, cx)),
        }
    }
}

impl Render for UtilityView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(
                TabBar::new("utility-tabs")
                    .selected_index(self.active_tab)
                    .on_click(cx.listener(|this, index, _, cx| {
                        this.active_tab = *index;
                        cx.notify();
                    }))
                    .child(Tab::new().label("LLM 提示词调试"))
                    .child(Tab::new().label("数据源")),
            )
            .child(div().flex_1().min_h_0().child(match self.active_tab {
                0 => self.prompt_debugger.clone().into_any_element(),
                1 => self.datasource_view.clone().into_any_element(),
                _ => div().into_any_element(),
            }))
    }
}
```

- [ ] **步骤 3：修改 app.rs 中 RootView 的初始化**

修改 `crates/hivegui/src/ui/app.rs` 中 `RootView::new` 方法：

将：
```rust
let tools = cx.new(|cx| UtilityView::new(cx, store.clone()));
```

改为：
```rust
let tools = cx.new(|cx| UtilityView::new(cx, store.clone(), llm_store.clone()));
```

- [ ] **步骤 4：运行编译检查**

运行：`cargo check -p hivegui`
预期：PASS

- [ ] **步骤 5：运行全部测试**

运行：`cargo test -p hivegui`
预期：全部 PASS

- [ ] **步骤 6：Commit**

```bash
git add crates/hivegui/src/ui/mod.rs crates/hivegui/src/ui/utility_view.rs crates/hivegui/src/ui/app.rs
git commit -m "feat(hivegui): integrate PromptDebugger into UtilityView with tab navigation"
```

---

### 任务 8：几何测试

**文件：**
- 修改：`crates/hivegui/src/ui/prompt_debugger.rs`

- [ ] **步骤 1：添加几何测试**

在 `prompt_debugger.rs` 的 `#[cfg(test)]` 模块中添加：

```rust
#[cfg(test)]
mod geometry_tests {
    use gpui::*;
    use gpui_component::theme::Theme;

    use super::*;

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
```

- [ ] **步骤 2：运行几何测试**

运行：`cargo test -p hivegui prompt_debugger::geometry_tests -- --nocapture`
预期：PASS

- [ ] **步骤 3：Commit**

```bash
git add crates/hivegui/src/ui/prompt_debugger.rs
git commit -m "test(hivegui): add geometry tests for PromptDebugger three-column layout"
```
