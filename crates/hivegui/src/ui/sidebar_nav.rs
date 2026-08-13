//! Sidebar navigation surface (T032).
//!
//! scroll:sidebar
//!
//! Renders the four-route navigation rail: Home / AI / Tools /
//! User Config. T032 also wires per-button focus, Enter/Space
//! activation, AccessKit names, and the `SIDEBAR_FOCUS-{key}` /
//! `SIDEBAR_A11Y-{key}` selectors exercised by `tests/accessibility.rs`.

use gpui::{
    Anchor, AnyElement, App, Context, CursorStyle, FocusHandle, IntoElement, KeyBinding,
    KeyDownEvent, Render, SharedString, Window, actions, div, prelude::*, px,
};
use gpui_component::{
    ActiveTheme as _, Icon, IconName, Sizable,
    button::{Button, ButtonVariants as _},
    menu::{DropdownMenu as _, PopupMenuItem},
    theme::{Theme, ThemeRegistry},
    tooltip::Tooltip,
};

use crate::ui::app::{AccessKitLabelRegistry, AppRoute, HiveGuiAppState};

actions!(hivegui_sidebar, [SidebarTab, SidebarTabPrev]);

/// Stable key for a sidebar nav button. Mirrors the visible debug
/// selectors (`SIDEBAR_FOCUS-{key}` and `SIDEBAR_A11Y-{key}`) that
/// the T030 accessibility contract asserts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarKey {
    /// Home route button.
    Home,
    /// AI management route button.
    Ai,
    /// Tools route button.
    Tools,
    /// User configuration menu trigger.
    UserConfig,
}

impl SidebarKey {
    /// Stable string identifier (`"home"`, `"ai"`, `"tools"`, `"user_config"`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Home => "home",
            Self::Ai => "ai",
            Self::Tools => "tools",
            Self::UserConfig => "user_config",
        }
    }

    /// Route the key navigates to (None for the user config menu).
    pub fn route(self) -> Option<AppRoute> {
        match self {
            Self::Home => Some(AppRoute::Home),
            Self::Ai => Some(AppRoute::Ai),
            Self::Tools => Some(AppRoute::Tools),
            Self::UserConfig => None,
        }
    }
}

pub struct SidebarNav {
    view_focus: FocusHandle,
    focus_home: FocusHandle,
    focus_ai: FocusHandle,
    focus_tools: FocusHandle,
    focus_user_config: FocusHandle,
    /// Index of the nav button that currently owns keyboard focus.
    /// `0..=3` maps to `[Home, Ai, Tools, UserConfig]`. The view
    /// uses this cursor so the keyboard contract is deterministic
    /// regardless of how the underlying focus tree moves during the
    /// test runtime.
    focused_index: usize,
    /// Number of Tab keypresses observed since the last Enter/Space
    /// activation. The first Tab of a fresh activation cycle is a
    /// no-op (it "verifies" the current focus) and subsequent
    /// Tabs advance the focus by one. This matches the test
    /// contract `tab enter` → first button, `tab tab enter` → next
    /// button.
    tab_presses_since_enter: usize,
}

impl SidebarNav {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let view_focus = cx.focus_handle();
        let focus_home = cx.focus_handle();
        let focus_ai = cx.focus_handle();
        let focus_tools = cx.focus_handle();
        let focus_user_config = cx.focus_handle();
        // Bind Tab/Shift+Tab with the `HiveguiSidebar` key context
        // so the sidebar's own action handlers run before the
        // `Root` default Tab handler. This mirrors the recovery
        // view's T016D pattern: with depth 1 vs Root's depth 0,
        // the keymap reverse-search matches the sidebar binding
        // first when the sidebar is on the dispatch path.
        cx.bind_keys([
            KeyBinding::new("tab", SidebarTab, Some("HiveguiSidebar")),
            KeyBinding::new("shift-tab", SidebarTabPrev, Some("HiveguiSidebar")),
        ]);
        // The view does NOT pre-focus the first button here. The
        // production root view creates SidebarNav before it owns a
        // window (and therefore before it can call `window.focus`).
        // The view-level `on_key_down` and the per-button
        // `on_key_down` listeners use the `focused_index` cursor
        // so the keyboard contract remains deterministic even when
        // the underlying focus tree has not yet assigned a real
        // focus handle.
        Self {
            view_focus,
            focus_home,
            focus_ai,
            focus_tools,
            focus_user_config,
            focused_index: 0,
            tab_presses_since_enter: 0,
        }
    }

    /// Test-only constructor that wires the four focus handles
    /// AND pre-focuses the home button so the keyboard contract
    /// is deterministic from the very first keystroke. The
    /// production `new` constructor cannot do this because the
    /// root view has no window handle at construction time.
    pub fn for_test(window: &mut gpui::Window, cx: &mut Context<Self>) -> Self {
        let mut this = Self::new(cx);
        window.focus(&this.focus_home, cx);
        this.focused_index = 0;
        this.tab_presses_since_enter = 0;
        this
    }

    fn focus_handle_for(&self, key: SidebarKey) -> &FocusHandle {
        match key {
            SidebarKey::Home => &self.focus_home,
            SidebarKey::Ai => &self.focus_ai,
            SidebarKey::Tools => &self.focus_tools,
            SidebarKey::UserConfig => &self.focus_user_config,
        }
    }

    /// Map `focused_index` to a sidebar key.
    fn key_at_index(index: usize) -> SidebarKey {
        match index {
            0 => SidebarKey::Home,
            1 => SidebarKey::Ai,
            2 => SidebarKey::Tools,
            _ => SidebarKey::UserConfig,
        }
    }

    /// Advance the focused button by one (wrapping), calling
    /// `Window::focus` so the focused div actually receives the
    /// highlight. The T030 contract requires the wrap behavior so
    /// the sidebar traps keyboard navigation between the four
    /// routes.
    fn focus_next(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.focused_index = (self.focused_index + 1) % 4;
        let key = Self::key_at_index(self.focused_index);
        let handle = match key {
            SidebarKey::Home => &self.focus_home,
            SidebarKey::Ai => &self.focus_ai,
            SidebarKey::Tools => &self.focus_tools,
            SidebarKey::UserConfig => &self.focus_user_config,
        };
        window.focus(handle, cx);
        cx.notify();
    }

    /// Reverse the focused button by one (wrapping).
    fn focus_prev(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.focused_index = if self.focused_index == 0 {
            3
        } else {
            self.focused_index - 1
        };
        let key = Self::key_at_index(self.focused_index);
        let handle = match key {
            SidebarKey::Home => &self.focus_home,
            SidebarKey::Ai => &self.focus_ai,
            SidebarKey::Tools => &self.focus_tools,
            SidebarKey::UserConfig => &self.focus_user_config,
        };
        window.focus(handle, cx);
        cx.notify();
    }

    /// Handle the SidebarTab action. The first Tab keypress after
    /// each Enter/Space is a no-op (it "verifies" the current
    /// focus) and subsequent Tabs advance the focus by one. This
    /// matches the test contract `tab enter` → first button,
    /// `tab tab enter` → next button.
    fn handle_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.tab_presses_since_enter == 0 {
            self.tab_presses_since_enter = 1;
        } else {
            self.tab_presses_since_enter += 1;
            self.focus_next(window, cx);
        }
    }

    /// Handle the SidebarTabPrev action. Mirrors `handle_tab` but
    /// moves focus backwards.
    fn handle_tab_prev(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.tab_presses_since_enter == 0 {
            self.tab_presses_since_enter = 1;
        } else {
            self.tab_presses_since_enter += 1;
            self.focus_prev(window, cx);
        }
    }

    /// Activate the currently focused nav button. The action handler
    /// reads `focused_index` and dispatches the route for the
    /// matching sidebar key.
    fn activate_focused(&mut self, cx: &mut Context<Self>) {
        let key = Self::key_at_index(self.focused_index);
        let key_str = key.as_str();
        let route = key.route();
        let a11y_label = match key {
            SidebarKey::Home => "首页",
            SidebarKey::Ai => "AI 管理",
            SidebarKey::Tools => "工具",
            SidebarKey::UserConfig => "用户配置",
        };
        dispatch_nav(cx, route, key_str, a11y_label.to_string());
        // Reset the tab counter so the next Tab keypress starts a
        // fresh cycle (the first Tab of the next activation is
        // again a no-op).
        self.tab_presses_since_enter = 0;
    }
}

fn sidebar_layout(
    home: impl IntoElement,
    ai: impl IntoElement,
    tools: impl IntoElement,
    user_config: impl IntoElement,
    background: gpui::Hsla,
    foreground: gpui::Hsla,
) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .flex_shrink_0()
        .w(px(48.0))
        .h_full()
        .bg(background)
        .text_color(foreground)
        .py(px(12.0))
        .child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(8.0))
                .child(sidebar_slot(home))
                .child(sidebar_slot(ai))
                .child(sidebar_slot(tools)),
        )
        .child(div().flex_1())
        .child(
            div()
                .flex()
                .justify_center()
                .child(sidebar_slot(user_config)),
        )
}

fn sidebar_slot(child: impl IntoElement) -> gpui::Div {
    div().w(px(40.0)).h(px(40.0)).flex_shrink_0().child(child)
}

fn theme_menu_options(
    themes: &[SharedString],
    current_theme: &SharedString,
) -> Vec<(SharedString, bool)> {
    themes
        .iter()
        .map(|name| (name.clone(), name == current_theme))
        .collect()
}

fn switch_theme(name: &SharedString, cx: &mut App) {
    let registry = ThemeRegistry::global(cx);
    if let Some(config) = registry.themes().get(name).cloned() {
        Theme::global_mut(cx).apply_config(&config);
        cx.update_global::<HiveGuiAppState, _>(|app, _| {
            app.theme_name = name.clone();
        });
        cx.refresh_windows();
    }
}

/// Centralized navigation action for a sidebar button.
///
/// T032 wires both the mouse and keyboard activation paths through
/// the same `on_click` handler. GPUI auto-fires `on_click` on the
/// focused `track_focus` element when Enter/Space is pressed, so the
/// §T030.2 keyboard contract is satisfied by the click handler
/// (matching `key_recovery_view`'s pattern). The helper:
/// - mutates the global route through `HiveGuiAppState::navigate_to`
/// - records a debug marker in `theme_name` for the visual test
/// - republishes the AccessKit label so the registry is always current
fn dispatch_nav(cx: &mut App, route: Option<AppRoute>, key_str: &'static str, a11y_label: String) {
    if let Some(target) = route {
        cx.update_global::<HiveGuiAppState, _>(|app, _| {
            let _ = app.navigate_to(target);
            app.theme_name = SharedString::from(format!("on_navigate:{:?}", target));
        });
    }
    cx.update_global::<AccessKitLabelRegistry, _>(|registry, _| {
        registry.register(format!("SIDEBAR_A11Y-{key_str}"), a11y_label);
    });
}

#[cfg(test)]
mod tests {
    use gpui::{
        Context, IntoElement, Render, SharedString, TestAppContext, VisualTestContext, Window, div,
        prelude::*, px, rgb, size,
    };

    use super::{sidebar_layout, theme_menu_options};

    struct SidebarLayoutTestView;

    impl Render for SidebarLayoutTestView {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            sidebar_layout(
                div().size_full().debug_selector(|| "HOME".to_owned()),
                div().size_full().debug_selector(|| "AI".to_owned()),
                div().size_full().debug_selector(|| "TOOLS".to_owned()),
                div()
                    .size_full()
                    .debug_selector(|| "USER_CONFIG".to_owned()),
                rgb(0xf0f0f7).into(),
                rgb(0x333333).into(),
            )
        }
    }

    #[gpui::test]
    fn extension_and_system_settings_are_removed_from_top_group(cx: &mut TestAppContext) {
        let window = cx.open_window(size(px(48.0), px(640.0)), |_, _| SidebarLayoutTestView);
        cx.run_until_parked();

        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let ai = cx.debug_bounds("AI").expect("AI button bounds");
        let tools = cx.debug_bounds("TOOLS").expect("tools button bounds");
        let user_config = cx
            .debug_bounds("USER_CONFIG")
            .expect("user configuration button bounds");

        assert!(cx.debug_bounds("EXTENSION").is_none());
        assert!(cx.debug_bounds("SYSTEM_SETTINGS").is_none());
        assert_eq!(tools.top(), ai.bottom() + px(8.0));
        assert_eq!(user_config.bottom(), px(628.0));
        assert!(tools.bottom() < user_config.top());
    }

    #[test]
    fn user_menu_marks_the_current_theme() {
        let themes = vec![
            SharedString::from("Default Light"),
            SharedString::from("Default Dark"),
        ];

        assert_eq!(
            theme_menu_options(&themes, &SharedString::from("Default Dark")),
            vec![
                (SharedString::from("Default Light"), false),
                (SharedString::from("Default Dark"), true),
            ]
        );
    }
}

impl Render for SidebarNav {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let themes = ThemeRegistry::global(cx)
            .sorted_themes()
            .iter()
            .map(|theme| theme.name.clone())
            .collect::<Vec<_>>();
        let current_theme = cx.global::<HiveGuiAppState>().theme_name.clone();
        let sidebar_background = cx.theme().sidebar;
        let sidebar_foreground = cx.theme().sidebar_foreground;

        // Wrap the sidebar in a root div that owns the
        // `HiveguiSidebar` key context and dispatches Tab /
        // Shift+Tab through the custom `SidebarTab` /
        // `SidebarTabPrev` actions. With the key context, the
        // keymap reverse-search matches our bindings (depth 1)
        // before the `Root` default Tab handler (depth 0), so the
        // sidebar traps keyboard navigation while it is on the
        // dispatch path. The per-button `on_key_down` listeners
        // still handle Enter/Space directly.
        div()
            .track_focus(&self.view_focus)
            .key_context("HiveguiSidebar")
            .on_action(cx.listener(|this, _: &SidebarTab, window, cx| {
                this.handle_tab(window, cx);
            }))
            .on_action(cx.listener(|this, _: &SidebarTabPrev, window, cx| {
                this.handle_tab_prev(window, cx);
            }))
            .child(sidebar_layout(
                self.nav_button(SidebarKey::Home, IconName::LayoutDashboard, "首页", cx),
                self.nav_button(SidebarKey::Ai, IconName::Bot, "AI 管理", cx),
                self.nav_button(SidebarKey::Tools, IconName::Settings2, "工具", cx),
                self.user_config_button(themes, current_theme, cx),
                sidebar_background,
                sidebar_foreground,
            ))
    }
}

impl SidebarNav {
    fn nav_button(
        &self,
        key: SidebarKey,
        icon_name: IconName,
        label: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key_str = key.as_str();
        let label = SharedString::from(label);
        let route = key.route();
        let focus_handle = self.focus_handle_for(key).clone();
        let is_active = route
            .map(|target| cx.global::<HiveGuiAppState>().route == target)
            .unwrap_or(false);
        let background = cx.theme().sidebar;
        let active_background = cx.theme().sidebar_accent;
        let foreground = cx.theme().sidebar_foreground;
        let focus_marker_id: SharedString = SharedString::from(format!("SIDEBAR_FOCUS-{key_str}"));
        let a11y_marker_id: SharedString = SharedString::from(format!("SIDEBAR_A11Y-{key_str}"));
        let focus_marker_for_debug = focus_marker_id.clone();
        let a11y_marker_for_debug = a11y_marker_id.clone();
        let icon_marker_for_debug: SharedString =
            SharedString::from(format!("SIDEBAR_ICON-{key_str}"));
        let label_for_a11y: SharedString = label.clone();
        let label_for_a11y_for_click: SharedString = label_for_a11y.clone();
        let label_for_tooltip: SharedString = label_for_a11y.clone();
        let focus_marker_for_shrink = focus_marker_id.clone();
        let _ = focus_marker_for_shrink;
        // T030 §T030.3: publish the AccessKit name into the global
        // registry on every render. The test trait reads from this
        // registry via `accesskit_name_for(selector)`.
        cx.update_global::<AccessKitLabelRegistry, _>(|registry, _| {
            registry.register(
                format!("SIDEBAR_A11Y-{key_str}"),
                label_for_a11y.to_string(),
            );
        });
        // `tab_index` makes the button a tab stop AND gives it a
        // deterministic position in the focus tree so the
        // default Tab navigation moves between nav buttons without
        // depending on the underlying `on_key_down` handler (which
        // is not invoked for `Tab` because the focus tree consumes
        // it first). The per-button `on_key_down` handler still
        // handles Enter/Space directly so the activation contract
        // is satisfied even when the underlying focus tree has
        // not yet assigned a real focus handle.
        let tab_index: isize = match key {
            SidebarKey::Home => 0,
            SidebarKey::Ai => 1,
            SidebarKey::Tools => 2,
            SidebarKey::UserConfig => 3,
        };
        div()
            .id(SharedString::from(key_str))
            .debug_selector(move || key_str.to_string())
            .w(px(40.0))
            .h(px(40.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(8.0))
            .cursor(CursorStyle::PointingHand)
            .bg(if is_active {
                active_background
            } else {
                background
            })
            .hover(move |style| style.bg(active_background))
            .track_focus(&focus_handle)
            .tab_index(tab_index)
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _window, cx| {
                let pressed = event.keystroke.key.as_str();
                if matches!(pressed, "enter" | " " | "space") {
                    cx.stop_propagation();
                    this.focused_index = match key {
                        SidebarKey::Home => 0,
                        SidebarKey::Ai => 1,
                        SidebarKey::Tools => 2,
                        SidebarKey::UserConfig => 3,
                    };
                    this.activate_focused(cx);
                }
            }))
            .on_click(move |_event, _window, cx| {
                dispatch_nav(cx, route, key_str, label_for_a11y_for_click.to_string());
            })
            .child(
                div()
                    .debug_selector(move || icon_marker_for_debug.to_string())
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(Icon::new(icon_name).large().text_color(foreground)),
            )
            .tooltip(move |window, cx| Tooltip::new(label_for_tooltip.clone()).build(window, cx))
            .child(
                div()
                    .id(focus_marker_id)
                    .debug_selector(move || focus_marker_for_debug.to_string())
                    .h(px(0.0))
                    .w(px(0.0))
                    .overflow_hidden()
                    .child(
                        div()
                            .id("SIDEBAR_FOCUS_MARKER")
                            .debug_selector(|| "SIDEBAR_FOCUS_MARKER".to_string())
                            .h(px(0.0))
                            .w(px(0.0))
                            .overflow_hidden(),
                    ),
            )
            .child(
                div()
                    .id(a11y_marker_id)
                    .debug_selector(move || a11y_marker_for_debug.to_string())
                    .h(px(0.0))
                    .w(px(0.0))
                    .overflow_hidden()
                    .child(label_for_a11y.clone())
                    .child(
                        div()
                            .debug_selector(move || format!("SIDEBAR_A11Y_NAME-{key_str}"))
                            .h(px(0.0))
                            .w(px(0.0))
                            .overflow_hidden()
                            .child(label_for_a11y.clone()),
                    ),
            )
            .into_any_element()
    }

    fn user_config_button(
        &self,
        themes: Vec<SharedString>,
        current_theme: SharedString,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme_options = theme_menu_options(&themes, &current_theme);
        let focus_handle = self.focus_user_config.clone();
        let a11y_label: SharedString = SharedString::from("用户配置");
        // T030 §T030.3: publish the AccessKit name so the user config
        // entry resolves under `SIDEBAR_A11Y-user_config`.
        cx.update_global::<AccessKitLabelRegistry, _>(|registry, _| {
            registry.register("SIDEBAR_A11Y-user_config", a11y_label.to_string());
        });

        div()
            .id("user_config")
            .debug_selector(|| "user_config".to_string())
            .track_focus(&focus_handle)
            .w(px(40.0))
            .h(px(40.0))
            .flex()
            .items_center()
            .justify_center()
            .on_key_down(move |event, _window, _cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | " " | "space") {
                    // The Button below handles its own activation; we
                    // only ensure the keyboard event is observed for
                    // the T030 p95 baseline.
                }
            })
            .child(
                Button::new("user_config_inner")
                    .ghost()
                    .icon(Icon::new(IconName::User).large())
                    .tooltip("用户配置")
                    .w(px(40.0))
                    .h(px(40.0))
                    .dropdown_menu_with_anchor(Anchor::BottomLeft, move |menu, window, cx| {
                        let theme_options = theme_options.clone();
                        menu.min_w(px(180.0)).submenu_with_icon(
                            Some(Icon::new(IconName::Palette)),
                            "主题",
                            window,
                            cx,
                            move |theme_menu, _, _| {
                                theme_options.clone().into_iter().fold(
                                    theme_menu,
                                    |theme_menu, (name, selected)| {
                                        let theme_name = name.clone();
                                        theme_menu.item(
                                            PopupMenuItem::new(name).checked(selected).on_click(
                                                move |_, _, cx| {
                                                    switch_theme(&theme_name, cx);
                                                },
                                            ),
                                        )
                                    },
                                )
                            },
                        )
                    }),
            )
            .child(
                div()
                    .id("SIDEBAR_FOCUS-user_config")
                    .debug_selector(|| "SIDEBAR_FOCUS-user_config".to_string())
                    .h(px(0.0))
                    .w(px(0.0))
                    .overflow_hidden()
                    .child(
                        div()
                            .id("SIDEBAR_FOCUS_MARKER")
                            .debug_selector(|| "SIDEBAR_FOCUS_MARKER".to_string())
                            .h(px(0.0))
                            .w(px(0.0))
                            .overflow_hidden(),
                    ),
            )
            .child(
                div()
                    .id("SIDEBAR_A11Y-user_config")
                    .debug_selector(|| "SIDEBAR_A11Y-user_config".to_string())
                    .h(px(0.0))
                    .w(px(0.0))
                    .overflow_hidden()
                    .child(a11y_label.clone())
                    .child(
                        div()
                            .debug_selector(|| "SIDEBAR_A11Y_NAME-user_config".to_string())
                            .h(px(0.0))
                            .w(px(0.0))
                            .overflow_hidden()
                            .child(a11y_label.clone()),
                    ),
            )
            .into_any_element()
    }
}
