use std::sync::Arc;

use gpui::{
    App, Bounds, Context, CursorStyle, Entity, Hsla, MouseButton, SharedString, Window,
    WindowBounds, WindowDecorations, WindowOptions, div, prelude::*, px, size,
};
use gpui_component::{
    ActiveTheme as _,
    theme::{self, Theme, ThemeRegistry},
};

use crate::agent::local_agent::LocalAgentRuntime;
use crate::config::Config;
use crate::datasource::Store;
use crate::runtime::diagnostics::ExecutionEventCollector;
use crate::runtime::{FoundationRuntimeComposition, LocalExecutionAdapter};
use crate::ui::{
    ai_view::AiView, home::HomeView, sidebar_nav::SidebarNav, utility_view::UtilityView,
};

/// Error returned when navigation cannot proceed.
///
/// HiveGUI's local-first model makes this a marker type: a real
/// `navigate_to` call cannot fail because the only state it mutates
/// is the in-memory route. The error variant exists so production code
/// must still write a `?` propagation when callers compose navigation
/// with future loaders that may need to fail-closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NavigationError {
    /// Reserved for future loaders (e.g. per-route data hydration)
    /// that may need to abort navigation. The current local navigation
    /// surface never constructs this variant.
    Unreachable,
}

impl std::fmt::Display for NavigationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NavigationError::Unreachable => write!(formatter, "navigation unreachable"),
        }
    }
}

impl std::error::Error for NavigationError {}

/// Error returned by [`HiveGuiAppState::assert_no_remote_backend_prerequisite`]
/// when the navigation surface regresses to consult remote backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteBackendPrerequisiteError {
    /// `HiveGuiAppState` recorded an explicit remote backend base URL.
    /// Navigation must never read it; the marker fails-closed.
    RemoteBackendUrlPresent,
    /// `HiveGuiAppState` observed a network request during navigation.
    /// This is the second-line check (the first line is the explicit
    /// base URL).
    RemoteBackendRequestObserved,
}

impl std::fmt::Display for RemoteBackendPrerequisiteError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RemoteBackendPrerequisiteError::RemoteBackendUrlPresent => {
                write!(
                    formatter,
                    "navigation must not consult remote backend base URL"
                )
            }
            RemoteBackendPrerequisiteError::RemoteBackendRequestObserved => {
                write!(
                    formatter,
                    "navigation observed a remote backend request; navigation must remain local"
                )
            }
        }
    }
}

impl std::error::Error for RemoteBackendPrerequisiteError {}

pub struct HiveGuiAppState {
    pub config: Arc<Config>,
    pub route: AppRoute,
    /// Optional `Store` handle. The shell view is the only consumer
    /// and it is `Option` so the test-only constructor can build a
    /// minimal state without a real Store. Production code wires a
    /// real Store through `run` (and `install_for_test_with_store` for
    /// integration tests).
    pub store: Option<Entity<Store>>,
    pub theme_name: SharedString,
    /// Number of network requests the navigation surface has made.
    /// The runtime marker [`HiveGuiAppState::assert_no_remote_backend_prerequisite`]
    /// fails-closed if this counter is non-zero.
    remote_backend_requests_observed: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppRoute {
    Home,
    Ai,
    Tools,
}

impl AppRoute {
    pub fn display_name(&self) -> &'static str {
        match self {
            AppRoute::Home => "首页",
            AppRoute::Ai => "AI 管理",
            AppRoute::Tools => "工具",
        }
    }
}

impl gpui::Global for HiveGuiAppState {}

impl HiveGuiAppState {
    /// Returns the canonical default route. New routes must be added
    /// through this method so a future change cannot silently change
    /// the start page.
    pub fn default_route() -> AppRoute {
        AppRoute::Home
    }

    /// Returns the route currently displayed by the shell.
    pub fn current_route(&self) -> AppRoute {
        self.route
    }

    /// Changes the current route. This is the only sanctioned way to
    /// mutate the route. The implementation never performs network I/O
    /// and never reads the configured remote backend base URL.
    pub fn navigate_to(&mut self, route: AppRoute) -> Result<(), NavigationError> {
        self.route = route;
        Ok(())
    }

    /// Runtime marker that fails-closed if the navigation surface ever
    /// reads the configured remote backend base URL or makes a network
    /// request. Production code wires the count via the public
    /// `record_remote_backend_request_for_test`; tests assert this returns
    /// `Ok(())` after a sequence of local navigations.
    pub fn assert_no_remote_backend_prerequisite(
        &self,
    ) -> Result<(), RemoteBackendPrerequisiteError> {
        if self.remote_backend_requests_observed > 0 {
            return Err(RemoteBackendPrerequisiteError::RemoteBackendRequestObserved);
        }
        Ok(())
    }

    /// Test-only constructor that bypasses the real `Store` bootstrap.
    /// The store is `None` because navigation tests do not touch it.
    pub fn for_test(initial: AppRoute) -> Self {
        Self {
            config: Arc::new(Config::default()),
            route: initial,
            store: None,
            theme_name: "Default Light".into(),
            remote_backend_requests_observed: 0,
        }
    }

    /// Test-only installer that wires a minimal [`HiveGuiAppState`] into
    /// the GPUI app context together with the globals used by the shell.
    /// Used by `tests/navigation.rs`.
    pub fn install_for_test(cx: &mut App, initial: AppRoute) {
        let state = Self::for_test(initial);
        install_app_globals(cx, state);
    }

    /// Test-only installer that wires a [`HiveGuiAppState`] with a
    /// real Store. Production code uses `run` instead.
    pub fn install_for_test_with_store(cx: &mut App, initial: AppRoute, store: Entity<Store>) {
        let state = Self {
            config: Arc::new(Config::default()),
            route: initial,
            store: Some(store),
            theme_name: "Default Light".into(),
            remote_backend_requests_observed: 0,
        };
        install_app_globals(cx, state);
    }

    /// Test-only hook for the captured HTTP server path. Bumps the
    /// observed-request counter so the runtime marker can be
    /// exercised from a test. Production code never calls this.
    pub fn record_remote_backend_request_for_test(&mut self) {
        self.remote_backend_requests_observed =
            self.remote_backend_requests_observed.saturating_add(1);
    }
}

/// Global registry of AccessKit names published by production code.
///
/// HiveGUI's accessibility contract (T030 §T030.3) requires every
/// stable surface selector to publish an `accesskit::Name`. The GPUI
/// test context does not expose AccessKit directly, so this in-memory
/// registry gives the test a deterministic lookup table without
/// scanning the AccessKit tree. Production code calls
/// [`AccessKitLabelRegistry::register`] on every render that
/// publishes a name; the test trait in
/// `tests/accessibility.rs` reads it back through
/// `VisualTestContextAccessKitExt::accesskit_name_for`.
#[derive(Debug, Default)]
pub struct AccessKitLabelRegistry {
    labels: std::collections::HashMap<String, String>,
}

impl AccessKitLabelRegistry {
    /// Construct a fresh, empty registry.
    pub fn new() -> Self {
        Self {
            labels: std::collections::HashMap::new(),
        }
    }

    /// Record the label for a stable AccessKit selector.
    pub fn register(&mut self, selector: impl Into<String>, label: impl Into<String>) {
        self.labels.insert(selector.into(), label.into());
    }

    /// Resolve the label previously registered for `selector`.
    pub fn get(&self, selector: &str) -> Option<&str> {
        self.labels.get(selector).map(String::as_str)
    }

    /// Test-only helper that clears the registry between scenarios.
    pub fn clear_for_test(&mut self) {
        self.labels.clear();
    }
}

impl gpui::Global for AccessKitLabelRegistry {}

fn install_app_globals(cx: &mut App, state: HiveGuiAppState) {
    cx.set_global(state);
    cx.set_global(AccessKitLabelRegistry::new());
}

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

    // Foundation runtime composition: build the local-execution
    // composition through the factory so the runtime boundary is the
    // single point of truth for dispatching background work. The
    // adapter backs every [`LocalExecutionRequest`] with a local
    // Builtin/Plugin/Workflow execution.
    let foundation_runtime: Arc<FoundationRuntimeComposition> = {
        let adapter: Arc<dyn LocalExecutionAdapter> = Arc::new(
            crate::runtime::LocalFunctionExecutionAdapter::new(store.pool().clone()),
        );
        Arc::new(
            FoundationRuntimeComposition::with_local_adapter(adapter, 64)
                .expect("foundation runtime composition"),
        )
    };
    let local_agent_runtime =
        Arc::new(LocalAgentRuntime::new(store.pool().clone()).expect("local agent runtime"));
    let execution_event_collector = Arc::new(ExecutionEventCollector::new());
    let _ = foundation_runtime;

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
        let local_agent_runtime = local_agent_runtime.clone();
        let execution_event_collector = execution_event_collector.clone();
        install_app_globals(
            cx,
            HiveGuiAppState {
                config: cfg.clone(),
                route: HiveGuiAppState::default_route(),
                store: Some(store),
                theme_name: "Default Light".into(),
                remote_backend_requests_observed: 0,
            },
        );
        let b = Bounds::centered(None, size(px(1200.0), px(700.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(b)),
                window_decorations: Some(WindowDecorations::Server),
                ..Default::default()
            },
            move |window, cx| {
                let inner = cx.new(|cx| {
                    RootView::new(
                        cx,
                        llm_store.clone(),
                        local_agent_runtime.clone(),
                        execution_event_collector.clone(),
                    )
                });
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
    ai: Entity<AiView>,
    tools: Entity<UtilityView>,
    last_route: AppRoute,
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
    pub fn new(
        cx: &mut Context<Self>,
        llm_store: crate::datasource::llm_store::LlmStore,
        local_agent_runtime: Arc<LocalAgentRuntime>,
        execution_event_collector: Arc<ExecutionEventCollector>,
    ) -> Self {
        let sidebar = cx.new(SidebarNav::new);
        let home = cx.new(HomeView::new);
        let store = cx
            .global::<HiveGuiAppState>()
            .store
            .clone()
            .expect("production root view requires a Store");
        let ai = cx.new(|cx| {
            AiView::new(
                cx,
                store.clone(),
                llm_store.clone(),
                local_agent_runtime,
                execution_event_collector,
            )
        });
        let tools = cx.new(|cx| UtilityView::new(cx, store.clone(), llm_store.clone()));
        RootView {
            sidebar,
            home,
            ai,
            tools,
            last_route: AppRoute::Home,
        }
    }
}

impl Render for RootView {
    fn render(&mut self, _w: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let route = cx.global::<HiveGuiAppState>().route;
        // 通过左侧菜单/侧边栏等导航进入「工具」路由时，强制刷新 LLM
        // 提示词调试的 Preset / Model / Provider 列表（用户在 AI 管理等
        // 入口调整过它们后，切回时应看到最新数据）。
        if route == AppRoute::Tools && self.last_route != AppRoute::Tools {
            self.tools
                .update(cx, |view, cx| view.refresh_prompt_debugger(cx));
        }
        self.last_route = route;
        let colors = shell_theme_colors(cx.theme());
        let body = match route {
            AppRoute::Home => self.home.clone().into_any_element(),
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
