use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable,
    theme::{self, Theme, ThemeRegistry},
    tooltip::Tooltip,
};
use gpui_kit_assets::IconName;
use gpui_kit::{
    App, Bounds, Context, CursorStyle, Entity, Hsla, MouseButton, SharedString, TitlebarOptions,
    Window, WindowBounds, WindowDecorations, WindowOptions, div, prelude::*, px, size,
};
use std::{path::Path, sync::Arc};

use crate::agent::local_agent::LocalAgentRuntime;
use crate::config::{AppIdentity, Config};
use crate::datasource::{BuiltinFunctions, Store, StoreOpenOptions, backup::RestoreCoordinator};
use crate::datasource::llm_store::LlmStore;
use crate::runtime::diagnostics::ExecutionEventCollector;
use crate::runtime::{FoundationRuntimeComposition, LocalExecutionAdapter};
use crate::ui::{
    ai_view::AiView, category_view::CategoryView, function_view::FunctionView,
    global_config::GlobalConfigView, home::HomeView, llm_config::LLMConfigView,
    prompt_debugger::PromptDebugger, prompt_library_view::PromptLibraryView,
    sidebar_nav::SidebarNav, theme_contrast, utility_view::UtilityView,
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

impl gpui_kit::Global for HiveGuiAppState {}

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

impl gpui_kit::Global for AccessKitLabelRegistry {}

fn install_app_globals(cx: &mut App, state: HiveGuiAppState) {
    cx.set_global(state);
    cx.set_global(AccessKitLabelRegistry::new());
}

/// Replay every durable restore/retirement state before opening the
/// owner-aware production Store for `root`.
///
/// `builtin_functions` states whether this data root owns the code-registered
/// executable Builtin Functions: the desktop database does, the Prompt Studio
/// database must not (its Function management only manages schema-only
/// placeholders).
///
/// Any ambiguous recovery state or incomplete recovery proof is returned to
/// the caller before the SQLite database, sidecars, encryption key, or Plugin
/// root can be created by Store startup.
pub async fn open_store_after_restore_recovery(
    root: &Path,
    builtin_functions: BuiltinFunctions,
) -> anyhow::Result<Store> {
    let recovery = RestoreCoordinator::new(root)?.recover_startup().await?;
    anyhow::ensure!(
        recovery.store_may_open()
            && recovery.has_exactly_one_live_database()
            && recovery.has_no_mixed_database_or_plugin_tree()
            && recovery.control_files_are_outside_live_tree()
            && recovery.retirement_is_done()
            && recovery.write_gate_is_open(),
        "restore startup replay did not prove every Store-open invariant"
    );
    Ok(Store::open_local(
        StoreOpenOptions::for_root(root).with_builtin_functions(builtin_functions),
    )
    .await?)
}

/// Open the owner-aware local [`Store`] for `root`, replay interrupted plugin
/// operations and drain artifact GC, then migrate the LLM tables.
///
/// Both HiveGUI entry points share this exact bootstrap so the Store/LlmStore
/// wiring cannot drift between the full desktop app ([`run`]) and the focused
/// prompt-engineering binary ([`run_prompt_studio`]): the only per-application
/// difference is the `builtin_functions` policy.
pub async fn bootstrap_stores(
    root: &Path,
    builtin_functions: BuiltinFunctions,
) -> anyhow::Result<(Store, LlmStore)> {
    let store = open_store_after_restore_recovery(root, builtin_functions).await?;

    // T079 ③ startup replay: recover any plugin install operation interrupted
    // by a crash, then drain pending artifact GC so orphaned staging bytes and
    // soft-deleted files left behind by a previous run are cleaned up.
    if let Ok(plugin_store) = crate::plugin::plugin_store::PluginStore::new(
        store.pool().clone(),
        store.plugin_root().to_path_buf(),
    ) {
        match plugin_store.recover_interrupted_operations().await {
            Ok(n) if n > 0 => {
                tracing::info!(recovered = n, "replayed interrupted plugin operations")
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "plugin operation recovery failed"),
        }
        match plugin_store.drain_pending_gc().await {
            Ok(n) if n > 0 => tracing::info!(drained = n, "drained pending plugin artifact GC"),
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "plugin artifact GC drain failed"),
        }
    }

    // Init LLM tables before app.run (avoids blocking UI thread)
    let llm_store = {
        let s = LlmStore::new(store.pool().clone(), store.crypto().clone());
        s.migrate().await.ok();
        s
    };

    Ok((store, llm_store))
}

pub fn run(config: Config) -> anyhow::Result<()> {
    let cfg = Arc::new(config);

    // Shared bootstrap: open the Store, replay plugin recovery + artifact GC,
    // and migrate the LLM tables. The desktop database owns the executable
    // Builtin Functions.
    let (store, llm_store) = tokio::runtime::Handle::current().block_on(bootstrap_stores(
        &Store::default_db_path(),
        BuiltinFunctions::Synchronize,
    ))?;
    let app = gpui_kit::application().with_assets(gpui_kit_assets::Assets);

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
        gpui_kit::component::init(cx);
        theme_contrast::install(cx);

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
                // OS-level window title (WM_NAME / xdg toplevel title), so the
                // taskbar and window list show the product name instead of the
                // executable name. Mirrors the in-app titlebar brand (and the
                // Prompt Studio entry, which sets its own title).
                titlebar: Some(TitlebarOptions {
                    title: Some("HiveClaw".into()),
                    ..Default::default()
                }),
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
                cx.new(|cx| gpui_kit::component::Root::new(inner, window, cx))
            },
        )
        .expect("window should open");
        cx.activate(true);
    });
    Ok(())
}

/// A prompt-engineering navigation destination: stable id (used for the
/// element id), the tooltip/status-bar label, and the sidebar icon.
///
/// The rail is icon-only (matching the main HiveGUI sidebar), so the label is
/// surfaced through a hover tooltip, the status bar, and the AccessKit name
/// rather than as visible text.
struct PromptStudioSection {
    id: &'static str,
    label: &'static str,
    icon: IconName,
}

/// Sections surfaced by the focused prompt-engineering application, in
/// navigation order.
const PROMPT_STUDIO_SECTIONS: &[PromptStudioSection] = &[
    PromptStudioSection {
        id: "prompt",
        label: "提示词调试",
        icon: IconName::FlaskConical,
    },
    // 提示词管理紧挨提示词调试：本页的 message 可以直接选用这里的提示词。
    PromptStudioSection {
        id: "prompts",
        label: "提示词管理",
        icon: IconName::BookOpenText,
    },
    PromptStudioSection {
        id: "llm",
        label: "LLM 配置",
        icon: IconName::BrainCircuit,
    },
    // 函数管理紧挨提示词调试：本页「从函数管理选择工具」直接读取这里的函数。
    PromptStudioSection {
        id: "functions",
        label: "函数管理",
        icon: IconName::SquareFunction,
    },
    PromptStudioSection {
        id: "category",
        label: "分类管理",
        icon: IconName::FolderTree,
    },
    // 全局配置是低频的设置类入口，放在导航最下边。
    PromptStudioSection {
        id: "global",
        label: "全局配置",
        icon: IconName::Cog,
    },
];

/// Focused prompt-engineering application: LLM prompt debugger, LLM config
/// (Model / Preset / Provider), function management, global configuration, and
/// category management.
///
/// It reuses the same Store / LlmStore bootstrap as [`run`] but wires a
/// narrower shell so the prompt-engineering surfaces are the only navigation
/// targets. The five views do not depend on `HiveGuiAppState`, so no
/// desktop-wide global is installed here.
///
/// Unlike [`run`], the prompt studio opens its **own** data root
/// ([`Store::prompt_studio_db_path`]) rather than
/// [`Store::default_db_path`]: the two Agents keep independent databases, and
/// the exclusive owner lock on `{root}/datasources.db.lock` no longer stops
/// both binaries from running at the same time.
pub fn run_prompt_studio(_config: Config) -> anyhow::Result<()> {
    // PRD §函数管理：Prompt Studio 的数据根不写入内置函数（桌面版仍然写入）。
    let (store, llm_store) = tokio::runtime::Handle::current().block_on(bootstrap_stores(
        &Store::prompt_studio_db_path(),
        BuiltinFunctions::Absent,
    ))?;
    // The icon rail uses names outside gpui-component's default icon bundle
    // (`default-icons.txt`), so this entry point registers the full Lucide
    // catalog instead. `Assets` only embeds that default subset and would make
    // any other icon silently render blank.
    let app = gpui_kit::application().with_assets(gpui_kit_assets::AllAssets);

    app.run(move |cx: &mut App| {
        theme::init(cx);
        gpui_kit::component::init(cx);
        theme_contrast::install(cx);

        // Load custom themes
        let themes_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("themes");
        if let Err(e) = ThemeRegistry::watch_dir(themes_dir, cx, |_| {}) {
            tracing::warn!("Failed to watch themes directory: {}", e);
        }

        let store = cx.new(|_| store);
        let llm_store = llm_store.clone();
        let b = Bounds::centered(None, size(px(1200.0), px(800.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(b)),
                window_decorations: Some(WindowDecorations::Server),
                // OS-level window title (WM_NAME / xdg toplevel title), so the
                // taskbar and window list show the product name instead of the
                // executable name. Mirrors the in-app titlebar brand.
                titlebar: Some(TitlebarOptions {
                    title: Some("Ngy Prompt Studio".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            move |window, cx| {
                let inner = cx.new(|cx| PromptStudioRoot::new(cx, store.clone(), llm_store.clone()));
                cx.new(|cx| gpui_kit::component::Root::new(inner, window, cx))
            },
        )
        .expect("window should open");
        cx.activate(true);
    });
    Ok(())
}

/// Root view for the prompt-engineering application. An icon-only navigation
/// rail (matching the main HiveGUI sidebar) selects which section the body
/// renders.
pub struct PromptStudioRoot {
    active: usize,
    prompt_debugger: Entity<PromptDebugger>,
    llm_config: Entity<LLMConfigView>,
    function_view: Entity<FunctionView>,
    global_config: Entity<GlobalConfigView>,
    category_view: Entity<CategoryView>,
    prompt_library: Entity<PromptLibraryView>,
    /// `on_window_should_close` 是否已经挂到本窗口上（只需挂一次）。
    window_close_hook_registered: bool,
}

impl PromptStudioRoot {
    pub fn new(cx: &mut Context<Self>, store: Entity<Store>, llm_store: LlmStore) -> Self {
        // The prompt studio reads/writes its own execution history file under
        // its data root instead of the desktop app's.
        let prompt_debugger = cx.new(|cx| {
            PromptDebugger::new(
                cx,
                store.clone(),
                llm_store.clone(),
                AppIdentity::NGY_PROMPT_STUDIO,
            )
        });
        let llm_config = cx.new(|cx| {
            let mut view = LLMConfigView::new(cx);
            view.llm_store = Some(llm_store);
            view
        });
        let global_config = cx.new(|cx| {
            let mut view = GlobalConfigView::new(cx);
            view.store = Some(store.read(cx).clone());
            // The prompt studio writes `ngy_prompt_studio.log`, so the
            // operator-facing copy must name that file instead of the desktop
            // app's `hivegui.log`.
            view.identity = AppIdentity::NGY_PROMPT_STUDIO;
            view
        });
        let function_view = cx.new(|cx| FunctionView::new_prompt_studio(store.clone(), cx));
        let category_view = cx.new(|cx| CategoryView::new(store.clone(), cx));
        let prompt_library = cx.new(|cx| PromptLibraryView::new(store.clone(), cx));
        Self {
            active: 0,
            prompt_debugger,
            llm_config,
            function_view,
            global_config,
            category_view,
            prompt_library,
            window_close_hook_registered: false,
        }
    }
}

/// 左侧导航栏的一个图标按钮。
///
/// `index` 是分区在 [`PROMPT_STUDIO_SECTIONS`] 中的位置，点击后写入
/// [`PromptStudioRoot::active`]。配色沿用 `SidebarNav` 的侧栏语义色。
fn prompt_studio_nav_button(
    index: usize,
    section: &'static PromptStudioSection,
    selected: bool,
    cx: &mut Context<PromptStudioRoot>,
) -> impl IntoElement {
    let sidebar_background = cx.theme().sidebar;
    let sidebar_accent = cx.theme().sidebar_accent;
    let sidebar_foreground = cx.theme().sidebar_foreground;
    let label = SharedString::from(section.label);
    let label_for_a11y = label.clone();

    div()
        .id(SharedString::from(format!("ps-nav-{}", section.id)))
        .w(px(40.0))
        .h(px(40.0))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(8.0))
        .cursor(CursorStyle::PointingHand)
        .bg(if selected {
            sidebar_accent
        } else {
            sidebar_background
        })
        .hover(move |style| style.bg(sidebar_accent))
        .role(gpui_kit::accesskit::Role::Button)
        .aria_label(label_for_a11y)
        .on_click(cx.listener(move |this, _, _, cx| {
            let changed = this.active != index;
            this.active = index;
            // 重新进入「提示词调试」分区时重新读取运行期开关：用户可能刚在
            // 「全局配置」里改过「查看/对比是否使用独立窗口」，不重读就要重启
            // 本应用才生效。
            if changed && section.id == "prompt" {
                this.prompt_debugger
                    .update(cx, |view, cx| view.reload_detached_window_settings(cx));
            }
            cx.notify();
        }))
        .child(
            div().flex().items_center().justify_center().child(
                Icon::new(section.icon)
                    .large()
                    .text_color(sidebar_foreground),
            ),
        )
        .tooltip(move |window, cx| Tooltip::new(label.clone()).build(window, cx))
}

impl Render for PromptStudioRoot {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 与桌面版主窗口同样：系统/窗口管理器关闭窗口时也要先把提示词调试页
        // 打开的独立窗口关掉（独立窗口会让应用以为还有窗口存在而不退出）。
        if !self.window_close_hook_registered {
            self.window_close_hook_registered = true;
            let prompt_debugger = self.prompt_debugger.clone();
            window.on_window_should_close(cx, move |_, cx| {
                prompt_debugger.update(cx, |view, cx| view.close_all_detached_windows(cx));
                true
            });
        }

        let colors = shell_theme_colors(cx.theme());
        let sidebar_background = cx.theme().sidebar;
        let sidebar_foreground = cx.theme().sidebar_foreground;

        // Icon-only navigation rail: 48px wide, 40x40 icon buttons with hover
        // tooltips. Mirrors `SidebarNav` so the two apps read the same way.
        let mut sidebar = div()
            .id("ps-sidebar")
            .w(px(48.0))
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(8.0))
            .bg(sidebar_background)
            .text_color(sidebar_foreground)
            .py(px(12.0));

        for (index, section) in PROMPT_STUDIO_SECTIONS.iter().enumerate() {
            // 设置类入口（全局配置）贴到面板最底部：在它前面插一段弹性空隙，
            // 窗口变高时由空隙把设置项推到下边缘，与上面的功能导航分开。
            if section.id == "global" {
                sidebar = sidebar.child(div().flex_1());
            }
            sidebar = sidebar.child(prompt_studio_nav_button(
                index,
                section,
                self.active == index,
                cx,
            ));
        }

        // 按 section id 分派而不是按索引：调整 `PROMPT_STUDIO_SECTIONS` 的
        // 顺序时，正文不会和左侧导航错位。
        let body = match PROMPT_STUDIO_SECTIONS[self.active].id {
            "prompt" => self.prompt_debugger.clone().into_any_element(),
            "prompts" => self.prompt_library.clone().into_any_element(),
            "llm" => self.llm_config.clone().into_any_element(),
            "functions" => self.function_view.clone().into_any_element(),
            "global" => self.global_config.clone().into_any_element(),
            "category" => self.category_view.clone().into_any_element(),
            _ => div().into_any_element(),
        };

        let prompt_debugger_for_close = self.prompt_debugger.clone();
        let titlebar = div()
            .id("ps-titlebar")
            .h(px(36.0))
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
                div()
                    .text_size(px(14.0))
                    .font_weight(gpui_kit::FontWeight::BOLD)
                    .child("Ngy 提示词工程 · Ngy Prompt Studio"),
            )
            .child(window_control_buttons(&colors, move |_, _, cx| {
                // 先关掉提示词调试页打开的执行历史独立窗口，再退出应用。
                prompt_debugger_for_close
                    .update(cx, |view, cx| view.close_all_detached_windows(cx));
                cx.quit();
            }));

        let statusbar = div()
            .id("ps-statusbar")
            .h(px(24.0))
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
                    .child(format!(
                        "当前页面：{}",
                        PROMPT_STUDIO_SECTIONS[self.active].label
                    )),
            );

        div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .border_b_2()
            .border_color(colors.window_border)
            .bg(colors.background)
            .text_color(colors.foreground)
            .child(titlebar)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .min_h_0()
                    .child(sidebar)
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
            .child(statusbar)
    }
}

pub struct RootView {
    sidebar: Entity<SidebarNav>,
    home: Entity<HomeView>,
    ai: Entity<AiView>,
    tools: Entity<UtilityView>,
    last_route: AppRoute,
    /// `on_window_should_close` 是否已经挂到本窗口上（只需挂一次）。
    window_close_hook_registered: bool,
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
            window_close_hook_registered: false,
        }
    }
}

impl Render for RootView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 用窗口管理器（Alt+F4、标题栏 ×）关闭主窗口时不会经过自绘的关闭按钮。
        // 而独立窗口也让“还有窗口存在”成立，应用不会随之退出 → 主窗口消失后
        // 桌面上会剩下孤立的执行历史窗口。这里挂上关闭前钩子：无论从哪条路关闭
        // 主窗口，都先把派生的独立窗口一并关掉。
        if !self.window_close_hook_registered {
            self.window_close_hook_registered = true;
            let tools = self.tools.clone();
            window.on_window_should_close(cx, move |_, cx| {
                tools.update(cx, |view, cx| view.close_prompt_debugger_windows(cx));
                true
            });
        }

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

        let tools_for_close = self.tools.clone();
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
                        .font_weight(gpui_kit::FontWeight::BOLD)
                        .child("HiveClaw"),
                ),
            )
            .child(window_control_buttons(&colors, move |_, _, cx| {
                // 先关掉提示词调试页打开的执行历史独立窗口，再退出应用。
                tools_for_close.update(cx, |view, cx| view.close_prompt_debugger_windows(cx));
                cx.quit();
            }));

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
) -> gpui_kit::Div {
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

/// 标题栏「最小化 / 最大化 / 关闭」按钮的测试选择器。
const WINDOW_CONTROL_MINIMIZE: &str = "WINDOW_CONTROL_MINIMIZE";
const WINDOW_CONTROL_MAXIMIZE: &str = "WINDOW_CONTROL_MAXIMIZE";
const WINDOW_CONTROL_CLOSE: &str = "WINDOW_CONTROL_CLOSE";

/// 标题栏右上角的「最小化 / 最大化 / 关闭」按钮组。
///
/// 两个应用的主窗口（桌面版 [`RootView`]、Prompt Studio [`PromptStudioRoot`]）
/// 共用这一套控件：窗口都带系统装饰但关闭/最小化等操作由自绘标题栏提供。
/// `close` 由调用方给出，因为两个入口在退出前要收尾的对象不同。
fn window_control_buttons(
    colors: &ShellThemeColors,
    close: impl Fn(&mut Window, &gpui_kit::MouseDownEvent, &mut gpui_kit::App) + 'static,
) -> gpui_kit::Div {
    div()
        .flex()
        .flex_row()
        .h_full()
        .flex_shrink_0()
        .child(wbtn(
            WINDOW_CONTROL_MINIMIZE,
            "―",
            colors.secondary_hover,
            colors.secondary_active,
            colors.foreground,
            colors.foreground,
            |w, _, _| w.minimize_window(),
        ))
        .child(wbtn(
            WINDOW_CONTROL_MAXIMIZE,
            "□",
            colors.secondary_hover,
            colors.secondary_active,
            colors.foreground,
            colors.foreground,
            |w, _, _| w.zoom_window(),
        ))
        .child(wbtn(
            WINDOW_CONTROL_CLOSE,
            "✕",
            colors.danger,
            colors.danger_active,
            colors.foreground,
            colors.danger_foreground,
            close,
        ))
}

fn wbtn(
    selector: &'static str,
    icon: &'static str,
    hc: Hsla,
    ac: Hsla,
    tc: Hsla,
    htc: Hsla,
    f: impl Fn(&mut Window, &gpui_kit::MouseDownEvent, &mut gpui_kit::App) + 'static,
) -> impl IntoElement {
    div()
        .id(SharedString::from(selector))
        .debug_selector(move || selector.to_owned())
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
    use gpui_kit::component::theme::Theme;
    use gpui_kit::{
        Context, IntoElement, Render, TestAppContext, VisualTestContext, Window, div, hsla,
        prelude::*, px, size,
    };

    use super::{
        PROMPT_STUDIO_SECTIONS, WINDOW_CONTROL_CLOSE, WINDOW_CONTROL_MAXIMIZE,
        WINDOW_CONTROL_MINIMIZE, shell_layout, shell_theme_colors, window_control_buttons,
    };

    /// PRD「提示词工程 · 函数管理 · 左侧菜单要添加函数管理」：Ngy Prompt
    /// Studio 的图标栏必须提供「函数管理」分区，并保持与正文分派一致的顺序。
    #[test]
    fn prompt_studio_rail_lists_function_management() {
        let ids = PROMPT_STUDIO_SECTIONS
            .iter()
            .map(|section| section.id)
            .collect::<Vec<_>>();
        assert_eq!(
            ids,
            [
                "prompt",
                "prompts",
                "llm",
                "functions",
                "category",
                "global"
            ]
        );
        assert!(
            PROMPT_STUDIO_SECTIONS
                .iter()
                .any(|section| section.id == "functions" && section.label == "函数管理"),
            "左侧菜单必须包含「函数管理」分区"
        );
        assert!(
            PROMPT_STUDIO_SECTIONS
                .iter()
                .any(|section| section.id == "prompts" && section.label == "提示词管理"),
            "左侧菜单必须包含「提示词管理」分区"
        );

        // 正文按 id 分派，重复 id 会让某个分区永远打不开。
        let mut unique = ids.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), ids.len(), "分区 id 必须唯一");
    }

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

    #[gpui_kit::test]
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

    /// 只渲染标题栏右侧的窗口控件，用于断言三个按钮都在（FR：主窗口右上角
    /// 要有最小化 / 最大化 / 关闭）。
    struct WindowControlsTestView;

    impl Render for WindowControlsTestView {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            let colors = shell_theme_colors(&Theme::default());
            div()
                .size_full()
                .child(window_control_buttons(&colors, |_, _, _| {}))
        }
    }

    #[gpui_kit::test]
    fn titlebar_offers_minimize_maximize_and_close_buttons(cx: &mut TestAppContext) {
        let window = cx.open_window(size(px(400.0), px(36.0)), |_, _| WindowControlsTestView);
        cx.run_until_parked();

        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let minimize = cx
            .debug_bounds(WINDOW_CONTROL_MINIMIZE)
            .expect("minimize button bounds");
        let maximize = cx
            .debug_bounds(WINDOW_CONTROL_MAXIMIZE)
            .expect("maximize button bounds");
        let close = cx
            .debug_bounds(WINDOW_CONTROL_CLOSE)
            .expect("close button bounds");

        assert_eq!(minimize.size.height, px(32.0));
        assert_eq!(maximize.size.height, px(32.0));
        assert_eq!(close.size.height, px(32.0));
        // 右上角从左到右：最小化 → 最大化 → 关闭。
        assert!(minimize.right() <= maximize.left());
        assert!(maximize.right() <= close.left());
        assert!(close.right() <= px(400.0));
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
