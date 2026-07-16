//! AI 管理视图 - 包含 Agent、工具、技能
use crate::datasource::Store;
use crate::ui::{agent_view::AgentView, skill_view::SkillView, tool_view::ToolView};
use gpui::*;
use gpui_component::ActiveTheme as _;
use gpui_component::scroll::ScrollableElement;
use gpui_component::tab::{Tab, TabBar};

pub struct AiView {
    active_tab: usize,
    agent_view: Entity<AgentView>,
    tool_view: Entity<ToolView>,
    skill_view: Entity<SkillView>,
}

impl AiView {
    pub fn new(cx: &mut Context<Self>, store: Entity<Store>) -> Self {
        let agent_view = cx.new(|cx| AgentView::new(store.clone(), cx));
        let tool_view = cx.new(|cx| ToolView::new(store.clone(), cx));
        let skill_view = cx.new(|cx| SkillView::new(store.clone(), cx));

        AiView {
            active_tab: 0,
            agent_view,
            tool_view,
            skill_view,
        }
    }
}

impl Render for AiView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            // Tab bar
            .child(
                TabBar::new("ai-tabs")
                    .selected_index(self.active_tab)
                    .on_click(cx.listener(|this, index, _, cx| {
                        this.active_tab = *index;
                        cx.notify();
                    }))
                    .child(Tab::new().label("Agent"))
                    .child(Tab::new().label("工具"))
                    .child(Tab::new().label("技能")),
            )
            // Content
            .child(div().flex_1().child(match self.active_tab {
                0 => self.agent_view.clone().into_any_element(),
                1 => self.tool_view.clone().into_any_element(),
                2 => self.skill_view.clone().into_any_element(),
                _ => div().into_any_element(),
            }))
    }
}
