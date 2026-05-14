use std::sync::Arc;

use gpui::{
    div, prelude::*, px, rgb, size, App, Bounds, Context, Entity, Window, WindowBounds,
    WindowOptions,
};

use crate::config::Config;
use crate::model::conversation::Conversation;
use crate::model::tools::ToolSeriesKind;
use crate::ui::{conversation::ConversationView, home::HomeView, tools_section::ToolsSectionView};

/// Top-level HiveGUI application state. Owned by the gpui app and
/// re-entered by every view via `cx.global::<HiveGuiApp>()`. Keeping the
/// `Conversation` model here is what guarantees state preservation across
/// surface navigations (SC-004 / FR-010 / cross-entity rule C1).
pub struct HiveGuiApp {
    pub config: Arc<Config>,
    pub conversation: Entity<Conversation>,
    pub route: AppRoute,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppRoute {
    Home,
    Conversation,
    Tools(ToolSeriesKind),
}

impl gpui::Global for HiveGuiApp {}

pub fn run(config: Config) -> anyhow::Result<()> {
    let app = gpui_platform::application();
    let cfg = Arc::new(config);

    app.run(move |cx: &mut App| {
        let conversation = cx.new(|_| Conversation::new());
        cx.set_global(HiveGuiApp {
            config: cfg.clone(),
            conversation,
            route: AppRoute::Home,
        });

        let bounds = Bounds::centered(None, size(px(960.0), px(640.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(|cx| RootView::new(cx)),
        )
        .expect("window should open");
        cx.activate(true);
    });

    Ok(())
}

/// Root view: dispatches on `HiveGuiApp::route`.
pub struct RootView {
    home: Entity<HomeView>,
    conversation: Entity<ConversationView>,
    day_plus_one: Entity<ToolsSectionView>,
    hour_plus_one: Entity<ToolsSectionView>,
}

impl RootView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let home = cx.new(|cx| HomeView::new(cx));
        let conversation = cx.new(|cx| ConversationView::new(cx));
        let day_plus_one = cx.new(|cx| ToolsSectionView::new(ToolSeriesKind::DayPlusOne, cx));
        let hour_plus_one = cx.new(|cx| ToolsSectionView::new(ToolSeriesKind::HourPlusOne, cx));
        RootView {
            home,
            conversation,
            day_plus_one,
            hour_plus_one,
        }
    }
}

impl Render for RootView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let route = cx.global::<HiveGuiApp>().route;
        let body = match route {
            AppRoute::Home => self.home.clone().into_any_element(),
            AppRoute::Conversation => self.conversation.clone().into_any_element(),
            AppRoute::Tools(ToolSeriesKind::DayPlusOne) => {
                self.day_plus_one.clone().into_any_element()
            }
            AppRoute::Tools(ToolSeriesKind::HourPlusOne) => {
                self.hour_plus_one.clone().into_any_element()
            }
        };

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(0xf7f7f7))
            .text_color(rgb(0x111111))
            .child(body)
    }
}
