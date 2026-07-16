# Theme-Aware Management UI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Unify every HiveGUI management-page button and list around the category-page structure while deriving all colors from the active GPUI Component theme.

**Architecture:** Add a stateless `management_style` module that maps action roles and list primitives to active-theme colors and owns the shared dimensions. Existing views keep their models, columns, async loading, event handlers, dialogs, and pagination state; they replace local color and spacing code with the shared builders. Representative visual tests plus per-source migration contracts make the consistency requirement executable.

**Tech Stack:** Rust 2024, GPUI, gpui-component theme tokens, inline `#[test]` and `#[gpui::test]` tests, Cargo.

## Global Constraints

- The approved design is `docs/superpowers/specs/2026-07-15-theme-aware-management-ui-design.md`.
- `ActionRole::Main` must use `button_info`, `button_info_hover`, `button_info_active`, and `button_info_foreground`; it must not use `button_primary`.
- `Edit`, `Delete`, `Warning`, and `Neutral` map to the theme's success, danger, warning, and secondary button families respectively.
- List colors come from `list_head`, `list`, `list_even`, `list_hover`, `list_active`, `list_active_border`, `border`, `foreground`, `muted_foreground`, and `muted`.
- Shared dimensions are fixed at: 1 px container border, 4 px container radius, 8 px horizontal cell padding, 8 px header vertical padding, 6 px row vertical padding, 12 px header/body text, 4 px action gap, and 8 x 3 px row-action padding.
- Preserve every page's data flow, columns, element IDs used by behavior, CRUD handlers, pagination rules, modal visibility, and scroll handles.
- Do not add dependencies and do not introduce a generic data model or generic table state.
- The checkout is already dirty and many target files contain pre-existing user changes. Do not reset, checkout, stash, stage, or commit implementation files. End each task with targeted tests and `git diff --check`; create a commit only if the user later explicitly requests one.

---

## File Responsibility Map

- Create `crates/hivegui/src/ui/management_style.rs`: action-role mapping, shared list colors, button builders, list primitives, shared dimensions, style unit tests, and migration-contract tests.
- Modify `crates/hivegui/src/ui/mod.rs`: export `management_style`.
- Modify `category_view.rs`, `tag_view.rs`, `capability_view.rs`, `function_view.rs`, `skill_view.rs`, `tool_view.rs`, `workflow_view.rs`, `plugin_view.rs`, and `agent_view.rs`: standard CRUD pages.
- Modify `global_config.rs`: themed controls, category-style list, and geometry regression test.
- Modify `llm_config.rs`: themed controls, standard Model/Preset/Provider rows, and geometry regression test.
- Modify `datasource_view.rs`, `datasource_form.rs`, and `tree_nav.rs`: themed data-source management actions, dialogs, and tree/list surfaces.
- Modify `table_viewer.rs`: themed database tabs, data grid, selection, pagination, and row controls without changing table state.
- Modify `settings_view.rs`: themed export/import actions.

---

### Task 1: Shared Theme Semantics and List Primitives

**Files:**
- Create: `crates/hivegui/src/ui/management_style.rs`
- Modify: `crates/hivegui/src/ui/mod.rs`

**Interfaces:**
- Consumes: `gpui_component::theme::Theme` and `gpui_component::ActiveTheme`.
- Produces: `ActionRole`, `ActionSize`, `ActionColors`, `ListColors`, `ManagementStyle::from_theme`, `ManagementStyle::current`, `ManagementStyle::action`, `action_button`, `list_container`, `list_header`, `list_row`, `list_header_cell`, `list_cell`, and `list_actions`.

- [ ] **Step 1: Write failing semantic and visual tests**

Create `management_style.rs` with the test module first. The semantic test must assign distinct colors to every field so an accidental mapping cannot pass:

```rust
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
        assert_ne!(style.action(ActionRole::Main).background, theme.button_primary);
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
```

Add `pub mod management_style;` to `ui/mod.rs` so Cargo compiles the new tests.

- [ ] **Step 2: Run the tests and verify the red state**

Run:

```bash
cargo test -p hivegui --lib ui::management_style::tests
```

Expected: compilation fails because the imported style types and builders do not exist yet.

- [ ] **Step 3: Implement the shared style API**

Add these production definitions above the test module in `management_style.rs`:

```rust
use gpui::*;
use gpui_component::{ActiveTheme as _, theme::Theme};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionRole {
    Main,
    Edit,
    Delete,
    Warning,
    Neutral,
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionSize {
    Page,
    Row,
    Dialog,
    Compact,
}

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
                row: theme.list,
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

    pub fn current(cx: &App) -> Self {
        Self::from_theme(cx.theme())
    }

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

pub fn list_container(style: ManagementStyle) -> Div {
    div()
        .flex()
        .flex_col()
        .border_1()
        .border_color(style.list.border)
        .rounded(px(4.0))
        .overflow_hidden()
}

pub fn list_header(style: ManagementStyle) -> Div {
    div()
        .flex()
        .bg(style.list.head)
        .border_b_1()
        .border_color(style.list.border)
        .text_color(style.list.foreground)
}

pub fn list_row(style: ManagementStyle) -> Div {
    div()
        .flex()
        .items_center()
        .bg(style.list.row)
        .border_b_1()
        .border_color(style.list.border)
        .text_color(style.list.foreground)
}

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

pub fn list_actions(width: Option<Pixels>, style: ManagementStyle) -> Div {
    list_cell(width, style)
        .flex()
        .items_center()
        .gap(px(4.0))
}

fn sized_cell(cell: Div, width: Option<Pixels>) -> Div {
    match width {
        Some(width) => cell.w(width).flex_shrink_0(),
        None => cell.flex_1(),
    }
}
```

- [ ] **Step 4: Verify the shared API and fixture**

Run:

```bash
cargo test -p hivegui --lib ui::management_style::tests
rustfmt --edition 2024 crates/hivegui/src/ui/management_style.rs crates/hivegui/src/ui/mod.rs
git diff --check -- crates/hivegui/src/ui/management_style.rs crates/hivegui/src/ui/mod.rs
```

Expected: all three management-style tests pass; formatting and diff checks exit 0.

---

### Task 2: Make Category the Theme-Aware Reference List

**Files:**
- Modify: `crates/hivegui/src/ui/management_style.rs`
- Modify: `crates/hivegui/src/ui/category_view.rs`

**Interfaces:**
- Consumes: all Task 1 builders and `ManagementStyle::current`.
- Produces: the reference implementation for a standard management list and the reusable migration assertion `assert_management_source_migrated` inside the test module.

- [ ] **Step 1: Add the failing category migration contract**

Add this helper and test inside `management_style.rs`'s test module:

```rust
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
```

- [ ] **Step 2: Verify that the legacy category palette is detected**

Run:

```bash
cargo test -p hivegui --lib ui::management_style::tests::category_view_uses_shared_management_style -- --exact
```

Expected: FAIL and report the first hardcoded category color.

- [ ] **Step 3: Migrate category list, actions, pagination, and dialogs**

Import the shared API:

```rust
use crate::ui::management_style::{
    ActionRole, ActionSize, ManagementStyle, action_button, list_cell, list_container,
    list_actions, list_header, list_header_cell, list_row,
};
```

At the beginning of `render_table` and `render`, copy `let style = ManagementStyle::current(cx);`. Rebuild the existing table wrappers with `list_container`, `list_header`, `list_row`, `list_header_cell`, `list_cell`, and `list_actions`, preserving the five existing column widths and cell contents. Every row action cell must use `list_actions` so the shared 4 px gap is enforced.

Replace each category action with the shared builder and keep its existing handler:

```rust
action_button(
    ("edit", category_id as u64),
    "编辑",
    ActionRole::Edit,
    ActionSize::Row,
    style,
)
```

```rust
action_button(
    ("delete", category_id as u64),
    "删除",
    ActionRole::Delete,
    ActionSize::Row,
    style,
)
```

Use `ActionRole::Main` for add, enabled pagination, and save; `ActionRole::Neutral` for cancel; `ActionRole::Disabled` for disabled pagination. Copy modal, border, warning, primary text, and secondary text colors from `cx.theme()` before constructing listeners, then use `popover`, `popover_foreground`, `border`, `warning`, `foreground`, and `muted_foreground` instead of fixed RGB values.

- [ ] **Step 4: Verify the reference page**

Run:

```bash
cargo test -p hivegui --lib ui::management_style::tests::category_view_uses_shared_management_style -- --exact
cargo test -p hivegui --lib ui::management_style::tests
rustfmt --edition 2024 crates/hivegui/src/ui/category_view.rs crates/hivegui/src/ui/management_style.rs
git diff --check -- crates/hivegui/src/ui/category_view.rs crates/hivegui/src/ui/management_style.rs
```

Expected: the category contract and shared visual tests pass; diff check exits 0.

---

### Task 3: Migrate Tag and Capability Lists

**Files:**
- Modify: `crates/hivegui/src/ui/management_style.rs`
- Modify: `crates/hivegui/src/ui/tag_view.rs`
- Modify: `crates/hivegui/src/ui/capability_view.rs`

**Interfaces:**
- Consumes: Task 1 shared builders and Task 2 migration assertion.
- Produces: themed tag and capability CRUD pages with unchanged search, pagination, and forms.

- [ ] **Step 1: Add failing source contracts**

```rust
#[test]
fn tag_and_capability_views_use_shared_management_style() {
    assert_management_source_migrated("tag_view", include_str!("tag_view.rs"));
    assert_management_source_migrated(
        "capability_view",
        include_str!("capability_view.rs"),
    );
}
```

- [ ] **Step 2: Run the contract and verify it fails on legacy colors**

Run:

```bash
cargo test -p hivegui --lib ui::management_style::tests::tag_and_capability_views_use_shared_management_style -- --exact
```

Expected: FAIL for `tag_view` before reaching or while checking `capability_view`.

- [ ] **Step 3: Migrate both pages**

In each file, import the Task 1 API and compute `ManagementStyle::current(cx)` inside `render`. Replace the existing bordered table, header row, data rows, and cells with the shared list primitives. Preserve Tag's ID/name/color columns and Capability's name/description/danger/category columns.

Use these exact roles throughout both files:

```rust
let add_role = ActionRole::Main;
let edit_role = ActionRole::Edit;
let delete_role = ActionRole::Delete;
let cancel_role = ActionRole::Neutral;
let save_role = ActionRole::Main;
let disabled_page_role = ActionRole::Disabled;
```

Build page, row, and dialog buttons with `ActionSize::Page`, `ActionSize::Row`, and `ActionSize::Dialog` respectively. Replace modal white, header gray, row white, fixed borders, fixed foregrounds, and warning backgrounds with the active theme's `popover`, `popover_foreground`, `border`, `foreground`, `muted_foreground`, and `warning.opacity(0.15)`.

- [ ] **Step 4: Verify both pages**

Run:

```bash
cargo test -p hivegui --lib ui::management_style::tests::tag_and_capability_views_use_shared_management_style -- --exact
cargo test -p hivegui --lib ui::management_style::tests
rustfmt --edition 2024 crates/hivegui/src/ui/tag_view.rs crates/hivegui/src/ui/capability_view.rs crates/hivegui/src/ui/management_style.rs
git diff --check -- crates/hivegui/src/ui/tag_view.rs crates/hivegui/src/ui/capability_view.rs crates/hivegui/src/ui/management_style.rs
```

Expected: all commands exit 0.

---

### Task 4: Migrate Function and Skill Lists

**Files:**
- Modify: `crates/hivegui/src/ui/management_style.rs`
- Modify: `crates/hivegui/src/ui/function_view.rs`
- Modify: `crates/hivegui/src/ui/skill_view.rs`

**Interfaces:**
- Consumes: shared builders and migration assertion.
- Produces: themed Function and Skill lists while preserving JSON/schema fields and long-form editors.

- [ ] **Step 1: Add the failing contract**

```rust
#[test]
fn function_and_skill_views_use_shared_management_style() {
    assert_management_source_migrated("function_view", include_str!("function_view.rs"));
    assert_management_source_migrated("skill_view", include_str!("skill_view.rs"));
}
```

- [ ] **Step 2: Confirm red**

Run:

```bash
cargo test -p hivegui --lib ui::management_style::tests::function_and_skill_views_use_shared_management_style -- --exact
```

Expected: FAIL on a legacy table or action color.

- [ ] **Step 3: Apply shared structure and semantics**

Import the shared module in both files. Use `list_container`, `list_header`, and `list_row` for their current tables; use `list_header_cell` and `list_cell` for every existing fixed-width and flexible column. Keep Function's identifier/name/kind/category columns and Skill's identifier/name/source/always/category columns unchanged.

Map add, save, and enabled pagination to `Main`; edit to `Edit`; delete to `Delete`; cancel to `Neutral`; disabled pagination to `Disabled`. Update each bottom `form_field` helper to receive copied theme colors or `ManagementStyle`, and replace fixed input backgrounds/borders with `input`, `background`, `foreground`, and `muted_foreground`.

- [ ] **Step 4: Verify both views**

Run:

```bash
cargo test -p hivegui --lib ui::management_style::tests::function_and_skill_views_use_shared_management_style -- --exact
cargo test -p hivegui --lib ui::management_style::tests
rustfmt --edition 2024 crates/hivegui/src/ui/function_view.rs crates/hivegui/src/ui/skill_view.rs crates/hivegui/src/ui/management_style.rs
git diff --check -- crates/hivegui/src/ui/function_view.rs crates/hivegui/src/ui/skill_view.rs crates/hivegui/src/ui/management_style.rs
```

Expected: all commands exit 0.

---

### Task 5: Migrate Tool and Workflow Lists

**Files:**
- Modify: `crates/hivegui/src/ui/management_style.rs`
- Modify: `crates/hivegui/src/ui/tool_view.rs`
- Modify: `crates/hivegui/src/ui/workflow_view.rs`

**Interfaces:**
- Consumes: shared builders and migration assertion.
- Produces: themed Tool and Workflow pages without changing kind selectors, schemas, or associations.

- [ ] **Step 1: Add the failing contract**

```rust
#[test]
fn tool_and_workflow_views_use_shared_management_style() {
    assert_management_source_migrated("tool_view", include_str!("tool_view.rs"));
    assert_management_source_migrated("workflow_view", include_str!("workflow_view.rs"));
}
```

- [ ] **Step 2: Confirm the contract fails**

Run:

```bash
cargo test -p hivegui --lib ui::management_style::tests::tool_and_workflow_views_use_shared_management_style -- --exact
```

Expected: FAIL on the current hardcoded palette.

- [ ] **Step 3: Apply the shared style**

Import the Task 1 API in both files and rebuild the existing list wrappers and cells with the shared primitives. Keep Tool's identifier/name/kind/source/always columns and Workflow's identifier/name/timeout/category columns unchanged. Use `ActionSize::Compact` for compact selectors, `Page` for header and pagination operations, `Row` for edit/delete, and `Dialog` for form actions.

Use `Main` for selected kind/boolean options that currently use the old blue, `Neutral` for unselected options, and `Disabled` for unavailable pagination. Use the theme's `popover`, `popover_foreground`, `input`, `border`, `foreground`, `muted`, `muted_foreground`, and `warning` colors for dialogs, fields, alerts, and chips.

- [ ] **Step 4: Verify both views**

Run:

```bash
cargo test -p hivegui --lib ui::management_style::tests::tool_and_workflow_views_use_shared_management_style -- --exact
cargo test -p hivegui --lib ui::management_style::tests
rustfmt --edition 2024 crates/hivegui/src/ui/tool_view.rs crates/hivegui/src/ui/workflow_view.rs crates/hivegui/src/ui/management_style.rs
git diff --check -- crates/hivegui/src/ui/tool_view.rs crates/hivegui/src/ui/workflow_view.rs crates/hivegui/src/ui/management_style.rs
```

Expected: all commands exit 0.

---

### Task 6: Migrate Plugin and Agent Lists

**Files:**
- Modify: `crates/hivegui/src/ui/management_style.rs`
- Modify: `crates/hivegui/src/ui/plugin_view.rs`
- Modify: `crates/hivegui/src/ui/agent_view.rs`

**Interfaces:**
- Consumes: shared builders and migration assertion.
- Produces: themed Plugin and Agent lists while preserving runtime, version, parent, depth, and model fields.

- [ ] **Step 1: Add the failing contract**

```rust
#[test]
fn plugin_and_agent_views_use_shared_management_style() {
    assert_management_source_migrated("plugin_view", include_str!("plugin_view.rs"));
    assert_management_source_migrated("agent_view", include_str!("agent_view.rs"));
}
```

- [ ] **Step 2: Confirm red**

Run:

```bash
cargo test -p hivegui --lib ui::management_style::tests::plugin_and_agent_views_use_shared_management_style -- --exact
```

Expected: FAIL on a fixed management color.

- [ ] **Step 3: Migrate list and dialog visuals**

Import the shared API in both files. Replace their current table wrappers, header rows, data rows, and cells with shared list primitives without changing column order or widths. Replace header add, edit, delete, pagination, form save, form cancel, and confirmation actions with the corresponding role and size.

Map destructive confirmation to `Delete`, confirmation cancel to `Neutral`, and any disabled parent/depth operation to `Disabled`. Use active-theme popover, input, border, foreground, muted, warning, and danger colors for every remaining fixed dialog/list color.

- [ ] **Step 4: Verify both pages and all standard CRUD contracts**

Run:

```bash
cargo test -p hivegui --lib ui::management_style::tests
rustfmt --edition 2024 crates/hivegui/src/ui/plugin_view.rs crates/hivegui/src/ui/agent_view.rs crates/hivegui/src/ui/management_style.rs
git diff --check -- crates/hivegui/src/ui/plugin_view.rs crates/hivegui/src/ui/agent_view.rs crates/hivegui/src/ui/management_style.rs
```

Expected: every standard CRUD source contract and shared visual test passes.

---

### Task 7: Migrate Global Configuration and Add Geometry Coverage

**Files:**
- Modify: `crates/hivegui/src/ui/management_style.rs`
- Modify: `crates/hivegui/src/ui/global_config.rs`

**Interfaces:**
- Consumes: shared builders, list primitives, and migration assertion.
- Produces: a category-style global-config list; debug selectors `GLOBAL_CONFIG_LIST`, `GLOBAL_CONFIG_HEADER`, `GLOBAL_CONFIG_ROW_1`, and `GLOBAL_CONFIG_ACTIONS_1`.

- [ ] **Step 1: Add failing source and rendered-geometry tests**

Add the source contract to `management_style.rs`:

```rust
#[test]
fn global_config_view_uses_shared_management_style() {
    assert_management_source_migrated(
        "global_config",
        include_str!("global_config.rs"),
    );
}
```

Extend `global_config.rs`'s test module with this visual test:

```rust
#[gpui::test]
fn global_config_list_matches_reference_geometry(cx: &mut gpui::TestAppContext) {
    use crate::datasource::GlobalConfig;
    use gpui::{VisualTestContext, px, size};

    cx.update(|cx| {
        gpui_component::theme::init(cx);
        gpui_component::init(cx);
    });
    let window = cx.open_window(size(px(1000.0), px(600.0)), |_, cx| {
        let mut view = super::GlobalConfigView::new(cx);
        view.loaded = true;
        view.total = 1;
        view.items = vec![GlobalConfig {
            id: 1,
            name: "测试配置".to_string(),
            key: "test.key".to_string(),
            config_type: "text".to_string(),
            data: "value".to_string(),
            created_at: String::new(),
            updated_at: String::new(),
        }];
        view
    });
    cx.run_until_parked();

    let mut cx = VisualTestContext::from_window(window.into(), cx);
    let list = cx.debug_bounds("GLOBAL_CONFIG_LIST").expect("list bounds");
    let header = cx.debug_bounds("GLOBAL_CONFIG_HEADER").expect("header bounds");
    let row = cx.debug_bounds("GLOBAL_CONFIG_ROW_1").expect("row bounds");
    let actions = cx
        .debug_bounds("GLOBAL_CONFIG_ACTIONS_1")
        .expect("action bounds");
    assert_eq!(header.left(), row.left());
    assert_eq!(header.right(), row.right());
    assert_eq!(header.bottom(), row.top());
    assert!(row.bottom() <= list.bottom());
    assert!(actions.left() >= row.left());
    assert!(actions.right() <= row.right());
}
```

- [ ] **Step 2: Verify both tests fail for the intended reasons**

Run:

```bash
cargo test -p hivegui --lib ui::management_style::tests::global_config_view_uses_shared_management_style -- --exact
cargo test -p hivegui --lib ui::global_config::tests::global_config_list_matches_reference_geometry -- --exact
```

Expected: the source contract reports `0x6f5699`; the visual test fails because the debug selectors do not exist.

- [ ] **Step 3: Migrate all global-config controls and list wrappers**

Import the shared API. Replace `render_table_header`, `table_cell`, `row_action`, `page_button`, and the local `btn` styling with shared primitives/builders. Keep `config_column_widths` and all value-conversion code unchanged.

Use these mappings:

- Add, search, save/update, enabled page, selected config type, and selected boolean: `Main`.
- Edit: `Edit`.
- Delete: `Delete`.
- Cancel: `Neutral`.
- Disabled page: `Disabled`.

Use `style.list.muted` and `style.list.muted_foreground` for the type badge. Add the four specified debug selectors to the container, header, row, and action cell. Use active-theme `popover`, `popover_foreground`, `input`, `border`, `foreground`, `muted_foreground`, and `danger` for form and validation surfaces.

- [ ] **Step 4: Verify global configuration**

Run:

```bash
cargo test -p hivegui --lib ui::global_config::tests
cargo test -p hivegui --lib ui::management_style::tests::global_config_view_uses_shared_management_style -- --exact
rustfmt --edition 2024 crates/hivegui/src/ui/global_config.rs crates/hivegui/src/ui/management_style.rs
git diff --check -- crates/hivegui/src/ui/global_config.rs crates/hivegui/src/ui/management_style.rs
```

Expected: conversion tests, geometry test, and source contract all pass.

---

### Task 8: Standardize LLM Model, Preset, and Provider Lists

**Files:**
- Modify: `crates/hivegui/src/ui/management_style.rs`
- Modify: `crates/hivegui/src/ui/llm_config.rs`

**Interfaces:**
- Consumes: shared style API and existing LLM relationship/scroll behavior.
- Produces: standard list containers and selectors `MODEL_LIST`, `MODEL_LIST_HEADER`, `MODEL_LIST_ROW_42`, `MODEL_LIST_ACTIONS_42`, `PROVIDER_LIST`, and `PRESET_LIST`.

- [ ] **Step 1: Add failing source and geometry tests**

Add to `management_style.rs`:

```rust
#[test]
fn llm_config_uses_shared_management_style() {
    assert_management_source_migrated("llm_config", include_str!("llm_config.rs"));
}
```

Add this independent test to the existing `llm_config.rs` test module:

```rust
#[gpui::test]
fn model_list_matches_reference_geometry(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_component::theme::init(cx);
        gpui_component::init(cx);
    });
    let window = cx.open_window(size(px(1000.0), px(600.0)), |_, cx| {
        let mut view = LLMConfigView::new(cx);
        view.loaded = true;
        view.selected_preset_id = Some(7);
        view.presets = vec![LlmPreset {
            id: 7,
            name: "默认".to_string(),
            description: String::new(),
            is_default: 1,
            max_tokens: 2048,
            temperature: 0.7,
            created_at: String::new(),
            updated_at: String::new(),
        }];
        view.providers = vec![LlmProvider {
            id: 9,
            category: "openai".to_string(),
            base_url: "https://api.example.com".to_string(),
            token_encrypted: None,
            token_env: "OPENAI_API_KEY".to_string(),
            created_at: String::new(),
            updated_at: String::new(),
        }];
        view.models = vec![LlmModel {
            id: 42,
            name: "chat-model".to_string(),
            preset_id: Some(7),
            provider_id: Some(9),
            priority: 10,
            created_at: String::new(),
            updated_at: String::new(),
        }];
        view
    });
    cx.run_until_parked();

    let mut cx = VisualTestContext::from_window(window.into(), cx);
    let list = cx.debug_bounds("MODEL_LIST").expect("model list bounds");
    let header = cx
        .debug_bounds("MODEL_LIST_HEADER")
        .expect("model header bounds");
    let row = cx
        .debug_bounds("MODEL_LIST_ROW_42")
        .expect("model row bounds");
    let actions = cx
        .debug_bounds("MODEL_LIST_ACTIONS_42")
        .expect("model actions bounds");
    assert_eq!(header.left(), row.left());
    assert_eq!(header.right(), row.right());
    assert_eq!(header.bottom(), row.top());
    assert!(row.bottom() <= list.bottom());
    assert!(actions.left() >= row.left());
    assert!(actions.right() <= row.right());
}
```

- [ ] **Step 2: Confirm red**

Run:

```bash
cargo test -p hivegui --lib ui::management_style::tests::llm_config_uses_shared_management_style -- --exact
cargo test -p hivegui --lib ui::llm_config::tests::model_list_matches_reference_geometry -- --exact
```

Expected: the contract reports old purple/list colors; the geometry test cannot find `MODEL_LIST`.

- [ ] **Step 3: Rebuild LLM lists and controls with shared styles**

Pass `ManagementStyle` into `model_table`, `preset_table`, `provider_table`, `table_header_cell`, `table_cell`, and `table_action_cell`. Wrap each list in `list_container`, use `list_header` and `list_row`, remove the existing 2 px card gaps and row rounding, and preserve every column width and ID debug selector.

Add the new list/header/row/action selectors. Use `Edit` and `Delete` row buttons instead of text-only actions. Map add, save, enabled pagination, selected preset filter, and boolean toggle to `Main`; cancel to `Neutral`; disabled pagination to `Disabled`.

Replace modal white backgrounds with `popover`, menus with `popover`/`popover_foreground`, form borders with `border`/`input`, and descriptive text with `muted_foreground`. Keep `PROVIDER_MODAL`, `PROVIDER_SCROLL`, `CATEGORY_MENU`, and event-propagation behavior unchanged.

- [ ] **Step 4: Verify LLM layout and nested scrolling**

Run:

```bash
cargo test -p hivegui --lib ui::llm_config::tests
cargo test -p hivegui --lib ui::management_style::tests::llm_config_uses_shared_management_style -- --exact
rustfmt --edition 2024 crates/hivegui/src/ui/llm_config.rs crates/hivegui/src/ui/management_style.rs
git diff --check -- crates/hivegui/src/ui/llm_config.rs crates/hivegui/src/ui/management_style.rs
```

Expected: ID, geometry, provider modal, category nested-scroll, and source-contract tests all pass.

---

### Task 9: Theme Data-Source Management and Tree Navigation

**Files:**
- Modify: `crates/hivegui/src/ui/management_style.rs`
- Modify: `crates/hivegui/src/ui/datasource_view.rs`
- Modify: `crates/hivegui/src/ui/datasource_form.rs`
- Modify: `crates/hivegui/src/ui/tree_nav.rs`

**Interfaces:**
- Consumes: shared action roles and list colors.
- Produces: themed data-source actions, form, error dialog, database tree, selected nodes, and tree controls.

- [ ] **Step 1: Add the failing contract**

```rust
#[test]
fn datasource_surfaces_use_shared_management_style() {
    assert_management_source_migrated("datasource_view", include_str!("datasource_view.rs"));
    assert_management_source_migrated("datasource_form", include_str!("datasource_form.rs"));
    assert_management_source_migrated("tree_nav", include_str!("tree_nav.rs"));
}
```

- [ ] **Step 2: Confirm the old data-source palette is detected**

Run:

```bash
cargo test -p hivegui --lib ui::management_style::tests::datasource_surfaces_use_shared_management_style -- --exact
```

Expected: FAIL on a fixed blue, white background, border, or foreground.

- [ ] **Step 3: Migrate data-source surfaces**

In `datasource_form.rs`, replace the local `action_button` with the shared builder: connection test and save use `Main`, cancel uses `Neutral`, and disabled testing/saving uses `Disabled`. Use `popover`, `popover_foreground`, `input`, `border`, `foreground`, `muted_foreground`, `success`, and `danger` for the modal, fields, and status text.

In `datasource_view.rs`, use `Neutral` for close and `Main` for retry/confirm operations; replace fixed dialog and separator colors with theme values.

In `tree_nav.rs`, compute `ManagementStyle::current(cx)` in render. Use `style.list.row`, `even_row`, `hover`, `active`, `active_border`, `foreground`, and `muted_foreground` for databases, tables, selected nodes, and empty/loading states. Use `Main`, `Neutral`, `Warning`, or `Delete` for tree actions according to their existing semantics. Preserve expansion, selection, drag/resize, and table-opening handlers.

- [ ] **Step 4: Verify data-source management**

Run:

```bash
cargo test -p hivegui --lib ui::management_style::tests::datasource_surfaces_use_shared_management_style -- --exact
cargo test -p hivegui --lib ui::management_style::tests
rustfmt --edition 2024 crates/hivegui/src/ui/datasource_view.rs crates/hivegui/src/ui/datasource_form.rs crates/hivegui/src/ui/tree_nav.rs crates/hivegui/src/ui/management_style.rs
git diff --check -- crates/hivegui/src/ui/datasource_view.rs crates/hivegui/src/ui/datasource_form.rs crates/hivegui/src/ui/tree_nav.rs crates/hivegui/src/ui/management_style.rs
```

Expected: all commands exit 0.

---

### Task 10: Theme the Database Table Viewer

**Files:**
- Modify: `crates/hivegui/src/ui/management_style.rs`
- Modify: `crates/hivegui/src/ui/table_viewer.rs`

**Interfaces:**
- Consumes: `ManagementStyle`, existing `OpenTable` state, page cache, selection, resize, and editor logic.
- Produces: themed table tabs, metadata lists, data grid, selected cells/rows, pagination, page-size controls, and refresh button.

- [ ] **Step 1: Add a failing source contract**

```rust
#[test]
fn table_viewer_uses_shared_management_style() {
    assert_management_source_migrated("table_viewer", include_str!("table_viewer.rs"));
}
```

- [ ] **Step 2: Confirm red**

Run:

```bash
cargo test -p hivegui --lib ui::management_style::tests::table_viewer_uses_shared_management_style -- --exact
```

Expected: FAIL on hardcoded table, selection, border, or pagination colors.

- [ ] **Step 3: Replace visual state without touching table state**

Compute `ManagementStyle::current(cx)` in `render` and pass it to `render_table_tabs`, `render_sub_tabs`, `render_sub_tab`, `render_columns`, `render_ddl`, `render_data_tab`, `render_data_grid`, `render_value_viewer`, `render_pagination`, and `render_page_size_option`.

Use these exact mappings:

- Table and metadata headers: `style.list.head`.
- Normal rows: `style.list.row`; alternating rows: `style.list.even_row`.
- Hover: `style.list.hover`.
- Selected row/cell/tab: `style.list.active` with `style.list.active_border`.
- Normal borders: `style.list.border`.
- Current page and selected page size: `Main`.
- Previous/next/refresh: `Neutral` when enabled, `Disabled` when unavailable.
- Destructive row operation: `Delete`; warning operation: `Warning`.

Do not change `page_cache`, `current_offset`, selected row/column, cell editor, resize handles, SQL loading, or value-viewer scrolling.

- [ ] **Step 4: Verify table-viewer compilation and contracts**

Run:

```bash
cargo test -p hivegui --lib ui::management_style::tests::table_viewer_uses_shared_management_style -- --exact
cargo test -p hivegui --lib ui::management_style::tests
rustfmt --edition 2024 crates/hivegui/src/ui/table_viewer.rs crates/hivegui/src/ui/management_style.rs
git diff --check -- crates/hivegui/src/ui/table_viewer.rs crates/hivegui/src/ui/management_style.rs
```

Expected: all commands exit 0.

---

### Task 11: Theme Data Management Buttons and Complete the Source Audit

**Files:**
- Modify: `crates/hivegui/src/ui/management_style.rs`
- Modify: `crates/hivegui/src/ui/settings_view.rs`

**Interfaces:**
- Consumes: shared action builders and every migration contract from Tasks 2 through 10.
- Produces: theme-aware export/import buttons and one complete management-source audit.

- [ ] **Step 1: Add the failing settings contract and complete inventory test**

```rust
#[test]
fn settings_view_uses_shared_management_style() {
    assert_management_source_migrated("settings_view", include_str!("settings_view.rs"));
}

#[test]
fn every_management_surface_is_in_the_style_inventory() {
    let sources = [
        ("category_view", include_str!("category_view.rs")),
        ("tag_view", include_str!("tag_view.rs")),
        ("global_config", include_str!("global_config.rs")),
        ("capability_view", include_str!("capability_view.rs")),
        ("function_view", include_str!("function_view.rs")),
        ("skill_view", include_str!("skill_view.rs")),
        ("tool_view", include_str!("tool_view.rs")),
        ("workflow_view", include_str!("workflow_view.rs")),
        ("plugin_view", include_str!("plugin_view.rs")),
        ("agent_view", include_str!("agent_view.rs")),
        ("llm_config", include_str!("llm_config.rs")),
        ("datasource_view", include_str!("datasource_view.rs")),
        ("datasource_form", include_str!("datasource_form.rs")),
        ("tree_nav", include_str!("tree_nav.rs")),
        ("table_viewer", include_str!("table_viewer.rs")),
        ("settings_view", include_str!("settings_view.rs")),
    ];
    for (name, source) in sources {
        assert_management_source_migrated(name, source);
    }
}
```

- [ ] **Step 2: Confirm settings still violates the shared action rule**

Run:

```bash
cargo test -p hivegui --lib ui::management_style::tests::settings_view_uses_shared_management_style -- --exact
```

Expected: FAIL because `settings_view.rs` does not use `management_style`.

- [ ] **Step 3: Replace data-management action styling**

Import `ActionRole`, `ActionSize`, `ManagementStyle`, and `action_button`. Replace the local `btn` helper:

- Export uses `ActionRole::Main` and `ActionSize::Dialog`.
- Import uses `ActionRole::Warning` and `ActionSize::Dialog`.
- While exporting or importing, render the corresponding button with `ActionRole::Disabled` and keep the existing click guard.

Keep the current themed group boxes, status messages, scroll container, and warning panel. Remove the unused local `btn` function and any now-unused primary/warning hover variables.

- [ ] **Step 4: Run the complete migration audit**

Run:

```bash
cargo test -p hivegui --lib ui::management_style::tests
cargo test -p hivegui --lib ui::settings_view::tests
rustfmt --edition 2024 crates/hivegui/src/ui/settings_view.rs crates/hivegui/src/ui/management_style.rs
git diff --check -- crates/hivegui/src/ui/settings_view.rs crates/hivegui/src/ui/management_style.rs
```

Expected: all migration contracts, shared semantic tests, shared visual test, and settings scrollbar test pass.

---

### Task 12: Verify Live Theme Switching and Full HiveGUI Regression

**Files:**
- Test: all files under `crates/hivegui/src/ui`
- Test: `crates/hivegui/tests/integration_test.rs`

**Interfaces:**
- Consumes: all migrated management views.
- Produces: automated light/dark theme evidence and final repository verification evidence.

- [ ] **Step 1: Re-run the live-theme regression created in Task 1**

Run:

```bash
cargo test -p hivegui --lib ui::management_style::tests::management_style_recomputes_after_theme_mode_changes -- --exact
```

Expected: PASS, proving a fresh `ManagementStyle` reflects the active light and dark theme values.

- [ ] **Step 2: Ensure every view computes style during render**

Search for persistent style storage and remove it:

```bash
rg -n "ManagementStyle" crates/hivegui/src/ui
```

Expected: `ManagementStyle` appears in imports, render functions, render-helper parameters, and tests, but not as a field on any view struct. Make every render entry point call `ManagementStyle::current(cx)` or receive a value computed during that render.

- [ ] **Step 3: Run focused and complete verification**

Run these commands in order and read each exit code:

```bash
cargo test -p hivegui --lib ui::management_style::tests
cargo test -p hivegui --lib ui::global_config::tests
cargo test -p hivegui --lib ui::llm_config::tests
cargo test -p hivegui --lib ui::settings_view::tests
cargo test -p hivegui
cargo fmt --package hivegui -- --check
git diff --check -- crates/hivegui/src/ui
rg -n "0x6f5699" crates/hivegui/src/ui
```

Expected:

- Every targeted and full Cargo test exits 0.
- `cargo fmt --package hivegui -- --check` and the UI-scoped `git diff --check` exit 0.
- The final `rg` prints no matches and exits 1 because the legacy purple is absent.

- [ ] **Step 4: Perform the visual acceptance pass**

Run:

```bash
cargo run -p hivegui
```

In Default Light and Default Dark, inspect Category, Global Configuration, LLM Model/Provider, one additional CRUD page, Data Source, database table viewer, and Data Management. Confirm main actions use the theme info color, semantic actions remain distinguishable, every list has the reference header/row geometry, fixed white surfaces are absent in dark mode, and the provider Category menu still scrolls independently of its modal.

- [ ] **Step 5: Record the safe completion checkpoint**

Run:

```bash
git status --short
git diff --stat -- crates/hivegui/src/ui
```

Expected: only intended management UI files plus pre-existing user changes are present. Do not stage or commit in this dirty checkout unless the user explicitly requests it.
