//! 系统设置视图 - 包含全局配置、Capabilities、分类、标签、LLM、数据管理
use crate::datasource::Store;
use crate::datasource::llm_store::LlmStore;
use crate::ui::{
    capability_view::CapabilityView, category_view::CategoryView,
    global_config::GlobalConfigView, llm_config::LLMConfigView, settings_view::SettingsView,
    tag_view::TagView,
};
use gpui::*;
use gpui_component::ActiveTheme as _;
use gpui_component::tab::{Tab, TabBar};

fn system_settings_tab_labels() -> [&'static str; 6] {
    ["全局配置", "Capabilities", "分类", "标签", "LLM", "数据管理"]
}

pub struct SystemSettingsView {
    active_tab: usize,
    global_config: Entity<GlobalConfigView>,
    capability_view: Entity<CapabilityView>,
    tag_view: Entity<TagView>,
    category_view: Entity<CategoryView>,
    llm_config: Entity<LLMConfigView>,
    settings_view: Entity<SettingsView>,
}

impl SystemSettingsView {
    pub fn new(cx: &mut Context<Self>, store: Entity<Store>, llm_store: LlmStore) -> Self {
        let global_config = cx.new(|cx| {
            let mut v = GlobalConfigView::new(cx);
            v.store = Some(store.read(cx).clone());
            v
        });
        let capability_view = cx.new(|cx| CapabilityView::new(store.clone(), cx));
        let tag_view = cx.new(|cx| TagView::new(store.clone(), cx));
        let category_view = cx.new(|cx| CategoryView::new(store.clone(), cx));
        let llm_config = cx.new(|cx| {
            let mut v = LLMConfigView::new(cx);
            v.llm_store = Some(llm_store.clone());
            v
        });
        let settings_view = cx.new(|cx| {
            let mut v = SettingsView::new(cx);
            v.set_store(store.read(cx).clone());
            v
        });

        SystemSettingsView {
            active_tab: 0,
            global_config,
            capability_view,
            tag_view,
            category_view,
            llm_config,
            settings_view,
        }
    }
}

impl Render for SystemSettingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tabs = system_settings_tab_labels().into_iter().fold(
            TabBar::new("system-settings-tabs")
                .selected_index(self.active_tab)
                .on_click(cx.listener(|this, index, _, cx| {
                    this.active_tab = *index;
                    cx.notify();
                })),
            |tabs, label| tabs.child(Tab::new().label(label)),
        );

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            // Tab bar
            .child(tabs)
            // Content
            .child(div().flex_1().min_h_0().overflow_hidden().child(match self.active_tab {
                0 => self.global_config.clone().into_any_element(),
                1 => self.capability_view.clone().into_any_element(),
                2 => self.category_view.clone().into_any_element(),
                3 => self.tag_view.clone().into_any_element(),
                4 => self.llm_config.clone().into_any_element(),
                5 => self.settings_view.clone().into_any_element(),
                _ => div().into_any_element(),
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::system_settings_tab_labels;

    #[test]
    fn capabilities_precedes_categories_and_datasource_stays_outside_system_settings() {
        assert_eq!(
            system_settings_tab_labels(),
            ["全局配置", "Capabilities", "分类", "标签", "LLM", "数据管理"]
        );
    }
}
