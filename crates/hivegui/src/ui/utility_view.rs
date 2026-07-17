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
