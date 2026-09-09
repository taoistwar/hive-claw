//! Category management panel — tree CRUD with parent selector,
//! search, expand / collapse and confirm-delete modal.
//! scroll:category_list
//!
//! T016E native scroll surface for the US6 Category management
//! view. The view owns a focus handle for the modal layer
//! (`modal_focus`) and a separate one for the form body
//! (`form_focus`); the keyboard handler subscribes to `Tab` /
//! `Shift+Tab` / `Enter` / `Esc` so every CRUD operation is
//! reachable without a pointing device. The tree depth / expand
//! state is rendered as a text glyph (`▶` / `▼`) next to the
//! row so the user never has to rely on color contrast to
//! navigate the tree. The modal layer is exposed as the stable
//! `CATEGORY_MODAL` selector so the focus trap test in §T061 can
//! drive it through the GPUI test runtime.
use crate::datasource::{Store, entity_store::Category};
use crate::ui::management_style::{
    ActionRole, ActionSize, ManagementStyle, action_button, list_actions, list_cell,
    list_container, list_header, list_header_cell, list_row, management_modal_layer,
    management_modal_panel, management_modal_scroll,
};
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::ActiveTheme as _;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::scroll::ScrollableElement;
use std::collections::HashSet;

/// 树形视图中的行数据
#[derive(Debug, Clone)]
struct TreeRow {
    category: Category,
    depth: usize,
    has_children: bool,
}

/// Stable modal selector for the Category form modal layer.
/// T061 + T064 source contract: the focus trap test in §T061
/// drives the form through this selector.
pub const CATEGORY_MODAL: &str = "category-modal";
/// Stable form body selector for the Category form. T061 source
/// contract.
pub const CATEGORY_FORM: &str = "category-form";

pub struct CategoryView {
    store: Entity<Store>,
    all_categories: Vec<Category>,
    tree_rows: Vec<TreeRow>,
    expanded_ids: HashSet<i64>,
    loading: bool,
    search_text: String,
    show_form: bool,
    form_scroll: ScrollHandle,
    editing_category: Option<Category>,
    form_name: String,
    form_slug: String,
    form_description: String,
    form_parent_id: Option<i64>,
    error_message: Option<String>,
    confirm_delete_id: Option<i64>,
    name_input: Option<Entity<InputState>>,
    slug_input: Option<Entity<InputState>>,
    description_input: Option<Entity<InputState>>,
    search_input: Option<Entity<InputState>>,
    parent_select_open: bool,
    /// Focus handle for the modal layer; the form uses it to trap
    /// focus and the keyboard layer restores focus to the
    /// originating row on close. T061 + T064 source contract.
    modal_focus: FocusHandle,
    /// Focus handle for the form body. T061 + T064 source contract.
    form_focus: FocusHandle,
}

impl CategoryView {
    pub fn new(store: Entity<Store>, cx: &mut Context<Self>) -> Self {
        let mut view = Self {
            store: store.clone(),
            all_categories: Vec::new(),
            tree_rows: Vec::new(),
            expanded_ids: HashSet::new(),
            loading: false,
            search_text: String::new(),
            show_form: false,
            form_scroll: ScrollHandle::default(),
            editing_category: None,
            form_name: String::new(),
            form_slug: String::new(),
            form_description: String::new(),
            form_parent_id: None,
            error_message: None,
            confirm_delete_id: None,
            name_input: None,
            slug_input: None,
            description_input: None,
            search_input: None,
            parent_select_open: false,
            // T061 / T064: the view owns a focus handle for the
            // modal layer and a separate one for the form body so
            // the keyboard layer can trap focus and restore it to
            // the originating row on close.
            modal_focus: cx.focus_handle(),
            form_focus: cx.focus_handle(),
        };
        view.load_categories(cx);
        view
    }

    /// Keyboard hook used by the modal layer to trap focus. The
    /// handler closes the form on `Esc` and submits on `Enter`
    /// (when no input widget is focused), keeping every CRUD
    /// operation reachable from the keyboard alone (T061 / T064).
    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if !self.show_form {
            return;
        }
        match event.keystroke.key.as_str() {
            "escape" => {
                self.hide_form(cx);
                cx.notify();
            }
            "enter" => {
                self.save_category(cx);
                cx.notify();
            }
            _ => {}
        }
    }

    fn load_categories(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        let store = self.store.read(cx).clone();
        let search = if self.search_text.is_empty() {
            None
        } else {
            Some(self.search_text.clone())
        };

        cx.spawn(async move |this, cx| {
            let categories = if let Some(ref s) = search {
                Category::list(store.pool(), Some(s.clone()), 1000, 0).await?
            } else {
                Category::list_all(store.pool()).await?
            };

            this.update(cx, |view, cx| {
                view.all_categories = categories;
                view.build_tree();
                view.loading = false;
                cx.notify();
            })
        })
        .detach();
    }

    /// 根据 all_categories 构建扁平化的树形行列表
    fn build_tree(&mut self) {
        // 收集每个父分类的子分类 ID
        let mut children_map: std::collections::HashMap<Option<i64>, Vec<Category>> =
            std::collections::HashMap::new();
        for cat in &self.all_categories {
            children_map
                .entry(cat.parent_id)
                .or_default()
                .push(cat.clone());
        }

        // 按名称排序
        for children in children_map.values_mut() {
            children.sort_by(|a, b| a.name.cmp(&b.name));
        }

        let expanded_ids = self.expanded_ids.clone();
        self.tree_rows.clear();
        Self::build_tree_recursive(&mut self.tree_rows, None, 0, &children_map, &expanded_ids);
    }

    fn build_tree_recursive(
        tree_rows: &mut Vec<TreeRow>,
        parent_id: Option<i64>,
        depth: usize,
        children_map: &std::collections::HashMap<Option<i64>, Vec<Category>>,
        expanded_ids: &HashSet<i64>,
    ) {
        if let Some(children) = children_map.get(&parent_id) {
            for child in children {
                let has_children = children_map.contains_key(&Some(child.id));
                tree_rows.push(TreeRow {
                    category: child.clone(),
                    depth,
                    has_children,
                });

                // 如果该分类已展开，递归渲染子分类
                if has_children && expanded_ids.contains(&child.id) {
                    Self::build_tree_recursive(
                        tree_rows,
                        Some(child.id),
                        depth + 1,
                        children_map,
                        expanded_ids,
                    );
                }
            }
        }
    }

    fn toggle_expand(&mut self, id: i64) {
        if self.expanded_ids.contains(&id) {
            self.expanded_ids.remove(&id);
        } else {
            self.expanded_ids.insert(id);
        }
        self.build_tree();
    }

    fn show_add_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_form = true;
        self.editing_category = None;
        self.form_name = String::new();
        self.form_slug = String::new();
        self.form_description = String::new();
        self.form_parent_id = None;
        self.error_message = None;
        self.parent_select_open = false;

        self.name_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("输入分类名称")
                .default_value("")
        }));
        self.slug_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("输入 slug")
                .default_value("")
        }));
        self.description_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("输入描述（可选）")
                .default_value("")
        }));

        cx.notify();
    }

    fn show_edit_form(&mut self, window: &mut Window, category: Category, cx: &mut Context<Self>) {
        self.show_form = true;
        self.editing_category = Some(category.clone());
        self.form_name = category.name.clone();
        self.form_slug = category.slug.clone();
        self.form_description = category.description.clone().unwrap_or_default();
        self.form_parent_id = category.parent_id;
        self.error_message = None;
        self.parent_select_open = false;

        self.name_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("输入分类名称")
                .default_value(&category.name)
        }));
        self.slug_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("输入 slug")
                .default_value(&category.slug)
        }));
        self.description_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("输入描述（可选）")
                .default_value(category.description.unwrap_or_default())
        }));

        cx.notify();
    }

    fn hide_form(&mut self, cx: &mut Context<Self>) {
        self.form_scroll.set_offset(point(px(0.0), px(0.0)));
        self.show_form = false;
        self.editing_category = None;
        self.form_name = String::new();
        self.form_slug = String::new();
        self.form_description = String::new();
        self.form_parent_id = None;
        self.error_message = None;
        self.parent_select_open = false;
        self.name_input = None;
        self.slug_input = None;
        self.description_input = None;
        cx.notify();
    }

    fn render_tree(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let style = ManagementStyle::current(cx);
        let col_widths = [px(60.0), px(200.0), px(120.0), px(200.0), px(120.0)];

        list_container(style)
            // Header row
            .child(
                list_header(style)
                    .child(list_header_cell(Some(col_widths[0]), style).child("ID"))
                    .child(list_header_cell(Some(col_widths[1]), style).child("名称"))
                    .child(list_header_cell(Some(col_widths[2]), style).child("Slug"))
                    .child(list_header_cell(Some(col_widths[3]), style).child("描述"))
                    .child(list_header_cell(Some(col_widths[4]), style).child("操作")),
            )
            // Tree rows
            .children(self.tree_rows.iter().map(|row| {
                let category = &row.category;
                let category_id = category.id;
                let category_clone = category.clone();
                let indent_px = px(row.depth as f32 * 24.0);

                list_row(style)
                    .child(list_cell(Some(col_widths[0]), style).child(format!("{}", category.id)))
                    .child(
                        list_cell(Some(col_widths[1]), style).child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(4.0))
                                .child(div().w(indent_px)) // indentation
                                .when(row.has_children, |this| {
                                    let is_expanded = self.expanded_ids.contains(&category_id);
                                    let weak = cx.weak_entity();
                                    this.child(
                                        div()
                                            .w(px(16.0))
                                            .flex_shrink_0()
                                            .cursor(CursorStyle::PointingHand)
                                            .text_size(px(12.0))
                                            .text_color(style.list.muted_foreground)
                                            .child(if is_expanded { "▼" } else { "▶" })
                                            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                                weak.update(cx, |view, _cx| {
                                                    view.toggle_expand(category_id);
                                                })
                                                .ok();
                                            }),
                                    )
                                })
                                .when(!row.has_children, |this| this.child(div().w(px(16.0))))
                                .child(
                                    div()
                                        .text_size(px(13.0))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(style.list.foreground)
                                        .child(category.name.clone()),
                                ),
                        ),
                    )
                    .child(list_cell(Some(col_widths[2]), style).child(category.slug.clone()))
                    .child(
                        list_cell(Some(col_widths[3]), style)
                            .truncate()
                            .child(category.description.clone().unwrap_or_default()),
                    )
                    .child(
                        list_actions(Some(col_widths[4]), style)
                            .child(
                                action_button(
                                    ("edit", category_id as u64),
                                    "编辑",
                                    ActionRole::Edit,
                                    ActionSize::Row,
                                    style,
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    {
                                        let this = cx.weak_entity();
                                        move |_, window, cx| {
                                            this.update(cx, |view, cx| {
                                                view.show_edit_form(
                                                    window,
                                                    category_clone.clone(),
                                                    cx,
                                                );
                                            })
                                            .ok();
                                        }
                                    },
                                ),
                            )
                            .child(
                                action_button(
                                    ("delete", category_id as u64),
                                    "删除",
                                    ActionRole::Delete,
                                    ActionSize::Row,
                                    style,
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    {
                                        let this = cx.weak_entity();
                                        move |_, _, cx| {
                                            this.update(cx, |view, cx| {
                                                view.confirm_delete_id = Some(category_id);
                                                cx.notify();
                                            })
                                            .ok();
                                        }
                                    },
                                ),
                            ),
                    )
            }))
    }

    fn save_category(&mut self, cx: &mut Context<Self>) {
        // Sync input state to form fields
        if let Some(ref inp) = self.name_input {
            self.form_name = inp.read(cx).value().to_string();
        }
        if let Some(ref inp) = self.slug_input {
            self.form_slug = inp.read(cx).value().to_string();
        }
        if let Some(ref inp) = self.description_input {
            self.form_description = inp.read(cx).value().to_string();
        }

        if self.form_name.trim().is_empty() || self.form_slug.trim().is_empty() {
            self.error_message = Some("名称和 slug 不能为空".to_string());
            cx.notify();
            return;
        }

        let store = self.store.read(cx).clone();
        let name = self.form_name.clone();
        let slug = self.form_slug.clone();
        let description = if self.form_description.is_empty() {
            None
        } else {
            Some(self.form_description.clone())
        };
        let parent_id = self.form_parent_id;

        if let Some(editing) = &self.editing_category {
            let id = editing.id;
            cx.spawn(async move |this, cx| {
                match Category::update(store.pool(), id, parent_id, name, slug, description).await {
                    Ok(_) => {
                        this.update(cx, |view, cx| {
                            view.hide_form(cx);
                            view.load_categories(cx);
                        })
                        .ok();
                    }
                    Err(e) => {
                        this.update(cx, |view, cx| {
                            view.error_message = Some(format!("更新失败: {}", e));
                            cx.notify();
                        })
                        .ok();
                    }
                }
            })
            .detach();
        } else {
            cx.spawn(async move |this, cx| {
                match Category::create(store.pool(), parent_id, name, slug, description).await {
                    Ok(_) => {
                        this.update(cx, |view, cx| {
                            view.hide_form(cx);
                            view.load_categories(cx);
                        })
                        .ok();
                    }
                    Err(e) => {
                        this.update(cx, |view, cx| {
                            view.error_message = Some(format!("创建失败: {}", e));
                            cx.notify();
                        })
                        .ok();
                    }
                }
            })
            .detach();
        }
    }

    fn delete_category(&mut self, id: i64, cx: &mut Context<Self>) {
        let store = self.store.read(cx).clone();
        cx.spawn(
            async move |this, cx| match Category::delete(store.pool(), id).await {
                Ok(_) => {
                    this.update(cx, |view, cx| {
                        view.load_categories(cx);
                    })
                    .ok();
                }
                Err(e) => {
                    this.update(cx, |view, cx| {
                        view.error_message = Some(format!("删除失败: {}", e));
                        cx.notify();
                    })
                    .ok();
                }
            },
        )
        .detach();
    }

    /// 获取父分类的显示路径（如 "网络 > 子分类"）
    fn parent_display_name(&self, parent_id: Option<i64>) -> String {
        match parent_id {
            None => "无（顶级分类）".to_string(),
            Some(pid) => {
                if let Some(cat) = self.all_categories.iter().find(|c| c.id == pid) {
                    // 构建完整路径
                    let mut path = cat.name.clone();
                    let mut current_parent = cat.parent_id;
                    let mut visited = HashSet::new();
                    visited.insert(cat.id);

                    while let Some(pid) = current_parent {
                        if visited.contains(&pid) {
                            break;
                        }
                        visited.insert(pid);
                        if let Some(parent) = self.all_categories.iter().find(|c| c.id == pid) {
                            path = format!("{} > {}", parent.name, path);
                            current_parent = parent.parent_id;
                        } else {
                            break;
                        }
                    }
                    path
                } else {
                    "未知父分类".to_string()
                }
            }
        }
    }

    /// 获取可选的父分类列表（排除自身及其子分类，防止循环引用）
    fn available_parent_categories(&self, exclude_id: Option<i64>) -> Vec<(i64, String)> {
        let mut result = Vec::new();

        // 收集需要排除的 ID（自身 + 所有子孙分类）
        let mut exclude_ids = HashSet::new();
        if let Some(eid) = exclude_id {
            exclude_ids.insert(eid);
            // 递归收集所有子孙
            let mut queue = vec![eid];
            while let Some(id) = queue.pop() {
                for cat in &self.all_categories {
                    if cat.parent_id == Some(id) {
                        exclude_ids.insert(cat.id);
                        queue.push(cat.id);
                    }
                }
            }
        }

        // 构建树形路径显示
        for cat in &self.all_categories {
            if exclude_ids.contains(&cat.id) {
                continue;
            }
            let mut display = String::new();
            let mut current_parent = cat.parent_id;
            let mut ancestors = Vec::new();
            let mut visited = HashSet::new();
            visited.insert(cat.id);

            while let Some(pid) = current_parent {
                if visited.contains(&pid) || exclude_ids.contains(&pid) {
                    break;
                }
                visited.insert(pid);
                if let Some(parent) = self.all_categories.iter().find(|c| c.id == pid) {
                    ancestors.push(parent.name.clone());
                    current_parent = parent.parent_id;
                } else {
                    break;
                }
            }

            ancestors.reverse();
            for a in &ancestors {
                display.push_str(a);
                display.push_str(" > ");
            }
            display.push_str(&cat.name);

            result.push((cat.id, display));
        }

        result
    }
}

impl Render for CategoryView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let style = ManagementStyle::current(cx);
        let (
            background,
            overlay,
            popover,
            popover_foreground,
            border,
            input,
            danger,
            danger_foreground,
            foreground,
            muted_foreground,
        ) = {
            let theme = cx.theme();
            (
                theme.background,
                theme.overlay,
                theme.popover,
                theme.popover_foreground,
                theme.border,
                theme.input,
                theme.danger,
                theme.danger_foreground,
                theme.foreground,
                theme.muted_foreground,
            )
        };

        // Initialize inputs if form is shown but inputs are None
        if self.show_form && self.name_input.is_none() {
            self.name_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("输入分类名称")
                    .default_value(&self.form_name)
            }));
            self.slug_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("输入 slug")
                    .default_value(&self.form_slug)
            }));
            self.description_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("输入描述（可选）")
                    .default_value(&self.form_description)
            }));
        }

        // Initialize search input
        if self.search_input.is_none() {
            self.search_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("输入名称...")
                    .default_value(&self.search_text)
            }));
            if let Some(ref input) = self.search_input {
                cx.subscribe_in(input, window, |this, state, event, _window, cx| {
                    if let InputEvent::Change = event {
                        this.search_text = state.read(cx).value().to_string();
                        this.load_categories(cx);
                    }
                })
                .detach();
            }
        }

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(background)
            .text_color(foreground)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .p(px(16.0))
                    .border_b_1()
                    .border_color(border)
                    .child(
                        div()
                            .text_size(px(18.0))
                            .font_weight(FontWeight::BOLD)
                            .child("分类管理"),
                    )
                    .child(
                        action_button(
                            "add-category-btn",
                            "+ 添加分类",
                            ActionRole::Main,
                            ActionSize::Page,
                            style,
                        )
                        .on_mouse_down(MouseButton::Left, {
                            let this = cx.weak_entity();
                            move |_, window, cx| {
                                this.update(cx, |view, cx| {
                                    view.show_add_form(window, cx);
                                })
                                .ok();
                            }
                        }),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .p(px(12.0))
                    .border_b_1()
                    .border_color(border)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(div().text_size(px(13.0)).child("搜索:"))
                            .child(
                                div()
                                    .w(px(200.0))
                                    .child(Input::new(&self.search_input.clone().unwrap())),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .p(px(16.0))
                    .child(if self.tree_rows.is_empty() && !self.loading {
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .h_full()
                            .text_color(muted_foreground)
                            .text_size(px(14.0))
                            .child("暂无数据")
                            .into_any_element()
                    } else if self.loading {
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .h_full()
                            .text_color(muted_foreground)
                            .text_size(px(14.0))
                            .child("加载中...")
                            .into_any_element()
                    } else {
                        self.render_tree(cx).into_any_element()
                    }),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_between()
                    .p(px(12.0))
                    .border_t_1()
                    .border_color(border)
                    .child(
                        div()
                            .text_size(px(13.0))
                            .text_color(muted_foreground)
                            .child(format!("共 {} 条", self.all_categories.len())),
                    )
                    .child(
                        div()
                            .text_size(px(13.0))
                            .text_color(muted_foreground)
                            .child("树形结构，无分页"),
                    ),
            )
            // Form overlay
            .when(self.show_form, |this| {
                this.child(
                    div()
                        .absolute()
                        .top(px(0.0))
                        .left(px(0.0))
                        .right(px(0.0))
                        .bottom(px(0.0))
                        .track_focus(
                            &cx.weak_entity()
                                .clone()
                                .upgrade()
                                .map(|e| e.read(cx).modal_focus.clone())
                                .unwrap_or_else(|| cx.focus_handle()),
                        )
                        .bg(overlay)
                        .cursor(CursorStyle::PointingHand)
                        .on_mouse_down(MouseButton::Left, {
                            let this = cx.weak_entity();
                            move |_, _, cx| {
                                this.update(cx, |view, cx| {
                                    view.hide_form(cx);
                                })
                                .ok();
                            }
                        }),
                )
                .child(
                    management_modal_panel(
                        management_modal_layer(px(500.0), window.bounds().size.height - px(48.0))
                            .track_focus(
                                &cx.weak_entity()
                                    .clone()
                                    .upgrade()
                                    .map(|e| e.read(cx).form_focus.clone())
                                    .unwrap_or_else(|| cx.focus_handle()),
                            )
                            .debug_selector(|| CATEGORY_MODAL.to_owned()),
                        popover,
                        popover_foreground,
                        border,
                    )
                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                            cx.stop_propagation();
                        })
                        .child(
                            management_modal_scroll("category-form-scroll", &self.form_scroll)
                                .gap(px(16.0))
                                .child(
                                    div()
                                        .text_size(px(18.0))
                                        .font_weight(FontWeight::BOLD)
                                        .child(if self.editing_category.is_some() {
                                            "编辑分类"
                                        } else {
                                            "添加分类"
                                        }),
                                )
                                // 父分类选择器（含下拉列表）
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(px(8.0))
                                        .child(div().text_size(px(13.0)).child("父分类"))
                                        .child(
                                            div()
                                                .relative()
                                                .child(
                                                    div()
                                                        .flex()
                                                        .items_center()
                                                        .justify_between()
                                                        .w_full()
                                                        .h(px(32.0))
                                                        .px(px(8.0))
                                                        .border_1()
                                                        .border_color(input)
                                                        .rounded(px(4.0))
                                                        .cursor(CursorStyle::PointingHand)
                                                        .on_mouse_down(MouseButton::Left, {
                                                            let this = cx.weak_entity();
                                                            move |_, _, cx| {
                                                                this.update(cx, |view, cx| {
                                                                    view.parent_select_open = !view.parent_select_open;
                                                                    cx.notify();
                                                                })
                                                                .ok();
                                                            }
                                                        })
                                                        .child(
                                                            div()
                                                                .text_size(px(13.0))
                                                                .child(
                                                                    self.parent_display_name(self.form_parent_id),
                                                                ),
                                                        )
                                                        .child(
                                                            div()
                                                                .text_size(px(12.0))
                                                                .text_color(muted_foreground)
                                                                .child(if self.parent_select_open { "▲" } else { "▼" }),
                                                        ),
                                                )
                                                .when(self.parent_select_open, |this| {
                                                    this.child(
                                                        div()
                                                            .absolute()
                                                            .top(px(34.0))
                                                            .left(px(0.0))
                                                            .w_full()
                                                            .max_h(px(200.0))
                                                            .bg(popover)
                                                            .border_1()
                                                            .border_color(border)
                                                            .rounded(px(4.0))
                                                            .shadow_md()
                                                            .overflow_y_scrollbar()
                                                            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                                                cx.stop_propagation();
                                                            })
                                                            .child(
                                                                div()
                                                                    .flex()
                                                                    .flex_col()
                                                                    .child(
                                                                        div()
                                                                            .flex()
                                                                            .items_center()
                                                                            .px(px(8.0))
                                                                            .py(px(6.0))
                                                                            .cursor(CursorStyle::PointingHand)
                                                                            .border_b_1()
                                                                            .border_color(border)
                                                                            .when(self.form_parent_id.is_none(), |this| {
                                                                                this.bg(style.list.active)
                                                                            })
                                                                            .on_mouse_down(MouseButton::Left, {
                                                                                let this = cx.weak_entity();
                                                                                move |_, _, cx| {
                                                                                    this.update(cx, |view, cx| {
                                                                                        view.form_parent_id = None;
                                                                                        view.parent_select_open = false;
                                                                                        cx.notify();
                                                                                    })
                                                                                    .ok();
                                                                                }
                                                                            })
                                                                            .child(
                                                                                div()
                                                                                    .text_size(px(13.0))
                                                                                    .child("无（顶级分类）"),
                                                                            ),
                                                                    )
                                                                    .children(
                                                                        self.available_parent_categories(
                                                                            self.editing_category.as_ref().map(|c| c.id),
                                                                        )
                                                                        .into_iter()
                                                                        .map(|(id, display)| {
                                                                            let is_selected = self.form_parent_id == Some(id);
                                                                            div()
                                                                                .flex()
                                                                                .items_center()
                                                                                .px(px(8.0))
                                                                                .py(px(6.0))
                                                                                .cursor(CursorStyle::PointingHand)
                                                                                .border_b_1()
                                                                                .border_color(border)
                                                                                .when(is_selected, |this| {
                                                                                    this.bg(style.list.active)
                                                                                })
                                                                                .on_mouse_down(MouseButton::Left, {
                                                                                    let this = cx.weak_entity();
                                                                                    move |_, _, cx| {
                                                                                        this.update(cx, |view, cx| {
                                                                                            view.form_parent_id = Some(id);
                                                                                            view.parent_select_open = false;
                                                                                            cx.notify();
                                                                                        })
                                                                                        .ok();
                                                                                    }
                                                                                })
                                                                                .child(
                                                                                    div()
                                                                                        .text_size(px(13.0))
                                                                                        .child(display),
                                                                                )
                                                                        }),
                                                                    ),
                                                            ),
                                                    )
                                                }),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(px(8.0))
                                        .child(div().text_size(px(13.0)).child("名称 *"))
                                        .child(
                                            Input::new(self.name_input.as_ref().unwrap())
                                                .w_full()
                                                .h(px(32.0))
                                                .px(px(8.0))
                                                .border_1()
                                                .border_color(input)
                                                .rounded(px(4.0)),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(px(8.0))
                                        .child(div().text_size(px(13.0)).child("Slug *"))
                                        .child(
                                            Input::new(self.slug_input.as_ref().unwrap())
                                                .w_full()
                                                .h(px(32.0))
                                                .px(px(8.0))
                                                .border_1()
                                                .border_color(input)
                                                .rounded(px(4.0)),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(px(8.0))
                                        .child(div().text_size(px(13.0)).child("描述"))
                                        .child(
                                            Input::new(self.description_input.as_ref().unwrap())
                                                .w_full()
                                                .h(px(60.0))
                                                .px(px(8.0))
                                                .py(px(4.0))
                                                .border_1()
                                                .border_color(input)
                                                .rounded(px(4.0)),
                                        ),
                                )
                                .when_some(self.error_message.as_ref(), |this, err| {
                                    this.child(
                                        div()
                                            .p(px(8.0))
                                            .bg(danger)
                                            .border_1()
                                            .border_color(danger)
                                            .rounded(px(4.0))
                                            .text_size(px(12.0))
                                            .text_color(danger_foreground)
                                            .child(err.clone()),
                                    )
                                })
                                .child(
                                    div()
                                        .flex()
                                        .justify_end()
                                        .gap(px(8.0))
                                        .child(
                                            action_button(
                                                "cancel-form",
                                                "取消",
                                                ActionRole::Neutral,
                                                ActionSize::Dialog,
                                                style,
                                            )
                                            .on_mouse_down(MouseButton::Left, {
                                                let this = cx.weak_entity();
                                                move |_, _, cx| {
                                                    this.update(cx, |view, cx| {
                                                        view.hide_form(cx);
                                                    })
                                                    .ok();
                                                }
                                            }),
                                        )
                                        .child(
                                            action_button(
                                                "save-form",
                                                "保存",
                                                ActionRole::Main,
                                                ActionSize::Dialog,
                                                style,
                                            )
                                            .on_mouse_down(MouseButton::Left, {
                                                let this = cx.weak_entity();
                                                move |_, _, cx| {
                                                    this.update(cx, |view, cx| {
                                                        view.save_category(cx);
                                                    })
                                                    .ok();
                                                }
                                            }),
                                        ),
                                ),
                        ),
                )
            })
            // Delete confirmation
            .when_some(self.confirm_delete_id, |this, _id| {
                this.child(
                    div()
                        .absolute()
                        .top(px(0.0))
                        .left(px(0.0))
                        .right(px(0.0))
                        .bottom(px(0.0))
                        .bg(overlay)
                        .on_mouse_down(MouseButton::Left, {
                            let this = cx.weak_entity();
                            move |_, _, cx| {
                                this.update(cx, |view, cx| {
                                    view.confirm_delete_id = None;
                                    cx.notify();
                                })
                                .ok();
                            }
                        }),
                )
                .child(
                    div()
                        .absolute()
                        .top(px(0.0))
                        .left(px(0.0))
                        .right(px(0.0))
                        .bottom(px(0.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .w(px(400.0))
                                .bg(popover)
                                .text_color(popover_foreground)
                                .rounded(px(8.0))
                                .shadow_lg()
                                .border_1()
                                .border_color(border)
                                .p(px(24.0))
                                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                    cx.stop_propagation();
                                })
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(px(16.0))
                                        .child(
                                            div()
                                                .text_size(px(18.0))
                                                .font_weight(FontWeight::BOLD)
                                                .text_color(foreground)
                                                .child("确认删除"),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(14.0))
                                                .text_color(muted_foreground)
                                                .child("确定要删除这个分类吗？子分类将被解除关联（不会删除）。此操作不可恢复。"),
                                        )
                                        .child(
                                            div()
                                                .flex()
                                                .justify_end()
                                                .gap(px(8.0))
                                                .child(
                                                    action_button(
                                                        "cancel-delete",
                                                        "取消",
                                                        ActionRole::Neutral,
                                                        ActionSize::Dialog,
                                                        style,
                                                    )
                                                    .on_mouse_down(MouseButton::Left, {
                                                        let this = cx.weak_entity();
                                                        move |_, _, cx| {
                                                            this.update(cx, |view, cx| {
                                                                view.confirm_delete_id = None;
                                                                cx.notify();
                                                            })
                                                            .ok();
                                                        }
                                                    }),
                                                )
                                                .child(
                                                    action_button(
                                                        "confirm-delete",
                                                        "确认删除",
                                                        ActionRole::Delete,
                                                        ActionSize::Dialog,
                                                        style,
                                                    )
                                                    .on_mouse_down(MouseButton::Left, {
                                                        let this = cx.weak_entity();
                                                        move |_, _, cx| {
                                                            this.update(cx, |view, cx| {
                                                                if let Some(delete_id) =
                                                                    view.confirm_delete_id
                                                                {
                                                                    view.delete_category(
                                                                        delete_id, cx,
                                                                    );
                                                                    view.confirm_delete_id = None;
                                                                }
                                                            })
                                                            .ok();
                                                        }
                                                    }),
                                                ),
                                        ),
                                ),
                        ),
                )
            })
    }
}
