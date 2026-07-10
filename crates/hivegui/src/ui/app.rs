use std::sync::{Arc, Mutex};

use gpui::{
    App, Bounds, Context, CursorStyle, Entity, KeyBinding, MouseButton, SharedString, Window,
    WindowBounds, WindowDecorations, WindowOptions, div, prelude::*, px, rgb, size,
};

use crate::client;
use crate::config::Config;
use crate::datasource::Store;
use crate::model::conversation::Conversation;
use crate::model::tools::ToolSeriesKind;
use crate::ui::input::{
    Backspace, Copy, Cut, Delete, End, Home, Left, Paste, Right, SelectAll, SelectLeft,
    SelectRight, Submit,
};
use crate::ui::{
    conversation::{ConversationView, PendingInput},
    datasource_view::DataSourceView,
    home::HomeView,
    sidebar_nav::SidebarNav,
    tools_section::ToolsSectionView,
};

/// Top-level HiveGUI application state. Owned by the gpui app and
/// re-entered by every view via `cx.global::<HiveGuiAppState>()`. Keeping the
/// `Conversation` model here is what guarantees state preservation across
/// surface navigations (SC-004 / FR-010 / cross-entity rule C1).
pub struct HiveGuiAppState {
    pub config: Arc<Config>,
    pub conversation: Entity<Conversation>,
    pub http: Arc<reqwest::Client>,
    pub pending_input: Arc<Mutex<PendingInput>>,
    pub route: AppRoute,
    pub store: Entity<Store>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppRoute {
    Home,
    Conversation,
    Tools(ToolSeriesKind),
    DataSource,
}

impl AppRoute {
    pub fn display_name(&self) -> &'static str {
        match self {
            AppRoute::Home => "首页",
            AppRoute::Conversation => "与 HiveClaw 对话",
            AppRoute::Tools(ToolSeriesKind::DayPlusOne) => "Day+1 工具",
            AppRoute::Tools(ToolSeriesKind::HourPlusOne) => "Hour+1 工具",
            AppRoute::DataSource => "数据源管理",
        }
    }
}

impl gpui::Global for HiveGuiAppState {}

pub fn run(config: Config) -> anyhow::Result<()> {
    let app = gpui_platform::application();
    let cfg = Arc::new(config);
    let http = Arc::new(client::build_client());

    let db_path = Store::default_db_path();
    let store = tokio::runtime::Handle::current()
        .block_on(Store::new(&db_path))
        .expect("Failed to initialize data source store");

    app.run(move |cx: &mut App| {
        let conversation = cx.new(|_| Conversation::new());
        let store = cx.new(|_| store);

        cx.set_global(HiveGuiAppState {
            config: cfg.clone(),
            conversation,
            http: http.clone(),
            pending_input: Arc::new(Mutex::new(PendingInput::default())),
            route: AppRoute::Home,
            store,
        });

        cx.bind_keys([
            KeyBinding::new("backspace", Backspace, Some("TextInput")),
            KeyBinding::new("delete", Delete, Some("TextInput")),
            KeyBinding::new("left", Left, Some("TextInput")),
            KeyBinding::new("right", Right, Some("TextInput")),
            KeyBinding::new("shift-left", SelectLeft, Some("TextInput")),
            KeyBinding::new("shift-right", SelectRight, Some("TextInput")),
            KeyBinding::new("cmd-a", SelectAll, Some("TextInput")),
            KeyBinding::new("ctrl-a", SelectAll, Some("TextInput")),
            KeyBinding::new("home", Home, Some("TextInput")),
            KeyBinding::new("end", End, Some("TextInput")),
            KeyBinding::new("cmd-v", Paste, Some("TextInput")),
            KeyBinding::new("ctrl-v", Paste, Some("TextInput")),
            KeyBinding::new("cmd-c", Copy, Some("TextInput")),
            KeyBinding::new("ctrl-c", Copy, Some("TextInput")),
            KeyBinding::new("cmd-x", Cut, Some("TextInput")),
            KeyBinding::new("ctrl-x", Cut, Some("TextInput")),
            KeyBinding::new("enter", Submit, Some("TextInput")),
            KeyBinding::new("cmd-enter", Submit, Some("TextInput")),
            KeyBinding::new("ctrl-enter", Submit, Some("TextInput")),
        ]);

        let bounds = Bounds::centered(None, size(px(1200.0), px(700.0)), cx);
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
    sidebar: Entity<SidebarNav>,
    home: Entity<HomeView>,
    conversation: Entity<ConversationView>,
    day_plus_one: Entity<ToolsSectionView>,
    hour_plus_one: Entity<ToolsSectionView>,
    data_source: Entity<DataSourceView>,
}

impl RootView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let sidebar = cx.new(SidebarNav::new);
        let home = cx.new(HomeView::new);
        let conversation = cx.new(ConversationView::new);
        let day_plus_one = cx.new(|cx| ToolsSectionView::new(ToolSeriesKind::DayPlusOne, cx));
        let hour_plus_one = cx.new(|cx| ToolsSectionView::new(ToolSeriesKind::HourPlusOne, cx));
        let store = cx.global::<HiveGuiAppState>().store.clone();
        let data_source = cx.new(|cx| DataSourceView::new(store, cx));
        RootView {
            sidebar,
            home,
            conversation,
            day_plus_one,
            hour_plus_one,
            data_source,
        }
    }
}

impl Render for RootView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let route = cx.global::<HiveGuiAppState>().route;
        let body = match route {
            AppRoute::Home => self.home.clone().into_any_element(),
            AppRoute::Conversation => self.conversation.clone().into_any_element(),
            AppRoute::Tools(ToolSeriesKind::DayPlusOne) => {
                self.day_plus_one.clone().into_any_element()
            }
            AppRoute::Tools(ToolSeriesKind::HourPlusOne) => {
                self.hour_plus_one.clone().into_any_element()
            }
            AppRoute::DataSource => self.data_source.clone().into_any_element(),
        };

        let titlebar_height = px(32.0);
        let statusbar_height = px(24.0);

        div()
            .flex()
            .flex_col()
            .size_full()
            .border_b_2()
            .border_color(rgb(0x6f5699))
            .bg(rgb(0xffffff))
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
                    .bg(rgb(0xf0f0f7))
                    .border_b_1()
                    .border_color(rgb(0xd8d8e8))
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
                            .h_full()
                            .child(window_button(
                                "―",
                                gpui::rgba(0x0000001a), // 10% 透明度的黑色
                                gpui::rgba(0x00000026), // 15% 透明度的黑色
                                rgb(0x111111),
                                rgb(0x111111),
                                |window, _, _| {
                                    window.minimize_window();
                                },
                            ))
                            .child(window_button(
                                "□",
                                gpui::rgba(0x0000001a), // 10% 透明度的黑色
                                gpui::rgba(0x00000026), // 15% 透明度的黑色
                                rgb(0x111111),
                                rgb(0x111111),
                                |window, _, _| {
                                    window.zoom_window();
                                },
                            ))
                            .child(window_button(
                                "✕",
                                gpui::rgba(0xff5f57ff), // 关闭按钮红色
                                gpui::rgba(0xff3b30ff), // 点击时更深的红色
                                rgb(0x111111),
                                rgb(0xffffff),
                                |_, _, cx| {
                                    cx.quit();
                                },
                            )),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .child(self.sidebar.clone())
                    .child(div().flex_1().child(body)),
            )
            .child(
                div()
                    .id("statusbar")
                    .flex()
                    .items_center()
                    .h(statusbar_height)
                    // .px(px(12.0))
                    .bg(rgb(0x6f5699))
                    .border_t_1()
                    .border_color(rgb(0xd8d8e8))
                    .child(
                        div()
                            .px(px(12.0))
                            .text_size(px(12.0))
                            .child(format!("当前页面：{}", route.display_name()))
                            .text_color(rgb(0xffffff))
                            .bg(rgb(0x44355d)),
                    ),
            )
    }
}

fn window_button(
    icon: &'static str,
    hover_color: gpui::Rgba,
    active_color: gpui::Rgba,
    text_color: gpui::Rgba,
    hover_text_color: gpui::Rgba,
    on_click: impl Fn(&mut Window, &gpui::MouseDownEvent, &mut gpui::App) + 'static,
) -> impl IntoElement {
    let id = SharedString::from(format!("window-button-{}", icon));
    div()
        .id(id)
        .w(px(46.0))
        .h(px(32.0))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(12.0))
        .text_color(text_color)
        .cursor(CursorStyle::PointingHand)
        .hover(|style| style.bg(hover_color).text_color(hover_text_color))
        .active(|style| style.bg(active_color))
        .child(icon)
        .on_mouse_down(MouseButton::Left, move |event, window, cx| {
            on_click(window, event, cx);
        })
}
