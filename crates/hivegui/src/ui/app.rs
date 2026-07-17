use std::sync::Arc;

use gpui::{
    App, Bounds, Context, CursorStyle, Entity, Hsla, MouseButton, SharedString, Window,
    WindowBounds, WindowDecorations, WindowOptions, div, prelude::*, px, size,
};
use gpui_component::{
    ActiveTheme as _,
    theme::{self, Theme, ThemeRegistry},
};

use crate::config::Config;
use crate::datasource::Store;
use crate::ui::{
    ai_view::AiView, extension_view::ExtensionView, home::HomeView, sidebar_nav::SidebarNav,
    system_settings_view::SystemSettingsView, utility_view::UtilityView,
};

pub struct HiveGuiAppState {
    pub config: Arc<Config>,
    pub route: AppRoute,
    pub store: Entity<Store>,
    pub theme_name: SharedString,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppRoute {
    Home,
    SystemSettings,
    Extension,
    Ai,
    Tools,
}

impl AppRoute {
    pub fn display_name(&self) -> &'static str {
        match self {
            AppRoute::Home => "首页",
            AppRoute::SystemSettings => "系统设置",
            AppRoute::Extension => "扩展管理",
            AppRoute::Ai => "AI 管理",
            AppRoute::Tools => "工具",
        }
    }
}

impl gpui::Global for HiveGuiAppState {}

pub fn run(config: Config) -> anyhow::Result<()> {
    let app = gpui_platform::application().with_assets(gpui_component_assets::Assets);
    let cfg = Arc::new(config);

    let db_path = Store::default_db_path();
    let store = tokio::runtime::Handle::current()
        .block_on(Store::new(&db_path))
        .expect("Failed to initialize data source store");

    // Init LLM tables before app.run (avoids blocking UI thread)
    let llm_store = {
        let s = crate::datasource::llm_store::LlmStore::new(
            store.pool().clone(),
            store.crypto().clone(),
        );
        tokio::runtime::Handle::current().block_on(s.migrate()).ok();
        s
    };

    app.run(move |cx: &mut App| {
        theme::init(cx);
        gpui_component::init(cx);

        // Load custom themes
        let themes_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("themes");
        if let Err(e) = ThemeRegistry::watch_dir(themes_dir, cx, |_| {}) {
            tracing::warn!("Failed to watch themes directory: {}", e);
        }

        let store = cx.new(|_| store);
        let llm_store = llm_store.clone();
        cx.set_global(HiveGuiAppState {
            config: cfg.clone(),
            route: AppRoute::Home,
            store,
            theme_name: "Default Light".into(),
        });
        let b = Bounds::centered(None, size(px(1200.0), px(700.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(b)),
                window_decorations: Some(WindowDecorations::Server),
                ..Default::default()
            },
            move |window, cx| {
                let inner = cx.new(|cx| RootView::new(cx, llm_store.clone()));
                cx.new(|cx| gpui_component::Root::new(inner, window, cx))
            },
        )
        .expect("window should open");
        cx.activate(true);
    });
    Ok(())
}

pub struct RootView {
    sidebar: Entity<SidebarNav>,
    home: Entity<HomeView>,
    system_settings: Entity<SystemSettingsView>,
    extension: Entity<ExtensionView>,
    ai: Entity<AiView>,
    tools: Entity<UtilityView>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct ShellThemeColors {
    background: Hsla,
    foreground: Hsla,
    title_bar: Hsla,
    title_bar_border: Hsla,
    status_bar: Hsla,
    status_bar_border: Hsla,
    window_border: Hsla,
    secondary_hover: Hsla,
    secondary_active: Hsla,
    danger: Hsla,
    danger_active: Hsla,
    danger_foreground: Hsla,
}

fn shell_theme_colors(theme: &Theme) -> ShellThemeColors {
    ShellThemeColors {
        background: theme.background,
        foreground: theme.foreground,
        title_bar: theme.title_bar,
        title_bar_border: theme.title_bar_border,
        status_bar: theme.status_bar,
        status_bar_border: theme.status_bar_border,
        window_border: theme.window_border,
        secondary_hover: theme.secondary_hover,
        secondary_active: theme.secondary_active,
        danger: theme.danger,
        danger_active: theme.danger_active,
        danger_foreground: theme.danger_foreground,
    }
}

impl RootView {
    pub fn new(cx: &mut Context<Self>, llm_store: crate::datasource::llm_store::LlmStore) -> Self {
        let sidebar = cx.new(SidebarNav::new);
        let home = cx.new(HomeView::new);
        let store = cx.global::<HiveGuiAppState>().store.clone();
        let system_settings =
            cx.new(|cx| SystemSettingsView::new(cx, store.clone(), llm_store.clone()));
        let extension = cx.new(|cx| ExtensionView::new(cx, store.clone()));
        let ai = cx.new(|cx| AiView::new(cx, store.clone()));
        let tools = cx.new(|cx| UtilityView::new(cx, store.clone(), llm_store.clone()));
        RootView {
            sidebar,
            home,
            system_settings,
            extension,
            ai,
            tools,
        }
    }
}

impl Render for RootView {
    fn render(&mut self, _w: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let route = cx.global::<HiveGuiAppState>().route;
        let colors = shell_theme_colors(cx.theme());
        let body = match route {
            AppRoute::Home => self.home.clone().into_any_element(),
            AppRoute::SystemSettings => self.system_settings.clone().into_any_element(),
            AppRoute::Extension => self.extension.clone().into_any_element(),
            AppRoute::Ai => self.ai.clone().into_any_element(),
            AppRoute::Tools => self.tools.clone().into_any_element(),
        };

        let titlebar = div()
            .id("titlebar")
            .size_full()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .px(px(8.0))
            .bg(colors.title_bar)
            .border_b_1()
            .border_color(colors.title_bar_border)
            .text_color(colors.foreground)
            .cursor(CursorStyle::OpenHand)
            .on_mouse_down(MouseButton::Left, |e, w, _| {
                if e.click_count == 2 {
                    w.zoom_window();
                } else {
                    w.start_window_move();
                }
            })
            .child(
                div().flex().flex_row().items_center().child(
                    div()
                        .text_size(px(14.0))
                        .font_weight(gpui::FontWeight::BOLD)
                        .child("HiveClaw"),
                ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .h_full()
                    .child(wbtn(
                        "―",
                        colors.secondary_hover,
                        colors.secondary_active,
                        colors.foreground,
                        colors.foreground,
                        |w, _, _| w.minimize_window(),
                    ))
                    .child(wbtn(
                        "□",
                        colors.secondary_hover,
                        colors.secondary_active,
                        colors.foreground,
                        colors.foreground,
                        |w, _, _| w.zoom_window(),
                    ))
                    .child(wbtn(
                        "✕",
                        colors.danger,
                        colors.danger_active,
                        colors.foreground,
                        colors.danger_foreground,
                        |_, _, cx| cx.quit(),
                    )),
            );

        let statusbar = div()
            .id("statusbar")
            .size_full()
            .flex()
            .items_center()
            .bg(colors.status_bar)
            .border_t_1()
            .border_color(colors.status_bar_border)
            .child(
                div()
                    .px(px(12.0))
                    .text_size(px(12.0))
                    .text_color(colors.foreground)
                    .child(format!("当前页面：{}", route.display_name())),
            );

        shell_layout(titlebar, self.sidebar.clone(), body, statusbar, colors)
    }
}

fn shell_layout(
    titlebar: impl IntoElement,
    sidebar: impl IntoElement,
    body: impl IntoElement,
    statusbar: impl IntoElement,
    colors: ShellThemeColors,
) -> gpui::Div {
    div()
        .relative()
        .size_full()
        .flex()
        .flex_col()
        .border_b_2()
        .border_color(colors.window_border)
        .bg(colors.background)
        .text_color(colors.foreground)
        .child(div().h(px(36.0)).flex_shrink_0().child(titlebar))
        .child(
            div()
                .flex()
                .flex_row()
                .flex_1()
                .min_h_0()
                .child(div().w(px(48.0)).h_full().flex_shrink_0().child(sidebar))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .child(body),
                ),
        )
        .child(div().h(px(24.0)).flex_shrink_0().child(statusbar))
}

fn wbtn(
    icon: &'static str,
    hc: Hsla,
    ac: Hsla,
    tc: Hsla,
    htc: Hsla,
    f: impl Fn(&mut Window, &gpui::MouseDownEvent, &mut gpui::App) + 'static,
) -> impl IntoElement {
    div()
        .id(SharedString::from(format!("wbtn-{icon}")))
        .w(px(46.0))
        .h(px(32.0))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(12.0))
        .text_color(tc)
        .cursor(CursorStyle::PointingHand)
        .hover(|s| s.bg(hc).text_color(htc))
        .active(|s| s.bg(ac))
        .child(icon)
        .on_mouse_down(MouseButton::Left, move |e, w, cx| f(w, e, cx))
}

#[cfg(test)]
mod tests {
    use gpui::{
        Context, IntoElement, Render, TestAppContext, VisualTestContext, Window, div, hsla,
        prelude::*, px, size,
    };
    use gpui_component::theme::Theme;

    use super::{shell_layout, shell_theme_colors};

    struct ShellLayoutTestView;

    impl Render for ShellLayoutTestView {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            let colors = shell_theme_colors(&Theme::default());
            shell_layout(
                div().size_full().debug_selector(|| "TITLEBAR".to_owned()),
                div().size_full().debug_selector(|| "SIDEBAR".to_owned()),
                div().size_full().debug_selector(|| "BODY".to_owned()),
                div().size_full().debug_selector(|| "STATUSBAR".to_owned()),
                colors,
            )
        }
    }

    #[gpui::test]
    fn main_content_stays_between_titlebar_and_statusbar(cx: &mut TestAppContext) {
        let window = cx.open_window(size(px(1200.0), px(700.0)), |_, _| ShellLayoutTestView);
        cx.run_until_parked();

        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let titlebar = cx.debug_bounds("TITLEBAR").expect("titlebar bounds");
        let sidebar = cx.debug_bounds("SIDEBAR").expect("sidebar bounds");
        let body = cx.debug_bounds("BODY").expect("body bounds");
        let statusbar = cx.debug_bounds("STATUSBAR").expect("statusbar bounds");

        assert_eq!(titlebar.size.height, px(36.0));
        assert_eq!(statusbar.size.height, px(24.0));
        assert_eq!(sidebar.size.width, px(48.0));
        assert_eq!(titlebar.bottom(), sidebar.top());
        assert_eq!(titlebar.bottom(), body.top());
        assert_eq!(sidebar.bottom(), statusbar.top());
        assert_eq!(body.bottom(), statusbar.top());
        assert_eq!(sidebar.right(), body.left());
    }

    #[test]
    fn shell_background_and_bars_follow_the_active_theme() {
        let mut theme = Theme::default();
        theme.colors.background = hsla(0.11, 0.22, 0.33, 1.0);
        theme.colors.foreground = hsla(0.44, 0.55, 0.66, 1.0);
        theme.colors.title_bar = hsla(0.12, 0.23, 0.34, 1.0);
        theme.colors.title_bar_border = hsla(0.13, 0.24, 0.35, 1.0);
        theme.colors.status_bar = hsla(0.14, 0.25, 0.36, 1.0);
        theme.colors.status_bar_border = hsla(0.15, 0.26, 0.37, 1.0);
        theme.colors.window_border = hsla(0.16, 0.27, 0.38, 1.0);

        let colors = shell_theme_colors(&theme);

        assert_eq!(colors.background, theme.background);
        assert_eq!(colors.foreground, theme.foreground);
        assert_eq!(colors.title_bar, theme.title_bar);
        assert_eq!(colors.title_bar_border, theme.title_bar_border);
        assert_eq!(colors.status_bar, theme.status_bar);
        assert_eq!(colors.status_bar_border, theme.status_bar_border);
        assert_eq!(colors.window_border, theme.window_border);
    }

    #[test]
    fn tools_route_has_a_status_bar_name() {
        assert_eq!(super::AppRoute::Tools.display_name(), "工具");
    }
}
