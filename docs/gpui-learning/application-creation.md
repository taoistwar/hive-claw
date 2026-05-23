# GPUI 学习文档：创建应用实例

## 目录
- [1. 概述](#1-概述)
- [2. 环境准备](#2-环境准备)
- [3. 核心概念](#3-核心概念)
- [4. 学习路径](#4-学习路径)
- [5. 实践案例](#5-实践案例)
- [6. API 参考](#6-api-参考)
- [7. 常见问题](#7-常见问题)

---

## 1. 概述

`gpui_platform::application()` 是 GPUI 框架的入口函数，负责创建平台相关的应用实例。GPUI 是 Zed 编辑器使用的 UI 框架，未发布到 crates.io，需要从 Zed 仓库拉取。

### 架构关系

```
gpui_platform::application()
        │
        ├── Linux: 自动检测 Wayland 或 X11
        ├── macOS: 使用 Cocoa/AppKit
        └── Web:   使用 WebAssembly
        │
        ▼
    Application
        │
        └── run(|cx: &mut App| { ... })
                │
                ├── 设置全局状态
                ├── 注册快捷键
                ├── 打开窗口
                └── 进入事件循环
```

---

## 2. 环境准备

### 2.1 依赖配置

`Cargo.toml` 配置：

```toml
[dependencies]
gpui = { git = "https://github.com/zed-industries/zed.git", branch = "main" }
gpui_platform = { git = "https://github.com/zed-industries/zed.git", branch = "main" }
```

### 2.2 Linux 平台后端配置

GPUI 在 Linux 上需要显式启用 Wayland 或 X11 后端：

```toml
gpui_platform = { git = "https://github.com/zed-industries/zed.git", branch = "main", features = ["wayland", "x11"] }
```

**为什么需要显式启用？**

GPUI 的 `gpui_linux` 模块使用条件编译。不启用 feature 时，对应的平台代码会被 `cfg` 掉，运行时会触发 `unreachable!()` panic。

### 2.3 系统依赖

Linux 上需要安装以下系统库：

```bash
# Ubuntu/Debian
sudo apt-get install -y \
    libxkbcommon-dev \
    libwayland-dev \
    libvulkan-dev \
    libx11-dev \
    libxrandr-dev \
    libxcursor-dev \
    libssl-dev \
    pkg-config

# Fedora
sudo dnf install -y \
    libxkbcommon-devel \
    wayland-devel \
    vulkan-loader-devel \
    libX11-devel \
    libXrandr-devel \
    libXcursor-devel \
    openssl-devel \
    pkg-config
```

---

## 3. 核心概念

### 3.1 Application

`Application` 是 GPUI 的顶层对象，代表整个 GUI 应用程序。它管理：

- 所有窗口
- 事件循环
- 全局状态
- 键盘绑定

### 3.2 App Context

`&mut App`（简称 `cx`）是应用级别的上下文，提供：

| 方法 | 功能 |
|------|------|
| `cx.new()` | 创建 Entity |
| `cx.set_global()` | 设置全局状态 |
| `cx.global()` | 读取全局状态 |
| `cx.update_global()` | 修改全局状态 |
| `cx.open_window()` | 打开新窗口 |
| `cx.bind_keys()` | 注册全局快捷键 |
| `cx.quit()` | 退出应用 |
| `cx.activate()` | 激活应用窗口 |
| `cx.spawn()` | 启动异步任务 |

### 3.3 平台自动检测

`gpui_platform::application()` 内部逻辑：

```rust
pub fn application() -> Application {
    match current_platform() {
        Platform::MacOS => build_macos_app(),
        Platform::Linux => {
            match guess_compositor() {
                Compositor::Wayland => build_wayland_app(),
                Compositor::X11 => build_x11_app(),
            }
        }
        Platform::Web => build_web_app(),
    }
}
```

**关键点**：调用方不需要写 `#[cfg]` 条件编译，一套代码跨平台运行。

---

## 4. 学习路径

按照以下顺序逐步学习：

```
Step 1: 最简窗口      →  理解 application() + run() 的基本用法
    ↓
Step 2: 窗口配置      →  理解 WindowOptions, Bounds, Decorations
    ↓
Step 3: 全局状态      →  理解 Global trait, set_global, global
    ↓
Step 4: 键盘绑定      →  理解 KeyBinding, actions! 宏
    ↓
Step 5: 完整应用      →  综合以上所有内容
```

---

## 5. 实践案例

### 案例 1：最简 GPUI 应用

这是最小的可运行 GPUI 应用，打开一个空白窗口。

```rust
use gpui::{prelude::*, App, Context, Render, Window, WindowBounds, WindowOptions};
use gpui_platform;

fn main() {
    // 1. 创建应用实例（自动选择平台后端）
    let app = gpui_platform::application();

    // 2. 进入事件循环
    app.run(|cx: &mut App| {
        // 3. 打开窗口
        cx.open_window(WindowOptions::default(), |_, cx| {
            cx.new(|_| EmptyView)
        })
        .expect("window should open");

        // 4. 激活窗口
        cx.activate(true);
    });
}

// 空白视图
struct EmptyView;

impl Render for EmptyView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        gpui::div()
            .size_full()
            .child("Hello, GPUI!")
    }
}
```

**运行方式**：
```bash
cargo run
```

**关键点**：
- `gpui_platform::application()` 必须在主线程调用
- `app.run()` 会阻塞直到应用退出
- `cx.new()` 创建 Entity，参数是闭包 `|cx| View`
- `cx.open_window()` 的闭包参数是 `|window, cx|`，返回一个 Entity

---

### 案例 2：配置窗口大小和位置

```rust
use gpui::{prelude::*, px, size, App, Bounds, Context, Render, Window, WindowBounds, WindowDecorations, WindowOptions};
use gpui_platform;

fn main() {
    let app = gpui_platform::application();

    app.run(|cx: &mut App| {
        // 创建窗口边界：居中，1200x700
        let bounds = Bounds::centered(None, size(px(1200.0), px(700.0)), cx);

        cx.open_window(
            WindowOptions {
                // 窗口大小和位置
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                // 使用系统窗口装饰（标题栏、关闭按钮等）
                window_decorations: Some(WindowDecorations::Server),
                // 其他选项使用默认值
                ..Default::default()
            },
            |_, cx| cx.new(|_| ConfiguredView),
        )
        .expect("window should open");

        cx.activate(true);
    });
}

struct ConfiguredView;

impl Render for ConfiguredView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        gpui::div()
            .size_full()
            .child("Window: 1200 x 700, Centered")
    }
}
```

**API 详解**：

| API | 说明 |
|-----|------|
| `Bounds::centered(parent, size, cx)` | 创建居中的边界，`parent=None` 表示屏幕中心 |
| `size(w, h)` | 创建 Size 类型 |
| `px(value)` | 创建像素值，类型是 `Pixels` |
| `WindowBounds::Windowed(bounds)` | 窗口模式，指定大小和位置 |
| `WindowBounds::Maximized` | 最大化模式 |
| `WindowBounds::Fullscreen` | 全屏模式 |
| `WindowDecorations::Server` | 使用系统提供的窗口装饰 |
| `WindowDecorations::Client` | 自定义客户端装饰（无边框窗口） |

---

### 案例 3：使用全局状态

全局状态是跨视图共享的单例数据，适合放配置、路由、HTTP 客户端等。

```rust
use gpui::{prelude::*, px, size, App, Bounds, Context, Entity, Global, Render, Window, WindowBounds, WindowOptions};
use gpui_platform;
use std::sync::Arc;

// 1. 定义全局状态结构体
struct AppState {
    title: String,
    counter: i32,
    config: Arc<Config>,
}

// 2. 实现 Global trait（空实现即可）
impl Global for AppState {}

struct Config {
    debug: bool,
}

fn main() {
    let app = gpui_platform::application();

    app.run(|cx: &mut App| {
        // 3. 创建视图
        let view = cx.new(|cx| CounterView {
            count: 0,
        });

        // 4. 设置全局状态
        cx.set_global(AppState {
            title: "Counter App".to_string(),
            counter: 0,
            config: Arc::new(Config { debug: true }),
        });

        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(
                    Bounds::centered(None, size(px(400.0), px(300.0)), cx),
                )),
                ..Default::default()
            },
            |_, _| view,
        )
        .expect("window should open");

        cx.activate(true);
    });
}

struct CounterView {
    count: i32,
}

impl Render for CounterView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 5. 读取全局状态
        let state = cx.global::<AppState>();

        gpui::div()
            .size_full()
            .child(format!("Title: {}", state.title))
            .child(format!("Count: {}", self.count))
    }
}
```

**Global 使用模式**：

```rust
// 设置
cx.set_global(MyState { ... });

// 读取（不可变）
let state = cx.global::<MyState>();

// 修改
cx.update_global::<MyState, _>(|state, cx| {
    state.counter += 1;
});

// 检查是否存在
if cx.has_global::<MyState>() { ... }

// 移除
cx.remove_global::<MyState>();
```

---

### 案例 4：注册键盘快捷键

```rust
use gpui::{prelude::*, px, size, App, Bounds, Context, Entity, Global, KeyBinding, Render, Window, WindowBounds, WindowOptions};
use gpui_platform;

// 1. 用 actions! 宏定义动作
gpui::actions!(actions, [Increment, Decrement, Quit]);

// 2. 实现 Global
struct AppState {
    count: i32,
}
impl Global for AppState {}

fn main() {
    let app = gpui_platform::application();

    app.run(|cx: &mut App| {
        // 3. 注册全局快捷键
        cx.bind_keys([
            KeyBinding::new("cmd-n", Increment, None),    // macOS: Cmd+N
            KeyBinding::new("ctrl-n", Increment, None),   // Linux/Windows: Ctrl+N
            KeyBinding::new("cmd-p", Decrement, None),    // macOS: Cmd+P
            KeyBinding::new("ctrl-p", Decrement, None),   // Linux/Windows: Ctrl+P
            KeyBinding::new("cmd-q", Quit, None),         // macOS: Cmd+Q
            KeyBinding::new("ctrl-q", Quit, None),        // Linux/Windows: Ctrl+Q
        ]);

        let view = cx.new(|_| CounterView);

        cx.set_global(AppState { count: 0 });

        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(
                    Bounds::centered(None, size(px(400.0), px(300.0)), cx),
                )),
                ..Default::default()
            },
            |_, _| view,
        )
        .expect("window should open");

        cx.activate(true);
    });
}

struct CounterView;

impl CounterView {
    fn increment(&mut self, _: &Increment, cx: &mut Context<Self>) {
        cx.update_global::<AppState, _>(|state, _| {
            state.count += 1;
        });
    }

    fn decrement(&mut self, _: &Decrement, cx: &mut Context<Self>) {
        cx.update_global::<AppState, _>(|state, _| {
            state.count -= 1;
        });
    }
}

impl Render for CounterView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let count = cx.global::<AppState>().count;

        gpui::div()
            .size_full()
            .on_action(cx.listener(Self::increment))
            .on_action(cx.listener(Self::decrement))
            .child(format!("Count: {}", count))
            .child("Press Cmd/Ctrl + N to increment")
            .child("Press Cmd/Ctrl + P to decrement")
    }
}
```

**KeyBinding 详解**：

| 参数 | 说明 |
|------|------|
| 第一个参数 | 按键字符串，如 `"cmd-a"`, `"ctrl-shift-x"` |
| 第二个参数 | 动作类型（actions! 定义的） |
| 第三个参数 | 上下文（`None` 表示全局，`Some("TextInput")` 表示只在 TextInput 上下文生效） |

**按键语法**：
- `cmd` / `ctrl` / `shift` / `alt` / `super` — 修饰键
- 用 `-` 连接，如 `ctrl-shift-a`
- 特殊键：`enter`, `escape`, `tab`, `space`, `backspace`, `delete`, `left`, `right`, `up`, `down`, `home`, `end`

---

### 案例 5：完整应用（模拟 HiveGUI 启动流程）

这是一个精简版的 HiveGUI 启动流程，综合了以上所有概念。

```rust
use gpui::{
    prelude::*, px, rgb, size, App, Bounds, Context, CursorStyle, Entity, Global,
    MouseButton, SharedString, Window, WindowBounds, WindowDecorations, WindowOptions,
};
use gpui_platform;
use std::sync::Arc;

// ─── 全局状态 ───

struct AppConfig {
    title: String,
    debug: bool,
}

struct AppRoute {
    current_page: String,
}

impl Global for AppConfig {}
impl Global for AppRoute {}

// ─── 主函数 ───

fn main() {
    // 1. 创建应用实例
    let app = gpui_platform::application();

    // 2. 准备配置（在 run 之外，可以包含同步初始化逻辑）
    let config = AppConfig {
        title: "My App".to_string(),
        debug: false,
    };

    // 3. 进入事件循环
    app.run(move |cx: &mut App| {
        // 4. 设置全局状态
        cx.set_global(config);
        cx.set_global(AppRoute {
            current_page: "Home".to_string(),
        });

        // 5. 配置窗口
        let bounds = Bounds::centered(None, size(px(1000.0), px(600.0)), cx);

        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_decorations: Some(WindowDecorations::Server),
                ..Default::default()
            },
            |_, cx| cx.new(RootView::new),
        )
        .expect("window should open");

        cx.activate(true);
    });
}

// ─── 根视图 ───

struct RootView {
    title_bar: Entity<TitleBar>,
    content: Entity<ContentView>,
    status_bar: Entity<StatusBar>,
}

impl RootView {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            title_bar: cx.new(|_| TitleBar),
            content: cx.new(|_| ContentView),
            status_bar: cx.new(|_| StatusBar),
        }
    }
}

impl Render for RootView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let title = cx.global::<AppConfig>().title.clone();

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(0xf7f7f7))
            .child(
                // 自定义标题栏
                div()
                    .id("titlebar")
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .h(px(32.0))
                    .px(px(8.0))
                    .bg(rgb(0xe8e8e8))
                    .cursor(CursorStyle::OpenHand)
                    .on_mouse_down(MouseButton::Left, |event, window, _cx| {
                        if event.click_count == 2 {
                            window.zoom_window();
                        } else {
                            window.start_window_move();
                        }
                    })
                    .child(div().child(title))
                    .child(
                        div()
                            .flex()
                            .gap(px(4.0))
                            .child(window_button("─", |window, _, _| {
                                window.minimize_window();
                            }))
                            .child(window_button("□", |window, _, _| {
                                window.zoom_window();
                            }))
                            .child(window_button("✕", |_, _, cx| {
                                cx.quit();
                            })),
                    ),
            )
            .child(div().flex_1().child(self.content.clone()))
            .child(div().h(px(24.0)).child(self.status_bar.clone()))
    }
}

// ─── 标题栏 ───

struct TitleBar;

impl Render for TitleBar {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

// ─── 内容区 ───

struct ContentView;

impl Render for ContentView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let page = &cx.global::<AppRoute>().current_page;

        div()
            .size_full()
            .items_center()
            .justify_center()
            .child(format!("Current Page: {}", page))
    }
}

// ─── 状态栏 ───

struct StatusBar;

impl Render for StatusBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let page = &cx.global::<AppRoute>().current_page;

        div()
            .h_full()
            .flex()
            .items_center()
            .px(px(12.0))
            .bg(rgb(0xe8e8e8))
            .child(format!("Page: {}", page))
    }
}

// ─── 工具函数 ───

fn window_button(
    label: &str,
    on_click: impl Fn(&mut Window, &gpui::MouseDownEvent, &mut gpui::App) + 'static,
) -> impl IntoElement {
    let label = SharedString::from(label);

    div()
        .id(label.clone())
        .w(px(30.0))
        .h(px(24.0))
        .flex()
        .items_center()
        .justify_center()
        .cursor(CursorStyle::PointingHand)
        .child(label)
        .on_mouse_down(MouseButton::Left, move |event, window, cx| {
            on_click(window, event, cx);
        })
}
```

---

### 案例 6：多窗口应用

```rust
use gpui::{prelude::*, px, size, App, Bounds, Context, Entity, Render, Window, WindowOptions};
use gpui_platform;

fn main() {
    let app = gpui_platform::application();

    app.run(|cx: &mut App| {
        // 主窗口
        cx.open_window(WindowOptions::default(), |_, cx| {
            cx.new(|_| MainView { counter: 0 })
        })
        .expect("main window should open");

        // 辅助窗口
        cx.open_window(WindowOptions::default(), |_, cx| {
            cx.new(|_| AuxView)
        })
        .expect("aux window should open");

        cx.activate(true);
    });
}

struct MainView {
    counter: i32,
}

impl MainView {
    fn open_new_window(&mut self, cx: &mut Context<Self>) {
        cx.open_window(WindowOptions::default(), |_, cx| {
            cx.new(|_| AuxView)
        })
        .expect("new window should open");
    }
}

impl Render for MainView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .items_center()
            .justify_center()
            .flex_col()
            .gap(px(16.0))
            .child(format!("Main Window - Counter: {}", self.counter))
            .child(
                div()
                    .id("open-window-btn")
                    .px(px(16.0))
                    .py(px(8.0))
                    .cursor(CursorStyle::PointingHand)
                    .on_mouse_down(MouseButton::Left, {
                        let cx = cx.weak_entity();
                        move |_, _, _| {
                            cx.update(|_, cx| {
                                cx.open_window(WindowOptions::default(), |_, cx| {
                                    cx.new(|_| AuxView)
                                })
                                .expect("new window should open");
                            })
                            .ok();
                        }
                    })
                    .child("Open New Window"),
            )
    }
}

struct AuxView;

impl Render for AuxView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child("Auxiliary Window")
    }
}
```

**多窗口要点**：
- `cx.open_window()` 可以在 `app.run()` 内多次调用
- 也可以在视图内通过 `cx.open_window()` 动态打开新窗口
- 每个窗口有自己的 `Window` 上下文

---

### 案例 7：与 Tokio 异步运行时集成

```rust
use gpui::{prelude::*, px, size, App, Bounds, Context, Entity, Global, Render, Window, WindowOptions};
use gpui_platform;
use std::sync::Arc;

// 全局状态
struct AppState {
    message: String,
    loading: bool,
}
impl Global for AppState {}

fn main() {
    let app = gpui_platform::application();

    // 在 gpui 事件循环之前启动 Tokio 运行时
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Failed to create Tokio runtime");

    // 进入 Tokio 上下文（使 reqwest 等需要 Tokio 的 crate 正常工作）
    let _guard = rt.enter();

    app.run(move |cx: &mut App| {
        cx.set_global(AppState {
            message: "Ready".to_string(),
            loading: false,
        });

        let view = cx.new(|_| AsyncView);

        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(
                    Bounds::centered(None, size(px(500.0), px(300.0)), cx),
                )),
                ..Default::default()
            },
            |_, _| view,
        )
        .expect("window should open");

        cx.activate(true);
    });
}

struct AsyncView;

impl AsyncView {
    // 异步加载数据
    fn load_data(&mut self, cx: &mut Context<Self>) {
        // 标记加载中
        cx.update_global::<AppState, _>(|state, _| {
            state.loading = true;
            state.message = "Loading...".to_string();
        });

        // spawn 异步任务
        cx.spawn(|view, mut cx| async move {
            // 模拟异步操作（如 HTTP 请求）
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;

            // 更新 UI
            view.update(&mut cx, |_, cx| {
                cx.update_global::<AppState, _>(|state, _| {
                    state.loading = false;
                    state.message = "Data loaded at ".to_string()
                        + &chrono::Local::now().format("%H:%M:%S").to_string();
                });
            })
        })
        .detach();
    }
}

impl Render for AsyncView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = cx.global::<AppState>();

        div()
            .size_full()
            .items_center()
            .justify_center()
            .flex_col()
            .gap(px(16.0))
            .child(state.message.clone())
            .child(
                div()
                    .id("load-btn")
                    .px(px(16.0))
                    .py(px(8.0))
                    .cursor(CursorStyle::PointingHand)
                    .on_mouse_down(MouseButton::Left, {
                        let cx = cx.weak_entity();
                        move |_, _, _| {
                            cx.update(|_, cx| {
                                // 通过 WeakEntity 调用
                            })
                            .ok();
                        }
                    })
                    .child("Load Data"),
            )
    }
}
```

**与 Tokio 集成的要点**：
- Tokio 运行时必须在 `app.run()` 之前创建并 `enter()`
- 使用 `cx.spawn()` 创建 GPUI 管理的异步任务
- 异步任务内用 `view.update(&mut cx, ...)` 安全更新 UI
- `.detach()` 让任务在后台运行，不阻塞主线程

---

## 6. API 参考

### gpui_platform::application()

```rust
pub fn application() -> Application
```

创建平台特定的 Application 实例。

| 平台 | 后端 |
|------|------|
| macOS | AppKit/Cocoa |
| Linux (Wayland) | Wayland + libxkbcommon |
| Linux (X11) | X11 + XCB |
| Web | WebAssembly |

### Application::run()

```rust
pub fn run<F>(self, closure: F)
where
    F: FnOnce(&mut App),
```

进入 GPUI 事件循环。`closure` 在事件循环开始前执行一次，用于初始化窗口和状态。

### WindowOptions

```rust
struct WindowOptions {
    window_bounds: Option<WindowBounds>,      // 窗口大小和位置
    window_decorations: Option<WindowDecorations>, // 窗口装饰
    titlebar: Option<TitlebarStyle>,          // 标题栏样式 (macOS)
    kind: Option<WindowKind>,                 // 窗口类型
    display_id: Option<DisplayId>,            // 指定显示器
    parent: Option<OwnedWindowHandle>,        // 父窗口
    focus: Option<bool>,                      // 是否聚焦
    show: Option<bool>,                       // 是否显示
    is_movable: Option<bool>,                 // 是否可移动
}
```

### WindowBounds

```rust
enum WindowBounds {
    Windowed(Bounds<BoundsUnit>),  // 指定大小和位置
    Maximized,                      // 最大化
    Fullscreen,                     // 全屏
}
```

### WindowDecorations

```rust
enum WindowDecorations {
    Server,  // 系统装饰（标题栏、边框、关闭/最小化/最大化按钮）
    Client,  // 无边框，自己绘制装饰
}
```

### Window 常用方法

| 方法 | 说明 |
|------|------|
| `window.minimize_window()` | 最小化 |
| `window.zoom_window()` | 最大化/恢复切换 |
| `window.start_window_move()` | 开始拖拽移动 |
| `window.start_resize(resize_edge)` | 开始拖拽调整大小 |

### App 常用方法

| 方法 | 说明 |
|------|------|
| `cx.new(\|cx\| View)` | 创建 Entity |
| `cx.open_window(options, \|w, cx\| view)` | 打开窗口 |
| `cx.set_global(T)` | 设置全局状态 |
| `cx.global::\<T\>()` | 读取全局状态 |
| `cx.update_global::\<T, _\>(\|state, cx\| {})` | 修改全局状态 |
| `cx.bind_keys([KeyBinding::new(...)])` | 注册快捷键 |
| `cx.quit()` | 退出应用 |
| `cx.activate(flag)` | 激活应用 |
| `cx.spawn(\|entity, cx\| async {}).detach()` | 启动异步任务 |

---

## 7. 常见问题

### Q1: 编译报错 "unreachable!" 在 Linux 上

**原因**：没有启用 `wayland` 或 `x11` feature。

**解决**：
```toml
gpui_platform = { ..., features = ["wayland", "x11"] }
```

### Q2: 编译报错找不到系统库

**原因**：缺少开发依赖。

**解决**：安装 `libxkbcommon-dev`, `libwayland-dev`, `libvulkan-dev` 等。

### Q3: 运行时窗口打不开

**原因**：没有显示服务器（如纯 SSH 环境）。

**解决**：设置 `HIVEGUI_HEADLESS=1` 跳过窗口打开，或使用 X11 转发。

### Q4: gpui_platform::application() 和 gpui::Application::new() 有什么区别？

- `gpui_platform::application()` 是跨平台的工厂函数，自动选择后端
- `gpui::Application::new()` 是内部方法，通常不需要直接调用

### Q5: app.run() 会阻塞吗？

是的，`app.run()` 会阻塞主线程直到应用退出（调用 `cx.quit()` 或关闭所有窗口）。

### Q6: 如何在 app.run() 之前做异步初始化？

使用 Tokio 的 `block_on`：

```rust
let rt = tokio::runtime::Builder::new_multi_thread().build()?;
let _guard = rt.enter();

let data = rt.block_on(async {
    // 异步初始化
    fetch_config().await
});

app.run(move |cx| {
    // 使用 data
});
```

### Q7: 支持 Windows 吗？

GPUI 目前主要支持 macOS 和 Linux。Windows 支持在开发中。

---

## 附录：学习检查清单

完成以下目标即掌握了 GPUI 应用创建：

- [ ] 能写出最简 GPUI 应用并运行
- [ ] 能配置窗口大小、位置、装饰类型
- [ ] 理解 `Bounds::centered()` 的工作原理
- [ ] 能创建和使用全局状态（Global）
- [ ] 能注册键盘快捷键并绑定动作
- [ ] 理解 `actions!` 宏的用法
- [ ] 能实现自定义标题栏（无边框窗口）
- [ ] 能实现窗口的移动、最小化、最大化、关闭
- [ ] 能打开多个窗口
- [ ] 能集成 Tokio 异步运行时
- [ ] 理解 `cx.spawn().detach()` 的异步任务模式
