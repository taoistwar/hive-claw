//! 扩展管理视图 - 包含插件、函数、流程
use crate::datasource::Store;
use crate::ui::{
    function_view::FunctionView, plugin_view::PluginView, workflow_view::WorkflowView,
};
use gpui::*;
use gpui_component::ActiveTheme as _;
use gpui_component::tab::{Tab, TabBar};

pub struct ExtensionView {
    active_tab: usize,
    plugin_view: Entity<PluginView>,
    function_view: Entity<FunctionView>,
    workflow_view: Entity<WorkflowView>,
}

impl ExtensionView {
    pub fn new(cx: &mut Context<Self>, store: Entity<Store>) -> Self {
        let plugin_view = cx.new(|cx| PluginView::new(store.clone(), cx));
        let function_view = cx.new(|cx| FunctionView::new(store.clone(), cx));
        let workflow_view = cx.new(|cx| WorkflowView::new(store.clone(), cx));

        ExtensionView {
            active_tab: 0,
            plugin_view,
            function_view,
            workflow_view,
        }
    }
}

impl Render for ExtensionView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            // Tab bar
            .child(
                TabBar::new("extension-tabs")
                    .selected_index(self.active_tab)
                    .on_click(cx.listener(|this, index, _, cx| {
                        this.active_tab = *index;
                        cx.notify();
                    }))
                    .child(Tab::new().label("插件"))
                    .child(Tab::new().label("函数"))
                    .child(Tab::new().label("流程")),
            )
            // Content
            .child(div().flex_1().min_h_0().child(match self.active_tab {
                0 => self.plugin_view.clone().into_any_element(),
                1 => self.function_view.clone().into_any_element(),
                2 => self.workflow_view.clone().into_any_element(),
                _ => div().into_any_element(),
            }))
    }
}
