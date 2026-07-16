---
name: "gpui-component"
description: "gpui-component 组件库完整使用指南。当在 hivegui 项目中使用 Input、Button、Icon、Scroll、Tab 等组件，或需要实现表单、表格、模态对话框、事件处理时调用此技能。"
---

# gpui-component 组件库完整使用指南

本技能为 hivegui 项目提供 gpui-component 组件库的完整使用参考，涵盖组件 API、表单、表格、模态对话框、事件处理等常见模式。

## 1. 可用图标列表 (IconName)

gpui-component 的图标由 `crates/assets/assets/icons/` 目录下的 SVG 文件通过宏 `icon_named!` 自动生成。
图标名称采用 **PascalCase** 形式，对应文件名使用 **kebab-case**。

### 完整可用图标

```
ALargeSmall, ArrowDown, ArrowLeft, ArrowRight, ArrowUp, Asterisk,
Battery, BatteryCharging, BatteryFull, BatteryLow, BatteryMedium, BatteryWarning,
Bell, BookOpen, Bot, Building2, Calendar, CaseSensitive, ChartPie, Check,
ChevronDown, ChevronLeft, ChevronRight, ChevronsUpDown, ChevronUp,
CircleCheck, CircleUser, CircleX, Close, Copy, Cpu, Dash, Delete,
Ellipsis, EllipsisVertical, ExternalLink, Eye, EyeOff,
File, Folder, FolderClosed, FolderOpen, Frame, GalleryVerticalEnd,
Github, Globe, HardDrive, Heart, HeartOff,
Inbox, Info, Inspector, LayoutDashboard,
Loader, LoaderCircle, Map, Maximize, MemoryStick, Menu, Minimize, Minus,
Moon, Network, Palette,
PanelBottom, PanelBottomOpen, PanelLeft, PanelLeftClose, PanelLeftOpen,
PanelRight, PanelRightClose, PanelRightOpen,
Pause, Play, Plus, Redo, Redo2, Replace, ResizeCorner,
Search, Settings, Settings2,
SortAscending, SortDescending, SquareTerminal,
Star, StarFill, StarOff, Sun,
ThumbsDown, ThumbsUp, TriangleAlert,
Undo, Undo2, User,
WindowClose, WindowMaximize, WindowMinimize, WindowRestore
```

### 图标命名转换规则

文件名 → IconName 的转换：
- `kebab-case` → `PascalCase`
- 数字用英文单词：`building-2` → `Building2`
- 连字符去除，每个单词首字母大写

### 使用示例

```rust
use gpui_component::{Icon, IconName};

// 基本用法
Icon::new(IconName::Settings).into_any_element()

// 在 nav_button 中使用
self.nav_button("home", Icon::new(IconName::LayoutDashboard).into_any_element(), AppRoute::Home, window, cx)
```

### 重要提示

- **不要猜测图标名称**：如果不确定图标是否存在，先查看上面的列表
- 没有 `Puzzle`、`Blocks`、`Sparkle`、`ZedSrcExtension`、`AiClaude` 等图标
- 常用替代：`Bot` (AI/机器人), `Settings` (设置), `HardDrive` (存储), `LayoutDashboard` (仪表盘)

## 2. 滚动容器 (ScrollableElement)

使用 `overflow_y_scrollbar()` 等方法需要导入 `ScrollableElement` trait：

```rust
use gpui_component::scroll::ScrollableElement;

// 然后才能在 div() 上使用滚动方法
div().flex_1().overflow_y_scrollbar()
    .child(content)
```

## 3. 常用组件导入

```rust
// 核心组件
use gpui_component::{Icon, IconName};
use gpui_component::button::{Button, ButtonStyle};
use gpui_component::input::{Input, InputState};
use gpui_component::scroll::ScrollableElement;

// 主题
use gpui_component::ActiveTheme;
```

## 4. 闭包捕获索引变量

在 `enumerate().map()` 中使用索引时，需要复制到局部变量避免类型不匹配：

```rust
tabs.iter().enumerate().map(|(i, label)| {
    let entity = cx.entity();
    let tab_index = i;  // 必须复制！不能直接在闭包中用 i
    div()
        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
            _ = entity.update(cx, |this, cx| {
                this.active_tab = tab_index;  // 使用复制的变量
                cx.notify();
            });
        })
})
```

## 5. Input 组件使用

### 5.1 InputState 创建与初始化

```rust
use gpui_component::input::{Input, InputState};

// 在视图结构体中添加字段
pub struct MyView {
    name_input: Option<Entity<InputState>>,
}

// 在 show_form 方法中初始化
fn show_add_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    self.name_input = Some(cx.new(|cx| 
        InputState::new(window, cx)
            .placeholder("输入名称")
            .default_value("")
    ));
    cx.notify();
}

fn show_edit_form(&mut self, window: &mut Window, item: Item, cx: &mut Context<Self>) {
    self.name_input = Some(cx.new(|cx| 
        InputState::new(window, cx)
            .placeholder("输入名称")
            .default_value(&item.name)
    ));
    cx.notify();
}
```

### 5.2 在 render 中使用 Input 组件

```rust
impl Render for MyView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 如果表单显示但输入框未初始化，则初始化
        if self.show_form && self.name_input.is_none() {
            self.name_input = Some(cx.new(|cx| 
                InputState::new(window, cx)
                    .placeholder("输入名称")
                    .default_value(&self.form_name)
            ));
        }

        div()
            // ... 其他内容
            .when(self.show_form, |this| {
                this.child(
                    div()
                        // 表单容器样式
                        .child(
                            div().child("名称:")
                        )
                        .child(
                            div()
                                .w(px(300.0))
                                .h(px(32.0))
                                .child(
                                    Input::new(&self.name_input.clone().unwrap())
                                )
                        )
                )
            })
    }
}
```

### 5.3 获取 Input 值

```rust
fn save(&mut self, cx: &mut Context<Self>) {
    // 同步输入框状态到表单字段
    if let Some(ref input) = self.name_input {
        self.form_name = input.read(cx).value().to_string();
    }
    
    // 验证和保存逻辑...
}
```

### 5.4 隐藏表单时清理

```rust
fn hide_form(&mut self, cx: &mut Context<Self>) {
    self.show_form = false;
    self.name_input = None;  // 清理 InputState
    cx.notify();
}
```

### 5.5 搜索输入框实现

搜索框必须使用 `Input` 组件，不能使用静态 `div`，否则无法点击和输入。

#### 步骤 1：添加字段和导入

```rust
use gpui_component::input::{Input, InputState, InputEvent};  // 必须导入 InputEvent

pub struct MyView {
    search_text: String,
    search_input: Option<Entity<InputState>>,  // 搜索输入框状态
    // ... 其他字段
}
```

#### 步骤 2：在 render 方法中初始化并订阅事件

```rust
impl Render for MyView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 初始化搜索输入框（只执行一次）
        if self.search_input.is_none() {
            self.search_input = Some(cx.new(|cx| 
                InputState::new(window, cx)
                    .placeholder("输入名称...")
                    .default_value(&self.search_text)
            ));
            
            // 订阅输入变化事件
            if let Some(ref input) = self.search_input {
                cx.subscribe_in(input, window, |this, state, event, window, cx| {
                    if let InputEvent::Change = event {
                        this.search_text = state.read(cx).value().to_string();
                        this.current_page = 0;  // 重置分页
                        this.load(cx);           // 重新加载数据
                    }
                }).detach();
            }
        }

        div()
            // ... 其他内容
            // 搜索栏
            .child(
                div()
                    .flex()
                    .items_center()
                    .p(px(12.0))
                    .border_b_1()
                    .border_color(rgb(0xe0e0e0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(div().text_size(px(13.0)).child("搜索:"))
                            .child(
                                div()
                                    .w(px(200.0))
                                    .child(Input::new(&self.search_input.clone().unwrap()))
                            )
                    )
            )
    }
}
```

#### 常见错误

| 错误 | 现象 | 正确做法 |
|------|------|----------|
| 使用静态 div 作为搜索框 | 点击无反应，无法输入 | 使用 `Input` 组件 + `InputState` |
| 未订阅 `InputEvent::Change` | 输入后不触发搜索 | 使用 `cx.subscribe_in()` 订阅事件 |
| 在 `new()` 中初始化 `InputState` | 编译错误，缺少 `window` 参数 | 在 `render()` 方法中初始化 |

## 6. 模态对话框与事件处理

### 6.1 表单对话框模式

表单对话框需要两层结构：遮罩层 + 内容层，作为兄弟元素渲染。

```rust
.when(self.show_form, |this| {
    this
    // 遮罩层 - 点击关闭表单
    .child(
        div()
            .absolute()
            .top(px(0.0))
            .left(px(0.0))
            .right(px(0.0))
            .bottom(px(0.0))
            .bg(rgb(0x000000))
            .opacity(0.3)
            .cursor(CursorStyle::PointingHand)
            .on_mouse_down(MouseButton::Left, {
                let this = cx.weak_entity();
                move |_, _, cx| {
                    this.update(cx, |view, cx| {
                        view.hide_form(cx);
                    })
                    .ok();
                }
            }),
    )
    // 内容层 - 表单容器
    .child(
        div()
            .absolute()
            .top(px(100.0))
            .left(px(50.0))
            .right(px(50.0))
            .max_w(px(500.0))
            .bg(rgb(0xffffff))
            .rounded(px(12.0))
            .shadow_lg()
            .border_1()
            .border_color(rgb(0xdddddd))
            .p(px(24.0))
            // 关键：阻止事件冒泡到遮罩层
            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                cx.stop_propagation();
            })
            .child(
                // 表单内容...
            )
    )
})
```

### 6.2 删除确认对话框模式

删除确认对话框同样需要两层结构：

```rust
.when(self.confirm_delete_id.is_some(), |this| {
    let id = self.confirm_delete_id.unwrap();
    this
    // 遮罩层 - 点击关闭对话框
    .child(
        div()
            .absolute()
            .top(px(0.0))
            .left(px(0.0))
            .right(px(0.0))
            .bottom(px(0.0))
            .bg(rgb(0x000000))
            .opacity(0.3)
            .on_mouse_down(MouseButton::Left, {
                let t = cx.weak_entity();
                move |_, _, cx| {
                    t.update(cx, |v, cx| {
                        v.confirm_delete_id = None;
                        cx.notify();
                    }).ok();
                }
            })
    )
    // 内容层 - 对话框
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
                    .bg(rgb(0xffffff))
                    .rounded(px(8.0))
                    .shadow_lg()
                    .border_1()
                    .border_color(rgb(0xdddddd))
                    .p(px(24.0))
                    // 关键：阻止事件冒泡
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .child(
                        // 对话框内容...
                    )
            )
    )
})
```

### 6.3 事件处理关键规则

1. **遮罩层**：点击时关闭对话框/表单
2. **内容层**：必须添加 `cx.stop_propagation()` 防止事件冒泡到遮罩层
3. **兄弟元素**：遮罩层和内容层必须是兄弟关系（使用 `.child().child()` 链式调用），不能嵌套

## 7. 表格渲染模式

### 7.1 基本表格结构

```rust
fn render_table(&self, cx: &mut Context<Self>) -> impl IntoElement {
    // 定义列宽
    let col_widths = [px(60.0), px(150.0), px(120.0), px(200.0), px(120.0)];

    div()
        .flex()
        .flex_col()
        .border_1()
        .border_color(rgb(0xe0e0e0))
        .rounded(px(4.0))
        .overflow_hidden()
        // 表头
        .child(
            div()
                .flex()
                .bg(rgb(0xf5f5f5))
                .border_b_1()
                .border_color(rgb(0xe0e0e0))
                .child(div().w(col_widths[0]).px(px(8.0)).py(px(8.0)).text_size(px(12.0)).font_weight(FontWeight::BOLD).text_color(rgb(0x333333)).child("ID"))
                .child(div().w(col_widths[1]).px(px(8.0)).py(px(8.0)).text_size(px(12.0)).font_weight(FontWeight::BOLD).text_color(rgb(0x333333)).child("名称"))
                .child(div().w(col_widths[2]).px(px(8.0)).py(px(8.0)).text_size(px(12.0)).font_weight(FontWeight::BOLD).text_color(rgb(0x333333)).child("Slug"))
                .child(div().w(col_widths[3]).px(px(8.0)).py(px(8.0)).text_size(px(12.0)).font_weight(FontWeight::BOLD).text_color(rgb(0x333333)).child("描述"))
                .child(div().w(col_widths[4]).px(px(8.0)).py(px(8.0)).text_size(px(12.0)).font_weight(FontWeight::BOLD).text_color(rgb(0x333333)).child("操作"))
        )
        // 数据行
        .children(
            self.items.iter().map(|item| {
                let id = item.id;
                let ic = item.clone();
                div()
                    .flex()
                    .border_b_1()
                    .border_color(rgb(0xf0f0f0))
                    .bg(rgb(0xffffff))
                    .child(div().w(col_widths[0]).px(px(8.0)).py(px(6.0)).text_size(px(12.0)).text_color(rgb(0x666666)).child(format!("{}", item.id)))
                    .child(div().w(col_widths[1]).px(px(8.0)).py(px(6.0)).text_size(px(13.0)).font_weight(FontWeight::MEDIUM).child(item.name.clone()))
                    .child(div().w(col_widths[2]).px(px(8.0)).py(px(6.0)).text_size(px(12.0)).text_color(rgb(0x666666)).child(item.slug.clone()))
                    .child(div().w(col_widths[3]).px(px(8.0)).py(px(6.0)).text_size(px(12.0)).text_color(rgb(0x666666)).truncate().child(item.description.clone().unwrap_or_default()))
                    .child(
                        div().w(col_widths[4]).px(px(8.0)).py(px(6.0)).flex().gap(px(4.0))
                            .child(
                                div().id(("edit", id as u64)).px(px(8.0)).py(px(3.0)).rounded(px(3.0)).bg(rgb(0x5cb85c)).text_color(rgb(0xffffff)).text_size(px(11.0)).cursor(CursorStyle::PointingHand).child("编辑")
                                    .on_mouse_down(MouseButton::Left, {
                                        let t = cx.weak_entity();
                                        move |_, window, cx| {
                                            t.update(cx, |v, cx| v.show_edit_form(window, ic.clone(), cx)).ok();
                                        }
                                    })
                            )
                            .child(
                                div().id(("del", id as u64)).px(px(8.0)).py(px(3.0)).rounded(px(3.0)).bg(rgb(0xd9534f)).text_color(rgb(0xffffff)).text_size(px(11.0)).cursor(CursorStyle::PointingHand).child("删除")
                                    .on_mouse_down(MouseButton::Left, {
                                        let t = cx.weak_entity();
                                        move |_, _, cx| {
                                            t.update(cx, |v, cx| {
                                                v.confirm_delete_id = Some(id);
                                                cx.notify();
                                            }).ok();
                                        }
                                    })
                            )
                    )
            })
        )
}
```

### 7.2 在 render 中使用表格

```rust
impl Render for MyView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(0xffffff))
            // 标题栏
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .p(px(16.0))
                    .border_b_1()
                    .border_color(rgb(0xe0e0e0))
                    .child(div().text_size(px(18.0)).font_weight(FontWeight::BOLD).child("管理页面"))
                    .child(
                        div()
                            .id("add-btn")
                            .px(px(12.0))
                            .py(px(6.0))
                            .rounded(px(4.0))
                            .bg(rgb(0x4a90d9))
                            .text_color(rgb(0xffffff))
                            .text_size(px(13.0))
                            .cursor(CursorStyle::PointingHand)
                            .child("+ 添加")
                            .on_mouse_down(MouseButton::Left, {
                                let t = cx.weak_entity();
                                move |_, window, cx| {
                                    t.update(cx, |v, cx| v.show_add_form(window, cx)).ok();
                                }
                            })
                    )
            )
            // 表格区域
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .overflow_y_scrollbar()
                    .p(px(16.0))
                    .child(
                        if self.items.is_empty() {
                            div()
                                .flex()
                                .items_center()
                                .justify_center()
                                .h_full()
                                .text_color(rgb(0x999999))
                                .text_size(px(14.0))
                                .child("暂无数据")
                                .into_any_element()
                        } else {
                            self.render_table(cx).into_any_element()
                        }
                    )
            )
            // 表单对话框
            .when(self.show_form, |this| { /* ... */ })
            // 删除确认对话框
            .when(self.confirm_delete_id.is_some(), |this| { /* ... */ })
    }
}
```

## 8. gpui-component 源码位置

gpui-component 是 Git 依赖，源码位于：
```
/home/developer/.cargo/git/checkouts/gpui-component-95ce574d8a0da8b8/49f4b4f/
```

关键目录：
- `crates/ui/src/` - 组件源码
- `crates/assets/assets/icons/` - 可用图标 SVG 文件
- `crates/ui/src/icon.rs` - Icon/IconName 定义
- `crates/ui/src/scroll/scrollable.rs` - ScrollableElement trait
- `crates/ui/src/element_ext.rs` - 通用 trait 定义
- `crates/ui/src/input/` - Input 组件源码
  - `input.rs` - Input 组件定义
  - `state.rs` - InputState 定义

## 9. 常见错误及解决

| 错误 | 原因 | 解决 |
|------|------|------|
| `no variant named Xxx found for IconName` | 图标名称不存在 | 查看上方图标列表 |
| `method overflow_y_scrollbar not found` | 缺少 ScrollableElement 导入 | `use gpui_component::scroll::ScrollableElement;` |
| `type mismatch in closure capture` | 闭包中直接捕获枚举索引 | 用 `let tab_index = i;` 复制 |
| 表单字段无法输入 | 使用了静态 div 而非 Input 组件 | 使用 InputState + Input 组件 |
| 点击表单时窗口被隐藏 | 事件冒泡到遮罩层 | 在表单容器添加 `cx.stop_propagation()` |
| 删除对话框被遮挡 | 遮罩层和对话框嵌套结构错误 | 使用兄弟元素结构，对话框在遮罩层之后渲染 |
| InputState 未初始化 | 表单显示时未创建 InputState | 在 show_form 方法和 render 中都检查并初始化 |

## 10. 完整视图文件模板

```rust
use gpui::*;
use gpui::prelude::FluentBuilder;
use gpui_component::scroll::ScrollableElement;
use gpui_component::input::{Input, InputState};
use crate::datasource::{Store, entity_store::MyEntity};

pub struct MyView {
    store: Entity<Store>,
    items: Vec<MyEntity>,
    loading: bool,
    show_form: bool,
    editing_item: Option<MyEntity>,
    form_name: String,
    error_message: Option<String>,
    confirm_delete_id: Option<i64>,
    name_input: Option<Entity<InputState>>,
}

impl MyView {
    pub fn new(store: Entity<Store>, cx: &mut Context<Self>) -> Self {
        let mut view = Self {
            store,
            items: Vec::new(),
            loading: false,
            show_form: false,
            editing_item: None,
            form_name: String::new(),
            error_message: None,
            confirm_delete_id: None,
            name_input: None,
        };
        view.load(cx);
        view
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        let store = self.store.read(cx).clone();
        cx.spawn(async move |this, cx| {
            match MyEntity::list(store.pool()).await {
                Ok(items) => {
                    this.update(cx, |v, cx| {
                        v.items = items;
                        v.loading = false;
                        cx.notify();
                    }).ok();
                }
                Err(e) => {
                    this.update(cx, |v, cx| {
                        v.error_message = Some(format!("加载失败: {}", e));
                        v.loading = false;
                        cx.notify();
                    }).ok();
                }
            }
        }).detach();
    }

    fn show_add_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_form = true;
        self.editing_item = None;
        self.form_name = String::new();
        self.error_message = None;
        self.name_input = Some(cx.new(|cx| 
            InputState::new(window, cx)
                .placeholder("输入名称")
                .default_value("")
        ));
        cx.notify();
    }

    fn show_edit_form(&mut self, window: &mut Window, item: MyEntity, cx: &mut Context<Self>) {
        self.show_form = true;
        self.editing_item = Some(item.clone());
        self.form_name = item.name.clone();
        self.error_message = None;
        self.name_input = Some(cx.new(|cx| 
            InputState::new(window, cx)
                .placeholder("输入名称")
                .default_value(&item.name)
        ));
        cx.notify();
    }

    fn hide_form(&mut self, cx: &mut Context<Self>) {
        self.show_form = false;
        self.editing_item = None;
        self.form_name.clear();
        self.error_message = None;
        self.name_input = None;
        cx.notify();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        if let Some(ref input) = self.name_input {
            self.form_name = input.read(cx).value().to_string();
        }
        
        if self.form_name.trim().is_empty() {
            self.error_message = Some("名称不能为空".into());
            cx.notify();
            return;
        }

        let store = self.store.read(cx).clone();
        let name = self.form_name.clone();

        if let Some(editing) = &self.editing_item {
            let id = editing.id;
            cx.spawn(async move |this, cx| {
                match MyEntity::update(store.pool(), id, name).await {
                    Ok(_) => {
                        this.update(cx, |v, cx| {
                            v.hide_form(cx);
                            v.load(cx);
                        }).ok();
                    }
                    Err(e) => {
                        this.update(cx, |v, cx| {
                            v.error_message = Some(format!("更新失败: {}", e));
                            cx.notify();
                        }).ok();
                    }
                }
            }).detach();
        } else {
            cx.spawn(async move |this, cx| {
                match MyEntity::create(store.pool(), name).await {
                    Ok(_) => {
                        this.update(cx, |v, cx| {
                            v.hide_form(cx);
                            v.load(cx);
                        }).ok();
                    }
                    Err(e) => {
                        this.update(cx, |v, cx| {
                            v.error_message = Some(format!("创建失败: {}", e));
                            cx.notify();
                        }).ok();
                    }
                }
            }).detach();
        }
    }

    fn delete(&mut self, id: i64, cx: &mut Context<Self>) {
        let store = self.store.read(cx).clone();
        cx.spawn(async move |this, cx| {
            match MyEntity::delete(store.pool(), id).await {
                Ok(_) => {
                    this.update(cx, |v, cx| {
                        v.load(cx);
                    }).ok();
                }
                Err(e) => {
                    this.update(cx, |v, cx| {
                        v.error_message = Some(format!("删除失败: {}", e));
                        cx.notify();
                    }).ok();
                }
            }
        }).detach();
    }
}

impl Render for MyView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 初始化 InputState
        if self.show_form && self.name_input.is_none() {
            self.name_input = Some(cx.new(|cx| 
                InputState::new(window, cx)
                    .placeholder("输入名称")
                    .default_value(&self.form_name)
            ));
        }

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(0xffffff))
            // 标题栏
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .p(px(16.0))
                    .border_b_1()
                    .border_color(rgb(0xe0e0e0))
                    .child(div().text_size(px(18.0)).font_weight(FontWeight::BOLD).child("管理"))
                    .child(
                        div()
                            .id("add-btn")
                            .px(px(12.0))
                            .py(px(6.0))
                            .rounded(px(4.0))
                            .bg(rgb(0x4a90d9))
                            .text_color(rgb(0xffffff))
                            .text_size(px(13.0))
                            .cursor(CursorStyle::PointingHand)
                            .child("+ 添加")
                            .on_mouse_down(MouseButton::Left, {
                                let t = cx.weak_entity();
                                move |_, window, cx| {
                                    t.update(cx, |v, cx| v.show_add_form(window, cx)).ok();
                                }
                            })
                    )
            )
            // 表格
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .overflow_y_scrollbar()
                    .p(px(16.0))
                    .child(
                        if self.items.is_empty() {
                            div()
                                .flex()
                                .items_center()
                                .justify_center()
                                .h_full()
                                .text_color(rgb(0x999999))
                                .text_size(px(14.0))
                                .child("暂无数据")
                                .into_any_element()
                        } else {
                            self.render_table(cx).into_any_element()
                        }
                    )
            )
            // 表单对话框
            .when(self.show_form, |this| {
                this
                .child(
                    div()
                        .absolute()
                        .top(px(0.0))
                        .left(px(0.0))
                        .right(px(0.0))
                        .bottom(px(0.0))
                        .bg(rgb(0x000000))
                        .opacity(0.3)
                        .cursor(CursorStyle::PointingHand)
                        .on_mouse_down(MouseButton::Left, {
                            let this = cx.weak_entity();
                            move |_, _, cx| {
                                this.update(cx, |view, cx| {
                                    view.hide_form(cx);
                                })
                                .ok();
                            }
                        }),
                )
                .child(
                    div()
                        .absolute()
                        .top(px(100.0))
                        .left(px(50.0))
                        .right(px(50.0))
                        .max_w(px(500.0))
                        .bg(rgb(0xffffff))
                        .rounded(px(12.0))
                        .shadow_lg()
                        .border_1()
                        .border_color(rgb(0xdddddd))
                        .p(px(24.0))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                            cx.stop_propagation();
                        })
                        .child(
                            div()
                                .text_size(px(16.0))
                                .font_weight(FontWeight::BOLD)
                                .margin_bottom(px(16.0))
                                .child(if self.editing_item.is_some() { "编辑" } else { "添加" })
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .margin_bottom(px(12.0))
                                .child(div().w(px(80.0)).text_size(px(13.0)).child("名称:"))
                                .child(
                                    div()
                                        .flex_1()
                                        .h(px(32.0))
                                        .child(Input::new(&self.name_input.clone().unwrap()))
                                )
                        )
                        .when_some(&self.error_message, |this, msg| {
                            this.child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(rgb(0xd9534f))
                                    .margin_bottom(px(12.0))
                                    .child(msg.clone())
                            )
                        })
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap(px(8.0))
                                .child(
                                    div()
                                        .px(px(12.0))
                                        .py(px(6.0))
                                        .rounded(px(4.0))
                                        .bg(rgb(0x999999))
                                        .text_color(rgb(0xffffff))
                                        .text_size(px(13.0))
                                        .cursor(CursorStyle::PointingHand)
                                        .child("取消")
                                        .on_mouse_down(MouseButton::Left, {
                                            let this = cx.weak_entity();
                                            move |_, _, cx| {
                                                this.update(cx, |view, cx| {
                                                    view.hide_form(cx);
                                                })
                                                .ok();
                                            }
                                        })
                                )
                                .child(
                                    div()
                                        .px(px(12.0))
                                        .py(px(6.0))
                                        .rounded(px(4.0))
                                        .bg(rgb(0x4a90d9))
                                        .text_color(rgb(0xffffff))
                                        .text_size(px(13.0))
                                        .cursor(CursorStyle::PointingHand)
                                        .child("保存")
                                        .on_mouse_down(MouseButton::Left, {
                                            let this = cx.weak_entity();
                                            move |_, _, cx| {
                                                this.update(cx, |view, cx| {
                                                    view.save(cx);
                                                })
                                                .ok();
                                            }
                                        })
                                )
                        )
                )
            })
            // 删除确认对话框
            .when(self.confirm_delete_id.is_some(), |this| {
                let id = self.confirm_delete_id.unwrap();
                this
                .child(
                    div()
                        .absolute()
                        .top(px(0.0))
                        .left(px(0.0))
                        .right(px(0.0))
                        .bottom(px(0.0))
                        .bg(rgb(0x000000))
                        .opacity(0.3)
                        .on_mouse_down(MouseButton::Left, {
                            let t = cx.weak_entity();
                            move |_, _, cx| {
                                t.update(cx, |v, cx| {
                                    v.confirm_delete_id = None;
                                    cx.notify();
                                }).ok();
                            }
                        })
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
                                .bg(rgb(0xffffff))
                                .rounded(px(8.0))
                                .shadow_lg()
                                .border_1()
                                .border_color(rgb(0xdddddd))
                                .p(px(24.0))
                                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                    cx.stop_propagation();
                                })
                                .child(
                                    div()
                                        .text_size(px(16.0))
                                        .font_weight(FontWeight::BOLD)
                                        .margin_bottom(px(12.0))
                                        .child("确认删除")
                                )
                                .child(
                                    div()
                                        .text_size(px(13.0))
                                        .text_color(rgb(0x666666))
                                        .margin_bottom(px(20.0))
                                        .child("确定要删除这个项目吗？此操作不可恢复。")
                                )
                                .child(
                                    div()
                                        .flex()
                                        .justify_end()
                                        .gap(px(8.0))
                                        .child(
                                            div()
                                                .px(px(12.0))
                                                .py(px(6.0))
                                                .rounded(px(4.0))
                                                .bg(rgb(0x999999))
                                                .text_color(rgb(0xffffff))
                                                .text_size(px(13.0))
                                                .cursor(CursorStyle::PointingHand)
                                                .child("取消")
                                                .on_mouse_down(MouseButton::Left, {
                                                    let t = cx.weak_entity();
                                                    move |_, _, cx| {
                                                        t.update(cx, |v, cx| {
                                                            v.confirm_delete_id = None;
                                                            cx.notify();
                                                        }).ok();
                                                    }
                                                })
                                        )
                                        .child(
                                            div()
                                                .px(px(12.0))
                                                .py(px(6.0))
                                                .rounded(px(4.0))
                                                .bg(rgb(0xd9534f))
                                                .text_color(rgb(0xffffff))
                                                .text_size(px(13.0))
                                                .cursor(CursorStyle::PointingHand)
                                                .child("确认删除")
                                                .on_mouse_down(MouseButton::Left, {
                                                    let t = cx.weak_entity();
                                                    move |_, _, cx| {
                                                        t.update(cx, |v, cx| {
                                                            v.delete(id, cx);
                                                            v.confirm_delete_id = None;
                                                            cx.notify();
                                                        }).ok();
                                                    }
                                                })
                                        )
                                )
                        )
                )
            })
    }
}
```
