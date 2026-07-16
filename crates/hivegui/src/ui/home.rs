use gpui::{Context, Window, div, prelude::*, px};
use gpui_component::ActiveTheme as _;

use crate::ui::strings_zh;

pub struct HomeView;

impl HomeView {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        HomeView
    }
}

impl Render for HomeView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .p(px(24.0))
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(
                div()
                    .text_color(cx.theme().foreground)
                    .text_size(px(28.0))
                    .child(strings_zh::HOME_TITLE),
            )
            .child(
                div()
                    .mt(px(24.0))
                    .text_size(px(16.0))
                    .text_color(cx.theme().muted_foreground)
                    .child("请点击左侧导航栏选择功能"),
            )
    }
}
