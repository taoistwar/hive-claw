//! AI 管理视图 - 统一承载 AI、扩展和系统配置管理。
use std::sync::Arc;

use gpui::*;
use gpui_component::ActiveTheme as _;
use gpui_component::tab::{Tab, TabBar};

use crate::agent::local_agent::LocalAgentRuntime;
use crate::datasource::Store;
use crate::datasource::llm_store::LlmStore;
use crate::runtime::diagnostics::ExecutionEventCollector;
use crate::ui::{
    agent_view::AgentView, capability_view::CapabilityView, category_view::CategoryView,
    conversation_view::ConversationView, function_view::FunctionView,
    global_config::GlobalConfigView, llm_config::LLMConfigView, plugin_view::PluginView,
    settings_view::SettingsView, skill_view::SkillView, tag_view::TagView, tool_view::ToolView,
    workflow_view::WorkflowView,
};

fn ai_management_tab_labels() -> [&'static str; 13] {
    [
        "会话",
        "Agent",
        "工具",
        "技能",
        "函数",
        "流程",
        "插件",
        "LLM",
        "Capabilities",
        "分类",
        "标签",
        "数据管理",
        "全局配置",
    ]
}

pub struct AiView {
    active_tab: usize,
    agent_view: Entity<AgentView>,
    tool_view: Entity<ToolView>,
    skill_view: Entity<SkillView>,
    function_view: Entity<FunctionView>,
    workflow_view: Entity<WorkflowView>,
    plugin_view: Entity<PluginView>,
    llm_config: Entity<LLMConfigView>,
    capability_view: Entity<CapabilityView>,
    category_view: Entity<CategoryView>,
    tag_view: Entity<TagView>,
    conversation_view: Entity<ConversationView>,
    settings_view: Entity<SettingsView>,
    global_config: Entity<GlobalConfigView>,
}

impl AiView {
    pub fn new(
        cx: &mut Context<Self>,
        store: Entity<Store>,
        llm_store: LlmStore,
        local_agent_runtime: Arc<LocalAgentRuntime>,
        execution_event_collector: Arc<ExecutionEventCollector>,
    ) -> Self {
        let agent_view = cx.new(|cx| AgentView::new(store.clone(), cx));
        let tool_view = cx.new(|cx| ToolView::new(store.clone(), cx));
        let skill_view = cx.new(|cx| SkillView::new(store.clone(), cx));
        let function_view = cx.new(|cx| FunctionView::new(store.clone(), cx));
        let workflow_view = cx.new(|cx| WorkflowView::new(store.clone(), cx));
        let plugin_view = cx.new(|cx| PluginView::new(store.clone(), cx));
        let llm_config = cx.new(|cx| {
            let mut view = LLMConfigView::new(cx);
            view.llm_store = Some(llm_store);
            view
        });
        let capability_view = cx.new(|cx| CapabilityView::new(store.clone(), cx));
        let category_view = cx.new(|cx| CategoryView::new(store.clone(), cx));
        let tag_view = cx.new(|cx| TagView::new(store.clone(), cx));
        let conversation_view = cx.new(|cx| {
            ConversationView::new(
                cx,
                Some(local_agent_runtime.as_ref().clone()),
                Some(store.read(cx).clone()),
                execution_event_collector.clone(),
            )
        });
        let settings_view = cx.new(|cx| {
            let mut view = SettingsView::new(cx, Some(execution_event_collector.clone()));
            view.set_store(store.read(cx).clone());
            view
        });
        let global_config = cx.new(|cx| {
            let mut view = GlobalConfigView::new(cx);
            view.store = Some(store.read(cx).clone());
            view
        });

        AiView {
            active_tab: 0,
            agent_view,
            tool_view,
            skill_view,
            function_view,
            workflow_view,
            plugin_view,
            llm_config,
            capability_view,
            category_view,
            tag_view,
            conversation_view,
            settings_view,
            global_config,
        }
    }
}

impl Render for AiView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tabs = ai_management_tab_labels().into_iter().fold(
            TabBar::new("ai-tabs")
                .menu(true)
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
            .child(tabs)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(match self.active_tab {
                        0 => self.conversation_view.clone().into_any_element(),
                        1 => self.agent_view.clone().into_any_element(),
                        2 => self.tool_view.clone().into_any_element(),
                        3 => self.skill_view.clone().into_any_element(),
                        4 => self.function_view.clone().into_any_element(),
                        5 => self.workflow_view.clone().into_any_element(),
                        6 => self.plugin_view.clone().into_any_element(),
                        7 => self.llm_config.clone().into_any_element(),
                        8 => self.capability_view.clone().into_any_element(),
                        9 => self.category_view.clone().into_any_element(),
                        10 => self.tag_view.clone().into_any_element(),
                        11 => self.settings_view.clone().into_any_element(),
                        12 => self.global_config.clone().into_any_element(),
                        _ => div().into_any_element(),
                    }),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::ai_management_tab_labels;

    #[test]
    fn ai_management_contains_all_management_tabs_in_order() {
        assert_eq!(
            ai_management_tab_labels(),
            [
                "会话",
                "Agent",
                "工具",
                "技能",
                "函数",
                "流程",
                "插件",
                "LLM",
                "Capabilities",
                "分类",
                "标签",
                "数据管理",
                "全局配置",
            ]
        );
    }
}
