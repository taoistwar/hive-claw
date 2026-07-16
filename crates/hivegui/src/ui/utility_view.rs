//! 工具视图 - 包含数据源等桌面辅助工具。

use crate::datasource::Store;
use crate::ui::datasource_view::DataSourceView;
use gpui::*;
use gpui_component::ActiveTheme as _;
use gpui_component::tab::{Tab, TabBar};

pub struct UtilityView {
    datasource_view: Entity<DataSourceView>,
}

impl UtilityView {
    pub fn new(cx: &mut Context<Self>, store: Entity<Store>) -> Self {
        Self {
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
                    .selected_index(0)
                    .child(Tab::new().label("数据源")),
            )
            .child(div().flex_1().min_h_0().child(self.datasource_view.clone()))
    }
}
