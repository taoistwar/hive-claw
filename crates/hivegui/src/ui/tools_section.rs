use gpui::{div, prelude::*, px, rgb, Context, MouseButton, Window};

use crate::model::tools::{ToolSeries, ToolSeriesKind};
use crate::ui::app::{AppRoute, HiveGuiApp};
use crate::ui::strings_zh;

pub struct ToolsSectionView {
    series: ToolSeries,
}

impl ToolsSectionView {
    pub fn new(kind: ToolSeriesKind, _cx: &mut Context<Self>) -> Self {
        ToolsSectionView {
            series: ToolSeries::for_kind(kind),
        }
    }
}

impl Render for ToolsSectionView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let title = self.series.display_name_zh();
        let body = if self.series.tools.is_empty() {
            div()
                .text_color(rgb(0x666666))
                .text_size(px(14.0))
                .child(strings_zh::EMPTY_TOOLS.to_string())
                .into_any_element()
        } else {
            // Forward-compatible non-empty branch. v1 never reaches here
            // (invariant H3); the code path exists so a future feature can
            // populate `tools` without touching navigation.
            let mut col = div().flex().flex_col().gap(px(8.0));
            for tool in self.series.tools.iter() {
                col = col.child(
                    div()
                        .px(px(12.0))
                        .py(px(8.0))
                        .rounded(px(6.0))
                        .bg(rgb(0xffffff))
                        .border_1()
                        .border_color(rgb(0xd0d0d0))
                        .child(tool.display_name_zh.clone()),
                );
            }
            col.into_any_element()
        };

        div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .p(px(24.0))
            .size_full()
            .child(top_bar())
            .child(
                div()
                    .text_color(rgb(0x111111))
                    .text_size(px(22.0))
                    .child(title.to_string()),
            )
            .child(body)
    }
}

fn top_bar() -> impl IntoElement {
    div()
        .id("back")
        .px(px(12.0))
        .py(px(6.0))
        .text_color(rgb(0x444444))
        .cursor_pointer()
        .child(format!("← {}", strings_zh::HOME_TITLE))
        .on_mouse_down(MouseButton::Left, |_, _, cx| {
            cx.update_global::<HiveGuiApp, _>(|app, _| app.route = AppRoute::Home);
            cx.refresh_windows();
        })
}
