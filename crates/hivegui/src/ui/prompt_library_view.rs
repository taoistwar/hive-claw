//! 提示词管理面板 —— 可复用提示词的扁平 CRUD（搜索 / 新增 / 编辑 / 删除）。
//!
//! 这里维护的是「提示词文本」本身：调试页可以把某条提示词的内容填进 message，
//! 也可以完全不用库里的内容直接手输（见 `prompt_debugger::PromptDebugger`
//! 的提示词选择器）。视图只依赖 `Store`，因此桌面版与 Ngy Prompt Studio 的同一
//! 份代码各自读写自己的数据根。
//!
//! 稳定选择器（测试用）：`PROMPT_*`。
use crate::datasource::Store;
use crate::datasource::entity_store::PromptTemplate;
use crate::ui::management_style::{
    ActionRole, ActionSize, ManagementStyle, action_button, list_actions, list_cell,
    list_container, list_header, list_header_cell, list_row, management_modal_layer,
    management_modal_panel, management_modal_scroll,
};
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

/// 表单弹窗层（遮罩 + 面板）的选择器。
pub const PROMPT_MODAL: &str = "PROMPT_MODAL";
/// 表单正文滚动区的选择器。
pub const PROMPT_FORM_SCROLL: &str = "PROMPT_FORM_SCROLL";
/// 列表加载完成的选择器（只在有数据时渲染），测试用它等待异步加载。
pub const PROMPT_LIST_LOADED: &str = "PROMPT_LIST_LOADED";

/// 单页最多展示的提示词条数（搜索同样受此上限约束）。
const PAGE_LIMIT: i64 = 200;

pub struct PromptLibraryView {
    store: Entity<Store>,
    prompts: Vec<PromptTemplate>,
    total_count: i64,
    loading: bool,
    search_text: String,
    show_form: bool,
    form_scroll: ScrollHandle,
    editing_prompt: Option<PromptTemplate>,
    form_name: String,
    form_content: String,
    form_description: String,
    error_message: Option<String>,
    confirm_delete_id: Option<i64>,
    name_input: Option<Entity<InputState>>,
    content_input: Option<Entity<TextareaState>>,
    description_input: Option<Entity<InputState>>,
    search_input: Option<Entity<InputState>>,
}

impl PromptLibraryView {
    pub fn new(store: Entity<Store>, cx: &mut Context<Self>) -> Self {
        let mut view = Self {
            store,
            prompts: Vec::new(),
            total_count: 0,
            loading: false,
            search_text: String::new(),
            show_form: false,
            form_scroll: ScrollHandle::default(),
            editing_prompt: None,
            form_name: String::new(),
            form_content: String::new(),
            form_description: String::new(),
            error_message: None,
            confirm_delete_id: None,
            name_input: None,
            content_input: None,
            description_input: None,
            search_input: None,
        };
        view.load_prompts(cx);
        view
    }

    /// 按当前搜索词重新读取提示词列表。
    pub fn load_prompts(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        let store = self.store.read(cx).clone();
        let search = if self.search_text.trim().is_empty() {
            None
        } else {
            Some(self.search_text.trim().to_string())
        };

        cx.spawn(async move |this, cx| {
            let prompts = PromptTemplate::list(store.pool(), search.clone(), PAGE_LIMIT, 0)
                .await
                .unwrap_or_default();
            let total = PromptTemplate::count(store.pool(), search)
                .await
                .unwrap_or(prompts.len() as i64);
            _ = this.update(cx, |view, cx| {
                view.prompts = prompts;
                view.total_count = total;
                view.loading = false;
                cx.notify();
            });
        })
        .detach();
    }

    fn show_add_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_form = true;
        self.editing_prompt = None;
        self.form_name = String::new();
        self.form_content = String::new();
        self.form_description = String::new();
        self.error_message = None;
        self.rebuild_form_inputs(window, cx);
        cx.notify();
    }

    fn show_edit_form(
        &mut self,
        window: &mut Window,
        prompt: PromptTemplate,
        cx: &mut Context<Self>,
    ) {
        self.show_form = true;
        self.form_name = prompt.name.clone();
        self.form_content = prompt.content.clone();
        self.form_description = prompt.description.clone().unwrap_or_default();
        self.editing_prompt = Some(prompt);
        self.error_message = None;
        self.rebuild_form_inputs(window, cx);
        cx.notify();
    }

    fn rebuild_form_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.form_name.clone();
        let content = self.form_content.clone();
        let description = self.form_description.clone();
        self.name_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("输入提示词名称")
                .default_value(&name)
        }));
        self.content_input = Some(cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("输入提示词内容")
                .default_value(&content)
                .auto_grow(4, 16)
        }));
        self.description_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("输入描述（可选）")
                .default_value(&description)
        }));
    }

    fn hide_form(&mut self, cx: &mut Context<Self>) {
        self.form_scroll.set_offset(point(px(0.0), px(0.0)));
        self.show_form = false;
        self.editing_prompt = None;
        self.form_name = String::new();
        self.form_content = String::new();
        self.form_description = String::new();
        self.error_message = None;
        self.name_input = None;
        self.content_input = None;
        self.description_input = None;
        cx.notify();
    }

    fn save_prompt(&mut self, cx: &mut Context<Self>) {
        if let Some(ref input) = self.name_input {
            self.form_name = input.read(cx).value().to_string();
        }
        if let Some(ref input) = self.content_input {
            self.form_content = input.read(cx).value().to_string();
        }
        if let Some(ref input) = self.description_input {
            self.form_description = input.read(cx).value().to_string();
        }

        if self.form_name.trim().is_empty() {
            self.error_message = Some("名称不能为空".to_string());
            cx.notify();
            return;
        }
        if self.form_content.trim().is_empty() {
            self.error_message = Some("内容不能为空".to_string());
            cx.notify();
            return;
        }

        let store = self.store.read(cx).clone();
        let name = self.form_name.trim().to_string();
        let content = self.form_content.clone();
        let description = if self.form_description.trim().is_empty() {
            None
        } else {
            Some(self.form_description.trim().to_string())
        };

        if let Some(editing) = self.editing_prompt.clone() {
            let id = editing.id;
            cx.spawn(async move |this, cx| {
                match PromptTemplate::update(store.pool(), id, name, content, description).await {
                    Ok(_) => {
                        _ = this.update(cx, |view, cx| {
                            view.hide_form(cx);
                            view.load_prompts(cx);
                        });
                    }
                    Err(error) => {
                        _ = this.update(cx, |view, cx| {
                            view.error_message = Some(format!("更新失败: {error}"));
                            cx.notify();
                        });
                    }
                }
            })
            .detach();
        } else {
            cx.spawn(async move |this, cx| {
                match PromptTemplate::create(store.pool(), name, content, description).await {
                    Ok(_) => {
                        _ = this.update(cx, |view, cx| {
                            view.hide_form(cx);
                            view.load_prompts(cx);
                        });
                    }
                    Err(error) => {
                        _ = this.update(cx, |view, cx| {
                            view.error_message = Some(format!("创建失败: {error}"));
                            cx.notify();
                        });
                    }
                }
            })
            .detach();
        }
    }

    fn delete_prompt(&mut self, id: i64, cx: &mut Context<Self>) {
        let store = self.store.read(cx).clone();
        cx.spawn(async move |this, cx| {
            if let Err(error) = PromptTemplate::delete(store.pool(), id).await {
                _ = this.update(cx, |view, cx| {
                    view.error_message = Some(format!("删除失败: {error}"));
                    cx.notify();
                });
                return;
            }
            _ = this.update(cx, |view, cx| {
                view.load_prompts(cx);
            });
        })
        .detach();
    }

    /// 列表里展示的内容预览：折叠换行并截断，避免撑高行。
    fn content_preview(content: &str) -> String {
        let flat = content.split_whitespace().collect::<Vec<_>>().join(" ");
        if flat.chars().count() > 60 {
            format!("{}…", flat.chars().take(60).collect::<String>())
        } else {
            flat
        }
    }

    fn render_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let style = ManagementStyle::current(cx);
        let col_widths = [px(50.0), px(180.0), px(280.0), px(180.0), px(120.0)];

        list_container(style)
            .child(
                list_header(style)
                    .child(list_header_cell(Some(col_widths[0]), style).child("ID"))
                    .child(list_header_cell(Some(col_widths[1]), style).child("名称"))
                    .child(list_header_cell(Some(col_widths[2]), style).child("内容"))
                    .child(list_header_cell(Some(col_widths[3]), style).child("描述"))
                    .child(list_header_cell(Some(col_widths[4]), style).child("操作")),
            )
            .children(self.prompts.iter().map(|prompt| {
                let prompt_id = prompt.id;
                let prompt_for_edit = prompt.clone();
                list_row(style)
                    .debug_selector({
                        let id = prompt_id;
                        move || format!("PROMPT_ROW-{id}")
                    })
                    .child(list_cell(Some(col_widths[0]), style).child(format!("{}", prompt.id)))
                    .child(
                        list_cell(Some(col_widths[1]), style)
                            .text_color(style.list.foreground)
                            .child(prompt.name.clone()),
                    )
                    .child(
                        list_cell(Some(col_widths[2]), style)
                            .child(Self::content_preview(&prompt.content)),
                    )
                    .child(
                        list_cell(Some(col_widths[3]), style)
                            .truncate()
                            .child(prompt.description.clone().unwrap_or_default()),
                    )
                    .child(
                        list_actions(Some(col_widths[4]), style)
                            .child(
                                action_button(
                                    ("prompt-edit", prompt_id as u64),
                                    "编辑",
                                    ActionRole::Edit,
                                    ActionSize::Row,
                                    style,
                                )
                                .debug_selector({
                                    let id = prompt_id;
                                    move || format!("PROMPT_EDIT-{id}")
                                })
                                .on_mouse_down(
                                    MouseButton::Left,
                                    {
                                        let this = cx.weak_entity();
                                        move |_, window, cx| {
                                            _ = this.update(cx, |view, cx| {
                                                view.show_edit_form(
                                                    window,
                                                    prompt_for_edit.clone(),
                                                    cx,
                                                );
                                            });
                                        }
                                    },
                                ),
                            )
                            .child(
                                action_button(
                                    ("prompt-delete", prompt_id as u64),
                                    "删除",
                                    ActionRole::Delete,
                                    ActionSize::Row,
                                    style,
                                )
                                .debug_selector({
                                    let id = prompt_id;
                                    move || format!("PROMPT_DELETE-{id}")
                                })
                                .on_mouse_down(
                                    MouseButton::Left,
                                    {
                                        let this = cx.weak_entity();
                                        move |_, _, cx| {
                                            _ = this.update(cx, |view, cx| {
                                                view.confirm_delete_id = Some(prompt_id);
                                                cx.notify();
                                            });
                                        }
                                    },
                                ),
                            ),
                    )
            }))
    }
}

impl Render for PromptLibraryView {
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

        // 表单弹窗可能在初始化前被渲染（show_form 已置位但输入状态未建），补一次。
        if self.show_form && self.name_input.is_none() {
            let name = self.form_name.clone();
            let content = self.form_content.clone();
            let description = self.form_description.clone();
            self.name_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("输入提示词名称")
                    .default_value(&name)
            }));
            self.content_input = Some(cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder("输入提示词内容")
                    .default_value(&content)
                    .auto_grow(4, 16)
            }));
            self.description_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("输入描述（可选）")
                    .default_value(&description)
            }));
        }

        if self.search_input.is_none() {
            self.search_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("搜索名称或内容...")
                    .default_value(&self.search_text)
            }));
            if let Some(ref search_input) = self.search_input {
                cx.subscribe_in(search_input, window, |this, state, event, _window, cx| {
                    if let InputEvent::Change = event {
                        this.search_text = state.read(cx).value().to_string();
                        this.load_prompts(cx);
                    }
                })
                .detach();
            }
        }

        let list_body: AnyElement = if self.loading && self.prompts.is_empty() {
            div()
                .flex()
                .items_center()
                .justify_center()
                .h_full()
                .text_color(muted_foreground)
                .text_size(px(14.0))
                .child("加载中...")
                .into_any_element()
        } else if self.prompts.is_empty() {
            div()
                .flex()
                .items_center()
                .justify_center()
                .h_full()
                .text_color(muted_foreground)
                .text_size(px(14.0))
                .child(if self.search_text.trim().is_empty() {
                    "暂无提示词，点击右上角「+ 添加提示词」新建"
                } else {
                    "没有匹配的提示词"
                })
                .into_any_element()
        } else {
            div()
                .debug_selector(|| PROMPT_LIST_LOADED.to_owned())
                .child(self.render_list(cx))
                .into_any_element()
        };

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
                            .child("提示词管理"),
                    )
                    .child(
                        action_button(
                            "prompt-add",
                            "+ 添加提示词",
                            ActionRole::Main,
                            ActionSize::Page,
                            style,
                        )
                        .debug_selector(|| "PROMPT_ADD".to_owned())
                        .on_mouse_down(MouseButton::Left, {
                            let this = cx.weak_entity();
                            move |_, window, cx| {
                                _ = this.update(cx, |view, cx| {
                                    view.show_add_form(window, cx);
                                });
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
                                    .w(px(240.0))
                                    .child(Input::new(self.search_input.as_ref().unwrap())),
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
                    .child(list_body),
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
                            .child(format!("共 {} 条", self.total_count)),
                    )
                    .child(
                        div()
                            .text_size(px(13.0))
                            .text_color(muted_foreground)
                            .child(format!("最多展示 {PAGE_LIMIT} 条")),
                    ),
            )
            // 新增 / 编辑弹窗
            .when(self.show_form, |this| {
                this.child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(overlay)
                        .cursor(CursorStyle::PointingHand)
                        .on_mouse_down(MouseButton::Left, {
                            let this = cx.weak_entity();
                            move |_, _, cx| {
                                _ = this.update(cx, |view, cx| view.hide_form(cx));
                            }
                        })
                        .child(
                            management_modal_panel(
                                management_modal_layer(
                                    px(560.0),
                                    window.bounds().size.height - px(48.0),
                                )
                                .debug_selector(|| PROMPT_MODAL.to_owned()),
                                popover,
                                popover_foreground,
                                border,
                            )
                            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                cx.stop_propagation();
                            })
                            .child(
                                management_modal_scroll(PROMPT_FORM_SCROLL, &self.form_scroll)
                                    .gap(px(16.0))
                                    .child(
                                        div()
                                            .text_size(px(18.0))
                                            .font_weight(FontWeight::BOLD)
                                            .child(if self.editing_prompt.is_some() {
                                                "编辑提示词"
                                            } else {
                                                "添加提示词"
                                            }),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap(px(8.0))
                                            .child(div().text_size(px(13.0)).child("名称 *"))
                                            .child(
                                                div()
                                                    .debug_selector(|| "PROMPT_NAME".to_owned())
                                                    .w_full()
                                                    .child(
                                                        Input::new(
                                                            self.name_input.as_ref().unwrap(),
                                                        )
                                                        .w_full()
                                                        .h(px(32.0))
                                                        .px(px(8.0))
                                                        .border_1()
                                                        .border_color(input)
                                                        .rounded(px(4.0)),
                                                    ),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap(px(8.0))
                                            .child(div().text_size(px(13.0)).child("内容 *"))
                                            .child(
                                                div()
                                                    .debug_selector(|| "PROMPT_CONTENT".to_owned())
                                                    .w_full()
                                                    .border_1()
                                                    .border_color(input)
                                                    .rounded(px(4.0))
                                                    .child(Textarea::new(
                                                        self.content_input.as_ref().unwrap(),
                                                    )),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap(px(8.0))
                                            .child(div().text_size(px(13.0)).child("描述"))
                                            .child(
                                                div()
                                                    .debug_selector(|| {
                                                        "PROMPT_DESCRIPTION".to_owned()
                                                    })
                                                    .w_full()
                                                    .child(
                                                        Input::new(
                                                            self.description_input
                                                                .as_ref()
                                                                .unwrap(),
                                                        )
                                                        .w_full()
                                                        .h(px(32.0))
                                                        .px(px(8.0))
                                                        .border_1()
                                                        .border_color(input)
                                                        .rounded(px(4.0)),
                                                    ),
                                            ),
                                    )
                                    .when_some(self.error_message.as_ref(), |this, error| {
                                        this.child(
                                            div()
                                                .p(px(8.0))
                                                .bg(danger)
                                                .rounded(px(4.0))
                                                .text_size(px(12.0))
                                                .text_color(danger_foreground)
                                                .child(error.clone()),
                                        )
                                    })
                                    .child(
                                        div()
                                            .flex()
                                            .justify_end()
                                            .gap(px(8.0))
                                            .child(
                                                action_button(
                                                    "prompt-form-cancel",
                                                    "取消",
                                                    ActionRole::Neutral,
                                                    ActionSize::Dialog,
                                                    style,
                                                )
                                                .on_mouse_down(MouseButton::Left, {
                                                    let this = cx.weak_entity();
                                                    move |_, _, cx| {
                                                        _ = this.update(cx, |view, cx| {
                                                            view.hide_form(cx);
                                                        });
                                                    }
                                                }),
                                            )
                                            .child(
                                                action_button(
                                                    "prompt-form-save",
                                                    "保存",
                                                    ActionRole::Main,
                                                    ActionSize::Dialog,
                                                    style,
                                                )
                                                .debug_selector(|| "PROMPT_FORM_SAVE".to_owned())
                                                .on_mouse_down(MouseButton::Left, {
                                                    let this = cx.weak_entity();
                                                    move |_, _, cx| {
                                                        _ = this.update(cx, |view, cx| {
                                                            view.save_prompt(cx);
                                                        });
                                                    }
                                                }),
                                            ),
                                    ),
                            ),
                        ),
                )
            })
            // 删除确认
            .when_some(self.confirm_delete_id, |this, _id| {
                this.child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(overlay)
                        .on_mouse_down(MouseButton::Left, {
                            let this = cx.weak_entity();
                            move |_, _, cx| {
                                _ = this.update(cx, |view, cx| {
                                    view.confirm_delete_id = None;
                                    cx.notify();
                                });
                            }
                        })
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
                                                .child("确定要删除这条提示词吗？此操作不可恢复。"),
                                        )
                                        .child(
                                            div()
                                                .flex()
                                                .justify_end()
                                                .gap(px(8.0))
                                                .child(
                                                    action_button(
                                                        "prompt-delete-cancel",
                                                        "取消",
                                                        ActionRole::Neutral,
                                                        ActionSize::Dialog,
                                                        style,
                                                    )
                                                    .on_mouse_down(MouseButton::Left, {
                                                        let this = cx.weak_entity();
                                                        move |_, _, cx| {
                                                            _ = this.update(cx, |view, cx| {
                                                                view.confirm_delete_id = None;
                                                                cx.notify();
                                                            });
                                                        }
                                                    }),
                                                )
                                                .child(
                                                    action_button(
                                                        "prompt-delete-confirm",
                                                        "确认删除",
                                                        ActionRole::Delete,
                                                        ActionSize::Dialog,
                                                        style,
                                                    )
                                                    .debug_selector(|| {
                                                        "PROMPT_DELETE_CONFIRM".to_owned()
                                                    })
                                                    .on_mouse_down(MouseButton::Left, {
                                                        let this = cx.weak_entity();
                                                        move |_, _, cx| {
                                                            _ = this.update(cx, |view, cx| {
                                                                if let Some(delete_id) =
                                                                    view.confirm_delete_id
                                                                {
                                                                    view.delete_prompt(
                                                                        delete_id, cx,
                                                                    );
                                                                    view.confirm_delete_id = None;
                                                                }
                                                            });
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
