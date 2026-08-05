use gpui::{
    AnyElement, Context, CursorStyle, MouseButton, SharedString, Window, div, prelude::*, px, rgb,
};

use crate::model::tools::ToolSeriesKind;
use crate::ui::app::{AppRoute, HiveGuiAppState};

pub struct SidebarNav {
    #[expect(
        dead_code,
        reason = "retained until route ownership is consolidated with HiveGuiAppState"
    )]
    active_route: AppRoute,
}

impl SidebarNav {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let active_route = cx.global::<HiveGuiAppState>().route;
        SidebarNav { active_route }
    }
}

impl Render for SidebarNav {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 从全局状态读取最新的 route，而不是使用组件自己的字段
        let current_route = cx.global::<HiveGuiAppState>().route;

        div()
            .flex()
            .flex_col()
            .w(px(64.0))
            .h_full()
            .bg(rgb(0xf0f0f7))
            .items_center()
            .py(px(12.0))
            .gap(px(8.0))
            .child(self.nav_button(
                "home",
                icon_home(current_route == AppRoute::Home),
                AppRoute::Home,
                window,
                cx,
            ))
            .child(self.nav_button(
                "conversation",
                icon_conversation(current_route == AppRoute::Conversation),
                AppRoute::Conversation,
                window,
                cx,
            ))
            .child(self.nav_button(
                "day_plus_one",
                icon_day(current_route == AppRoute::Tools(ToolSeriesKind::DayPlusOne)),
                AppRoute::Tools(ToolSeriesKind::DayPlusOne),
                window,
                cx,
            ))
            .child(self.nav_button(
                "hour_plus_one",
                icon_hour(current_route == AppRoute::Tools(ToolSeriesKind::HourPlusOne)),
                AppRoute::Tools(ToolSeriesKind::HourPlusOne),
                window,
                cx,
            ))
            .child(self.nav_button(
                "datasource",
                icon_datasource(current_route == AppRoute::DataSource),
                AppRoute::DataSource,
                window,
                cx,
            ))
    }
}

impl SidebarNav {
    fn nav_button(
        &self,
        id: &str,
        icon: AnyElement,
        route: AppRoute,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let id = SharedString::from(id);
        let is_active = cx.global::<HiveGuiAppState>().route == route;

        div()
            .id(id)
            .w(px(48.0))
            .h(px(48.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(8.0))
            .cursor(CursorStyle::PointingHand)
            .bg(if is_active {
                rgb(0xd8d8e8)
            } else {
                rgb(0xf0f0f7)
            })
            .hover(|style| style.bg(rgb(0xd8d8e8)))
            .child(icon)
            .on_mouse_down(MouseButton::Left, move |_event, _window, cx| {
                cx.update_global::<HiveGuiAppState, _>(|app, _| app.route = route);
                cx.refresh_windows();
            })
    }
}

fn icon_home(_active: bool) -> AnyElement {
    div()
        .w(px(28.0))
        .h(px(28.0))
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .w(px(24.0))
                .h(px(24.0))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .w(px(20.0))
                        .h(px(20.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .w(px(18.0))
                                .h(px(18.0))
                                .border_2()
                                .border_color(rgb(0x6b5b7a))
                                .rounded(px(4.0))
                                .child(
                                    div()
                                        .w_full()
                                        .h_full()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .child(
                                            div()
                                                .w(px(10.0))
                                                .h(px(10.0))
                                                .rounded(px(2.0))
                                                .bg(rgb(0x6b5b7a)),
                                        ),
                                ),
                        ),
                ),
        )
        .into_any()
}

fn icon_conversation(_active: bool) -> AnyElement {
    div()
        .w(px(28.0))
        .h(px(28.0))
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .w(px(24.0))
                .h(px(24.0))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .w(px(20.0))
                        .h(px(20.0))
                        .border_2()
                        .border_color(rgb(0x6b5b7a))
                        .rounded(px(6.0))
                        .child(
                            div()
                                .w_full()
                                .h_full()
                                .flex()
                                .flex_col()
                                .justify_center()
                                .items_center()
                                .gap(px(2.0))
                                .child(
                                    div()
                                        .w(px(12.0))
                                        .h(px(2.0))
                                        .bg(rgb(0x6b5b7a))
                                        .rounded(px(1.0)),
                                )
                                .child(
                                    div()
                                        .w(px(12.0))
                                        .h(px(2.0))
                                        .bg(rgb(0x6b5b7a))
                                        .rounded(px(1.0)),
                                )
                                .child(
                                    div()
                                        .w(px(8.0))
                                        .h(px(2.0))
                                        .bg(rgb(0x6b5b7a))
                                        .rounded(px(1.0)),
                                ),
                        ),
                ),
        )
        .into_any()
}

fn icon_day(_active: bool) -> AnyElement {
    div()
        .w(px(28.0))
        .h(px(28.0))
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .w(px(24.0))
                .h(px(24.0))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .w(px(20.0))
                        .h(px(20.0))
                        .border_2()
                        .border_color(rgb(0x6b5b7a))
                        .rounded(px(4.0))
                        .child(
                            div()
                                .w_full()
                                .h_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    div()
                                        .w(px(10.0))
                                        .h(px(10.0))
                                        .rounded(px(2.0))
                                        .bg(rgb(0x6b5b7a)),
                                ),
                        ),
                ),
        )
        .into_any()
}

fn icon_hour(_active: bool) -> AnyElement {
    div()
        .w(px(28.0))
        .h(px(28.0))
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .w(px(24.0))
                .h(px(24.0))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .w(px(20.0))
                        .h(px(20.0))
                        .border_2()
                        .border_color(rgb(0x6b5b7a))
                        .rounded_full()
                        .child(
                            div()
                                .w_full()
                                .h_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    div()
                                        .w(px(3.0))
                                        .h(px(8.0))
                                        .rounded(px(1.5))
                                        .bg(rgb(0x6b5b7a)),
                                ),
                        ),
                ),
        )
        .into_any()
}

fn icon_datasource(_active: bool) -> AnyElement {
    div()
        .w(px(28.0))
        .h(px(28.0))
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .w(px(24.0))
                .h(px(24.0))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .w(px(20.0))
                        .h(px(20.0))
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap(px(2.0))
                        .child(
                            div()
                                .w(px(16.0))
                                .h(px(3.0))
                                .bg(rgb(0x6b5b7a))
                                .rounded(px(1.5)),
                        )
                        .child(
                            div()
                                .w(px(16.0))
                                .h(px(3.0))
                                .bg(rgb(0x6b5b7a))
                                .rounded(px(1.5)),
                        )
                        .child(
                            div()
                                .w(px(16.0))
                                .h(px(3.0))
                                .bg(rgb(0x6b5b7a))
                                .rounded(px(1.5)),
                        ),
                ),
        )
        .into_any()
}
