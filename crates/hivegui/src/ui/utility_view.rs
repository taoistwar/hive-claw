//! 工具视图 - 包含 LLM 提示词调试和数据源等桌面辅助工具。

use crate::config::AppIdentity;
use crate::datasource::Store;
use crate::datasource::llm_store::LlmStore;
use crate::ui::datasource_view::DataSourceView;
use crate::ui::prompt_debugger::PromptDebugger;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::*;

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
            prompt_debugger: cx
                .new(|cx| PromptDebugger::new(cx, store.clone(), llm_store, AppIdentity::HIVEGUI)),
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
            self.refresh_prompt_debugger(cx);
        }
        cx.notify();
    }

    /// 通过侧边栏/菜单等外部导航进入本视图时，强制刷新 LLM 提示词
    /// 调试的 Preset / Model / Provider 列表与运行期开关（查看/对比是否使用
    /// 独立窗口）。用户在 AI 管理（含「全局配置」页）或其他入口调整过它们后，
    /// 再导航回本视图应看到最新行为，而不是重启后才知道改错了。
    pub fn refresh_prompt_debugger(&mut self, cx: &mut Context<Self>) {
        self.prompt_debugger.update(cx, |view, cx| {
            view.reload_presets_and_models(cx);
            view.reload_detached_window_settings(cx);
        });
    }

    /// 关掉内嵌提示词调试页打开的执行历史独立窗口（查看 / 对比）。
    ///
    /// 桌面版主窗口关闭时调用：这些独立窗口是主窗口的派生窗口，主窗口退出后
    /// 不应继续留在桌面上（否则应用会因为有窗口存在而不退出）。
    pub fn close_prompt_debugger_windows(&mut self, cx: &mut Context<Self>) {
        self.prompt_debugger
            .update(cx, |view, cx| view.close_all_detached_windows(cx));
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
