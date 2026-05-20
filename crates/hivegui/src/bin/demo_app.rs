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