use gpui::*;
use gpui_component::{ActiveTheme as _, theme::Theme};

/// Semantic purpose of an action in a management surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionRole {
    Main,
    Edit,
    Delete,
    Warning,
    Neutral,
    Disabled,
}

/// Shared density preset for management action buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionSize {
    Page,
    Row,
    Dialog,
    Compact,
}

/// Theme colors used across an action's interaction states.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ActionColors {
    pub background: Hsla,
    pub hover: Hsla,
    pub active: Hsla,
    pub foreground: Hsla,
}

impl ActionColors {
    pub const fn new(background: Hsla, hover: Hsla, active: Hsla, foreground: Hsla) -> Self {
        Self {
            background,
            hover,
            active,
            foreground,
        }
    }
}

/// Theme colors used by shared management list primitives.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ListColors {
    pub head: Hsla,
    pub row: Hsla,
    pub even_row: Hsla,
    pub hover: Hsla,
    pub active: Hsla,
    pub active_border: Hsla,
    pub border: Hsla,
    pub foreground: Hsla,
    pub muted_foreground: Hsla,
    pub muted: Hsla,
}

/// Stateless semantic style snapshot derived from the active application theme.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ManagementStyle {
    main: ActionColors,
    edit: ActionColors,
    delete: ActionColors,
    warning: ActionColors,
    neutral: ActionColors,
    disabled: ActionColors,
    pub list: ListColors,
}

impl ManagementStyle {
    /// Creates a management style snapshot from the supplied theme.
    pub fn from_theme(theme: &Theme) -> Self {
        Self {
            main: ActionColors::new(
                theme.button_info,
                theme.button_info_hover,
                theme.button_info_active,
                theme.button_info_foreground,
            ),
            edit: ActionColors::new(
                theme.button_success,
                theme.button_success_hover,
                theme.button_success_active,
                theme.button_success_foreground,
            ),
            delete: ActionColors::new(
                theme.button_danger,
                theme.button_danger_hover,
                theme.button_danger_active,
                theme.button_danger_foreground,
            ),
            warning: ActionColors::new(
                theme.button_warning,
                theme.button_warning_hover,
                theme.button_warning_active,
                theme.button_warning_foreground,
            ),
            neutral: ActionColors::new(
                theme.button_secondary,
                theme.button_secondary_hover,
                theme.button_secondary_active,
                theme.button_secondary_foreground,
            ),
            disabled: ActionColors::new(
                theme.muted,
                theme.muted,
                theme.muted,
                theme.muted_foreground,
            ),
            list: ListColors {
                head: theme.list_head,
                row: theme.colors.list,
                even_row: theme.list_even,
                hover: theme.list_hover,
                active: theme.list_active,
                active_border: theme.list_active_border,
                border: theme.border,
                foreground: theme.foreground,
                muted_foreground: theme.muted_foreground,
                muted: theme.muted,
            },
        }
    }

    /// Creates a fresh snapshot from the application's active theme.
    pub fn current(cx: &App) -> Self {
        Self::from_theme(cx.theme())
    }

    /// Returns the interaction colors for a semantic action role.
    pub const fn action(self, role: ActionRole) -> ActionColors {
        match role {
            ActionRole::Main => self.main,
            ActionRole::Edit => self.edit,
            ActionRole::Delete => self.delete,
            ActionRole::Warning => self.warning,
            ActionRole::Neutral => self.neutral,
            ActionRole::Disabled => self.disabled,
        }
    }
}

/// Builds a themed action button with shared role and density semantics.
pub fn action_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    role: ActionRole,
    size: ActionSize,
    style: ManagementStyle,
) -> Stateful<Div> {
    let colors = style.action(role);
    let cursor = if role == ActionRole::Disabled {
        CursorStyle::Arrow
    } else {
        CursorStyle::PointingHand
    };
    let button = div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(match size {
            ActionSize::Row => 3.0,
            ActionSize::Dialog => 6.0,
            ActionSize::Page | ActionSize::Compact => 4.0,
        }))
        .bg(colors.background)
        .text_color(colors.foreground)
        .text_size(px(match size {
            ActionSize::Row => 11.0,
            ActionSize::Compact => 12.0,
            ActionSize::Page | ActionSize::Dialog => 13.0,
        }))
        .cursor(cursor)
        .hover(move |element| element.bg(colors.hover))
        .active(move |element| element.bg(colors.active))
        .child(label.into());

    match size {
        ActionSize::Page => button.px(px(12.0)).py(px(6.0)),
        ActionSize::Row => button.px(px(8.0)).py(px(3.0)),
        ActionSize::Dialog => button.px(px(16.0)).py(px(8.0)),
        ActionSize::Compact => button.px(px(12.0)).py(px(4.0)),
    }
}

/// Builds the outer frame for a management list.
pub fn list_container(style: ManagementStyle) -> Div {
    div()
        .flex()
        .flex_col()
        .border_1()
        .border_color(style.list.border)
        .rounded(px(4.0))
        .overflow_hidden()
}

/// Builds a management list header row.
pub fn list_header(style: ManagementStyle) -> Div {
    div()
        .flex()
        .bg(style.list.head)
        .border_b_1()
        .border_color(style.list.border)
        .text_color(style.list.foreground)
}

/// Builds a management list body row.
pub fn list_row(style: ManagementStyle) -> Div {
    div()
        .flex()
        .items_center()
        .bg(style.list.row)
        .border_b_1()
        .border_color(style.list.border)
        .text_color(style.list.foreground)
}

/// Builds a header cell with either a fixed width or flexible remaining width.
pub fn list_header_cell(width: Option<Pixels>, style: ManagementStyle) -> Div {
    sized_cell(
        div()
            .px(px(8.0))
            .py(px(8.0))
            .text_size(px(12.0))
            .font_weight(FontWeight::BOLD)
            .text_color(style.list.foreground),
        width,
    )
}

/// Builds a body cell with either a fixed width or flexible remaining width.
pub fn list_cell(width: Option<Pixels>, style: ManagementStyle) -> Div {
    sized_cell(
        div()
            .px(px(8.0))
            .py(px(6.0))
            .text_size(px(12.0))
            .text_color(style.list.muted_foreground),
        width,
    )
}

/// Builds a list cell that lays out row actions with the shared button spacing.
pub fn list_actions(width: Option<Pixels>, style: ManagementStyle) -> Div {
    list_cell(width, style).flex().items_center().gap(px(4.0))
}

fn sized_cell(cell: Div, width: Option<Pixels>) -> Div {
    match width {
        Some(width) => cell.w(width).flex_shrink_0(),
        None => cell.flex_1(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ActionColors, ActionRole, ActionSize, ListColors, ManagementStyle, action_button,
        list_actions, list_cell, list_container, list_header, list_header_cell, list_row,
    };
    use gpui::{
        Context, Hsla, IntoElement, Render, TestAppContext, VisualTestContext, Window, hsla,
        prelude::*, px, size,
    };
    use gpui_component::theme::Theme;

    fn color(hue: f32) -> Hsla {
        hsla(hue, 0.7, 0.5, 1.0)
    }

    fn assert_management_source_migrated(name: &str, source: &str) {
        assert!(
            source.contains("management_style"),
            "{name} does not import the shared management style"
        );
        for legacy in ["rgb(0x", "rgba(0x"] {
            assert!(
                !source.contains(legacy),
                "{name} still contains legacy management style {legacy}"
            );
        }
    }

    #[test]
    fn category_view_uses_shared_management_style() {
        assert_management_source_migrated("category_view", include_str!("category_view.rs"));
    }

    #[test]
    fn action_roles_use_their_theme_families() {
        let mut theme = Theme::default();
        theme.colors.button_info = color(0.10);
        theme.colors.button_info_hover = color(0.11);
        theme.colors.button_info_active = color(0.12);
        theme.colors.button_info_foreground = color(0.13);
        theme.colors.button_success = color(0.20);
        theme.colors.button_success_hover = color(0.21);
        theme.colors.button_success_active = color(0.22);
        theme.colors.button_success_foreground = color(0.23);
        theme.colors.button_danger = color(0.30);
        theme.colors.button_danger_hover = color(0.31);
        theme.colors.button_danger_active = color(0.32);
        theme.colors.button_danger_foreground = color(0.33);
        theme.colors.button_warning = color(0.40);
        theme.colors.button_warning_hover = color(0.41);
        theme.colors.button_warning_active = color(0.42);
        theme.colors.button_warning_foreground = color(0.43);
        theme.colors.button_secondary = color(0.50);
        theme.colors.button_secondary_hover = color(0.51);
        theme.colors.button_secondary_active = color(0.52);
        theme.colors.button_secondary_foreground = color(0.53);
        theme.colors.muted = color(0.60);
        theme.colors.muted_foreground = color(0.61);

        let style = ManagementStyle::from_theme(&theme);
        assert_eq!(
            style.action(ActionRole::Main),
            ActionColors::new(color(0.10), color(0.11), color(0.12), color(0.13))
        );
        assert_eq!(
            style.action(ActionRole::Edit),
            ActionColors::new(color(0.20), color(0.21), color(0.22), color(0.23))
        );
        assert_eq!(
            style.action(ActionRole::Delete),
            ActionColors::new(color(0.30), color(0.31), color(0.32), color(0.33))
        );
        assert_eq!(
            style.action(ActionRole::Warning),
            ActionColors::new(color(0.40), color(0.41), color(0.42), color(0.43))
        );
        assert_eq!(
            style.action(ActionRole::Neutral),
            ActionColors::new(color(0.50), color(0.51), color(0.52), color(0.53))
        );
        assert_eq!(
            style.action(ActionRole::Disabled),
            ActionColors::new(color(0.60), color(0.60), color(0.60), color(0.61))
        );
        assert_ne!(
            style.action(ActionRole::Main).background,
            theme.button_primary
        );
    }

    #[test]
    fn list_style_uses_theme_list_tokens() {
        let mut theme = Theme::default();
        theme.colors.list_head = color(0.10);
        theme.colors.list = color(0.20);
        theme.colors.list_even = color(0.30);
        theme.colors.list_hover = color(0.40);
        theme.colors.list_active = color(0.50);
        theme.colors.list_active_border = color(0.60);
        theme.colors.border = color(0.70);
        theme.colors.foreground = color(0.80);
        theme.colors.muted_foreground = color(0.90);
        theme.colors.muted = color(0.95);

        assert_eq!(
            ManagementStyle::from_theme(&theme).list,
            ListColors {
                head: color(0.10),
                row: color(0.20),
                even_row: color(0.30),
                hover: color(0.40),
                active: color(0.50),
                active_border: color(0.60),
                border: color(0.70),
                foreground: color(0.80),
                muted_foreground: color(0.90),
                muted: color(0.95),
            }
        );
    }

    #[gpui::test]
    fn management_style_recomputes_after_theme_mode_changes(cx: &mut TestAppContext) {
        use gpui_component::{ThemeMode, theme::Theme};

        cx.update(|cx| {
            gpui_component::theme::init(cx);
            gpui_component::init(cx);
        });
        let (light, dark) = cx.update(|cx| {
            Theme::change(ThemeMode::Light, None, cx);
            let light = ManagementStyle::current(cx);
            Theme::change(ThemeMode::Dark, None, cx);
            let dark = ManagementStyle::current(cx);
            (light, dark)
        });

        assert_ne!(light.list.head, dark.list.head);
        assert_ne!(light.list.row, dark.list.row);
        assert_ne!(
            light.action(ActionRole::Main).background,
            dark.action(ActionRole::Main).background
        );
    }

    struct ListFixture;

    impl Render for ListFixture {
        fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let style = ManagementStyle::current(cx);
            list_container(style)
                .w_full()
                .debug_selector(|| "MANAGEMENT_LIST".to_owned())
                .child(
                    list_header(style)
                        .debug_selector(|| "MANAGEMENT_HEADER".to_owned())
                        .child(list_header_cell(Some(px(120.0)), style).child("名称"))
                        .child(list_header_cell(None, style).child("操作")),
                )
                .child(
                    list_row(style)
                        .debug_selector(|| "MANAGEMENT_ROW".to_owned())
                        .child(list_cell(Some(px(120.0)), style).child("测试项"))
                        .child(
                            list_actions(None, style)
                                .justify_end()
                                .child(
                                    action_button(
                                        "EDIT_ACTION",
                                        "编辑",
                                        ActionRole::Edit,
                                        ActionSize::Row,
                                        style,
                                    )
                                    .debug_selector(|| "EDIT_ACTION".to_owned()),
                                )
                                .child(
                                    action_button(
                                        "DELETE_ACTION",
                                        "删除",
                                        ActionRole::Delete,
                                        ActionSize::Row,
                                        style,
                                    )
                                    .debug_selector(|| "DELETE_ACTION".to_owned()),
                                ),
                        ),
                )
        }
    }

    #[gpui::test]
    fn shared_list_primitives_align_header_row_and_action(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::theme::init(cx);
            gpui_component::init(cx);
        });
        let window = cx.open_window(size(px(480.0), px(160.0)), |_, _| ListFixture);
        cx.run_until_parked();

        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let list = cx.debug_bounds("MANAGEMENT_LIST").expect("list bounds");
        let header = cx.debug_bounds("MANAGEMENT_HEADER").expect("header bounds");
        let row = cx.debug_bounds("MANAGEMENT_ROW").expect("row bounds");
        let edit_action = cx.debug_bounds("EDIT_ACTION").expect("edit action bounds");
        let delete_action = cx
            .debug_bounds("DELETE_ACTION")
            .expect("delete action bounds");

        assert_eq!(header.left(), row.left());
        assert_eq!(header.right(), row.right());
        assert_eq!(header.bottom(), row.top());
        assert!(header.top() >= list.top());
        assert!(row.bottom() <= list.bottom());
        assert!(edit_action.left() >= row.left());
        assert!(edit_action.right() <= row.right());
        assert!(delete_action.left() >= row.left());
        assert!(delete_action.right() <= row.right());
        assert_eq!(delete_action.left() - edit_action.right(), px(4.0));
    }
}
