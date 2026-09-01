//! 工具视图 - 包含 LLM 提示词调试和数据源等桌面辅助工具。

use crate::datasource::Store;
use crate::datasource::llm_store::LlmStore;
use crate::ui::datasource_view::DataSourceView;
use crate::ui::prompt_debugger::PromptDebugger;
use gpui::*;
use gpui_component::ActiveTheme as _;
use gpui_component::tab::{Tab, TabBar};

/// `UtilityView` 内嵌标签页索引。
const TAB_PROMPT_DEBUGGER: usize = 0;
const TAB_DATASOURCE: usize = 1;

pub struct UtilityView {
    active_tab: usize,
    prompt_debugger: Entity<PromptDebugger>,
    datasource_view: Entity<DataSourceView>,
}

impl UtilityView {
    pub fn new(cx: &mut Context<Self>, store: Entity<Store>, llm_store: LlmStore) -> Self {
        Self {
            active_tab: 0,
            prompt_debugger: cx.new(|cx| PromptDebugger::new(cx, store.clone(), llm_store)),
            datasource_view: cx.new(|cx| DataSourceView::new(store, cx)),
        }
    }

    /// 切换到指定的工具页标签。
    ///
    /// 当目标标签是 LLM 提示词调试（且当前不是该标签）时，强制刷新
    /// Preset / Model / Provider 列表：用户可能在数据源或其他视图中
    /// 调整过它们，回到本视图应看到最新数据。
    fn switch_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        let previous = self.active_tab;
        self.active_tab = index;
        if index == TAB_PROMPT_DEBUGGER && previous != TAB_PROMPT_DEBUGGER {
            self.prompt_debugger
                .update(cx, |view, cx| view.reload_presets_and_models(cx));
        }
        cx.notify();
    }

    /// 通过侧边栏/菜单等外部导航进入本视图时，强制刷新 LLM 提示词
    /// 调试中的 Preset / Model / Provider 列表。用户在 AI 管理或其他
    /// 入口调整过它们后，再导航回本视图应看到最新数据。
    pub fn refresh_prompt_debugger(&mut self, cx: &mut Context<Self>) {
        self.prompt_debugger
            .update(cx, |view, cx| view.reload_presets_and_models(cx));
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
                        this.switch_tab(*index, cx);
                    }))
                    .child(Tab::new().label("LLM 提示词调试"))
                    .child(Tab::new().label("数据源")),
            )
            .child(div().flex_1().min_h_0().child(match self.active_tab {
                TAB_PROMPT_DEBUGGER => self.prompt_debugger.clone().into_any_element(),
                TAB_DATASOURCE => self.datasource_view.clone().into_any_element(),
                _ => div().into_any_element(),
            }))
    }
}
