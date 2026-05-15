use std::sync::{Arc, Mutex};

use gpui::{
    div, prelude::*, px, rgb, size, App, Bounds, Context, CursorStyle, Entity, MouseButton,
    SharedString, Window, WindowBounds, WindowDecorations, WindowOptions,
};

use crate::client;
use crate::config::Config;
use crate::model::conversation::Conversation;
use crate::model::tools::ToolSeriesKind;
use crate::ui::{
    conversation::{ConversationView, PendingInput},
    home::HomeView,
    tools_section::ToolsSectionView,
};

/// Top-level HiveGUI application state. Owned by the gpui app and
/// re-entered by every view via `cx.global::<HiveGuiApp>()`. Keeping the
/// `Conversation` model here is what guarantees state preservation across
/// surface navigations (SC-004 / FR-010 / cross-entity rule C1).
pub struct HiveGuiApp {
    pub config: Arc<Config>,
    pub conversation: Entity<Conversation>,
    pub http: Arc<reqwest::Client>,
    /// Mirror of the user's currently-pending input (editor text +
    /// attachments + transient error). The active `ConversationView`
    /// reads/writes this on every render; the network dispatcher drains
    /// it on send. Mutex'd because gpui's view-update lifetime would
    /// otherwise make borrowing through `Entity<ConversationView>`
    /// awkward to thread into a Tokio task.
    pub pending_input: Arc<Mutex<PendingInput>>,
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
    let http = Arc::new(client::build_client());

    app.run(move |cx: &mut App| {
        let conversation = cx.new(|_| Conversation::new());
        cx.set_global(HiveGuiApp {
            config: cfg.clone(),
            conversation,
            http: http.clone(),
            pending_input: Arc::new(Mutex::new(PendingInput::default())),
            route: AppRoute::Home,
        });

        let bounds = Bounds::centered(None, size(px(960.0), px(640.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_decorations: Some(WindowDecorations::Server),
                ..Default::default()
            },
            |_, cx| cx.new(RootView::new),
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
        let home = cx.new(HomeView::new);
        let conversation = cx.new(ConversationView::new);
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

        let titlebar_height = px(32.0);

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(0xf7f7f7))
            .text_color(rgb(0x111111))
            .child(
                div()
                    .id("titlebar")
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .h(titlebar_height)
                    .px(px(8.0))
                    .bg(rgb(0xe8e8e8))
                    .border_b_1()
                    .border_color(rgb(0xe0e0e0))
                    .cursor(CursorStyle::OpenHand)
                    .on_mouse_down(MouseButton::Left, |event, window, _cx| {
                        if event.click_count == 2 {
                            window.zoom_window();
                        } else {
                            window.start_window_move();
                        }
                    })
                    .child(div().text_size(px(13.0)).child("HiveClaw"))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap(px(4.0))
                            .child(window_button("—", |window, _, _| {
                                window.minimize_window();
                            }))
                            .child(window_button("□", |window, _, _| {
                                window.zoom_window();
                            }))
                            .child(window_button("✕", |_, _, cx| {
                                cx.quit();
                            })),
                    ),
            )
            .child(body)
    }
}

fn window_button(
    label: &str,
    on_click: impl Fn(&mut Window, &gpui::MouseDownEvent, &mut gpui::App) + 'static,
) -> impl IntoElement {
    let label = SharedString::from(label);
    div()
        .id(label.clone())
        .w(px(30.0))
        .h(px(24.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.0))
        .text_size(px(14.0))
        .text_color(rgb(0x555555))
        .cursor(CursorStyle::PointingHand)
        .hover(|style| style.bg(rgb(0xe8e8e8)))
        .active(|style| style.bg(rgb(0xd8d8d8)))
        .child(label)
        .on_mouse_down(MouseButton::Left, move |event, window, cx| {
            on_click(window, event, cx);
        })
}
