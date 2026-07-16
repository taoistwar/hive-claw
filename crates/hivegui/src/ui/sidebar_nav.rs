use gpui::{
    Anchor, AnyElement, App, Context, CursorStyle, IntoElement, MouseButton, SharedString, Window,
    div, prelude::*, px,
};
use gpui_component::{
    ActiveTheme as _, Icon, IconName,
    button::{Button, ButtonVariants as _},
    menu::{DropdownMenu as _, PopupMenuItem},
    theme::{Theme, ThemeRegistry},
    tooltip::Tooltip,
};

use crate::ui::app::{AppRoute, HiveGuiAppState};

pub struct SidebarNav;

impl SidebarNav {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self
    }
}

fn sidebar_layout(
    home: impl IntoElement,
    extension: impl IntoElement,
    ai: impl IntoElement,
    tools: impl IntoElement,
    system_settings: impl IntoElement,
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
                .child(sidebar_slot(extension))
                .child(sidebar_slot(ai))
                .child(sidebar_slot(tools))
                .child(sidebar_slot(system_settings)),
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
                div().size_full().debug_selector(|| "EXTENSION".to_owned()),
                div().size_full().debug_selector(|| "AI".to_owned()),
                div().size_full().debug_selector(|| "TOOLS".to_owned()),
                div()
                    .size_full()
                    .debug_selector(|| "SYSTEM_SETTINGS".to_owned()),
                div()
                    .size_full()
                    .debug_selector(|| "USER_CONFIG".to_owned()),
                rgb(0xf0f0f7).into(),
                rgb(0x333333).into(),
            )
        }
    }

    #[gpui::test]
    fn tools_and_system_settings_are_in_top_group(cx: &mut TestAppContext) {
        let window = cx.open_window(size(px(48.0), px(640.0)), |_, _| SidebarLayoutTestView);
        cx.run_until_parked();

        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let ai = cx.debug_bounds("AI").expect("AI button bounds");
        let tools = cx.debug_bounds("TOOLS").expect("tools button bounds");
        let system_settings = cx
            .debug_bounds("SYSTEM_SETTINGS")
            .expect("system settings button bounds");
        let user_config = cx
            .debug_bounds("USER_CONFIG")
            .expect("user configuration button bounds");

        assert_eq!(tools.top(), ai.bottom() + px(8.0));
        assert_eq!(system_settings.top(), tools.bottom() + px(8.0));
        assert_eq!(user_config.bottom(), px(628.0));
        assert!(system_settings.bottom() < user_config.top());
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

        sidebar_layout(
            self.nav_button(
                "home",
                IconName::LayoutDashboard,
                "首页",
                AppRoute::Home,
                cx,
            ),
            self.nav_button(
                "extension",
                IconName::GalleryVerticalEnd,
                "扩展管理",
                AppRoute::Extension,
                cx,
            ),
            self.nav_button("ai", IconName::Bot, "AI 管理", AppRoute::Ai, cx),
            self.nav_button("tools", IconName::Settings2, "工具", AppRoute::Tools, cx),
            self.nav_button(
                "system_settings",
                IconName::Settings,
                "系统设置",
                AppRoute::SystemSettings,
                cx,
            ),
            self.user_config_button(themes, current_theme),
            sidebar_background,
            sidebar_foreground,
        )
    }
}

impl SidebarNav {
    fn nav_button(
        &self,
        id: &str,
        icon_name: IconName,
        label: &str,
        route: AppRoute,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = SharedString::from(id);
        let label = SharedString::from(label);
        let is_active = cx.global::<HiveGuiAppState>().route == route;
        let background = cx.theme().sidebar;
        let active_background = cx.theme().sidebar_accent;
        let foreground = cx.theme().sidebar_foreground;
        div()
            .id(id)
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
            .child(Icon::new(icon_name).text_color(foreground))
            .tooltip(move |window, cx| Tooltip::new(label.clone()).build(window, cx))
            .on_mouse_down(MouseButton::Left, move |_event, _window, cx| {
                cx.update_global::<HiveGuiAppState, _>(|app, _| app.route = route);
                cx.refresh_windows();
            })
            .into_any_element()
    }

    fn user_config_button(
        &self,
        themes: Vec<SharedString>,
        current_theme: SharedString,
    ) -> AnyElement {
        let theme_options = theme_menu_options(&themes, &current_theme);

        Button::new("user_config")
            .ghost()
            .icon(IconName::User)
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
            })
            .into_any_element()
    }
}
