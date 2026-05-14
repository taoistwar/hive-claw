use gpui::{div, prelude::*, px, rgb, Context, MouseButton, Window};

use crate::model::tools::ToolSeriesKind;
use crate::ui::app::{AppRoute, HiveGuiApp};
use crate::ui::strings_zh;

pub struct HomeView;

impl HomeView {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        HomeView
    }
}

impl Render for HomeView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .p(px(24.0))
            .size_full()
            .child(
                div()
                    .text_color(rgb(0x111111))
                    .text_size(px(28.0))
                    .child(strings_zh::HOME_TITLE),
            )
            .child(section_button(
                strings_zh::CONVERSATION_ENTRY,
                AppRoute::Conversation,
            ))
            .child(section_button(
                strings_zh::DAY_PLUS_ONE_LABEL,
                AppRoute::Tools(ToolSeriesKind::DayPlusOne),
            ))
            .child(section_button(
                strings_zh::HOUR_PLUS_ONE_LABEL,
                AppRoute::Tools(ToolSeriesKind::HourPlusOne),
            ))
    }
}

fn section_button(label: &'static str, target: AppRoute) -> impl IntoElement {
    div()
        .id(label)
        .px(px(16.0))
        .py(px(12.0))
        .rounded(px(8.0))
        .bg(rgb(0xffffff))
        .border_1()
        .border_color(rgb(0xd0d0d0))
        .text_size(px(16.0))
        .text_color(rgb(0x111111))
        .cursor_pointer()
        .child(label)
        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
            cx.update_global::<HiveGuiApp, _>(|app, _| app.route = target);
            cx.refresh_windows();
        })
}
