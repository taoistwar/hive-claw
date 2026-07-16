//! Global config panel — CRUD with pagination and search.
use crate::datasource::{GlobalConfig, Store};
use gpui::*;
use gpui_component::ActiveTheme as _;
use gpui_component::input::{Input, InputState, NumberInput};
use gpui_component::scroll::ScrollableElement;

const PAGE_SIZE: i64 = 20;
const CONFIG_TYPES: &[&str] = &["text", "number", "json", "boolean"];

fn single_line_preview(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn convert_config_value(value: &str, source_type: &str, target_type: &str) -> String {
    if source_type == target_type || value.trim().is_empty() || target_type == "text" {
        return value.to_string();
    }

    let value = value.trim();
    match target_type {
        "number" => value
            .parse::<f64>()
            .ok()
            .filter(|number| number.is_finite())
            .map(|_| value.to_string())
            .unwrap_or_else(|| "0".to_string()),
        "boolean" => value
            .parse::<bool>()
            .map(|boolean| boolean.to_string())
            .unwrap_or_else(|_| "false".to_string()),
        "json" => serde_json::from_str::<serde_json::Value>(value)
            .map(|json| json.to_string())
            .unwrap_or_else(|_| "{}".to_string()),
        _ => value.to_string(),
    }
}

pub struct GlobalConfigView {
    pub store: Option<Store>,
    items: Vec<GlobalConfig>,
    total: i64,
    page: i64,
    search: SharedString,
    loaded: bool,
    show_form: bool,
    edit_id: Option<i64>,
    form_name: SharedString,
    form_key: SharedString,
    form_type_idx: usize,
    form_data: SharedString,
    error: Option<SharedString>,
    search_input: Option<Entity<InputState>>,
    name_input: Option<Entity<InputState>>,
    key_input: Option<Entity<InputState>>,
    data_input: Option<Entity<InputState>>,
}

impl GlobalConfigView {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        GlobalConfigView {
            store: None,
            items: vec![],
            total: 0,
            page: 1,
            search: "".into(),
            loaded: false,
            show_form: false,
            edit_id: None,
            form_name: "".into(),
            form_key: "".into(),
            form_type_idx: 0,
            form_data: "".into(),
            error: None,
            search_input: None,
            name_input: None,
            key_input: None,
            data_input: None,
        }
    }
    fn reload(&mut self, cx: &mut Context<Self>) {
        self.loaded = false;
        cx.notify();
    }
    fn prev_page(&mut self, cx: &mut Context<Self>) {
        if self.page > 1 {
            self.page -= 1;
            self.reload(cx);
        }
    }
    fn next_page(&mut self, cx: &mut Context<Self>) {
        let tp = ((self.total + PAGE_SIZE - 1) / PAGE_SIZE).max(1);
        if self.page < tp {
            self.page += 1;
            self.reload(cx);
        }
    }
    fn do_search(&mut self, cx: &mut Context<Self>) {
        if let Some(ref inp) = self.search_input {
            self.search = inp.read(cx).value().to_string().into();
        }
        self.page = 1;
        self.reload(cx);
    }
    fn open_add(&mut self, cx: &mut Context<Self>) {
        self.show_form = true;
        self.edit_id = None;
        self.error = None;
        self.form_name = "".into();
        self.form_key = "".into();
        self.form_type_idx = 0;
        self.form_data = "".into();
        self.name_input = None;
        self.key_input = None;
        self.data_input = None;
        cx.notify();
    }
    fn open_edit(&mut self, item: &GlobalConfig, cx: &mut Context<Self>) {
        self.show_form = true;
        self.edit_id = Some(item.id);
        self.error = None;
        self.form_name = item.name.clone().into();
        self.form_key = item.key.clone().into();
        self.form_type_idx = CONFIG_TYPES
            .iter()
            .position(|t| t == &item.config_type)
            .unwrap_or(0);
        self.form_data = item.data.clone().into();
        self.name_input = None;
        self.key_input = None;
        self.data_input = None;
        cx.notify();
    }
    fn close_form(&mut self, cx: &mut Context<Self>) {
        self.show_form = false;
        cx.notify();
    }

    fn save_config(&mut self, cx: &mut Context<Self>) {
        let name = self.form_name.to_string();
        let key = self.form_key.to_string();
        let config_type = CONFIG_TYPES[self.form_type_idx].to_string();
        let data = self.form_data.to_string();
        if name.is_empty() || key.is_empty() {
            self.error = Some("名称和 Key 不能为空".into());
            cx.notify();
            return;
        }
        if let Some(ref s) = self.store {
            let s = s.clone();
            let edit_id = self.edit_id;
            let entity = cx.entity();
            cx.spawn(async move |_this, cx| {
                let result = if let Some(id) = edit_id {
                    s.update_global_config(id, &name, &key, &config_type, &data)
                        .await
                } else {
                    s.create_global_config(&name, &key, &config_type, &data)
                        .await
                        .map(|_| true)
                };
                _ = entity.update(cx, |this, cx| {
                    if result.is_ok() {
                        this.show_form = false;
                        this.reload(cx);
                    } else {
                        this.error = Some("保存失败".into());
                        cx.notify();
                    }
                });
            })
            .detach();
        }
    }

    fn ensure_loaded(&mut self, cx: &mut Context<Self>) {
        if self.loaded {
            return;
        }
        let Some(store) = self.store.clone() else {
            return;
        };

        self.loaded = true;
        let search = self.search.to_string();
        let page = self.page;
        let entity = cx.entity();
        cx.spawn(async move |_this, cx| {
            if let Ok((items, total)) = store.list_global_configs(&search, page, PAGE_SIZE).await {
                _ = entity.update(cx, |this, cx| {
                    this.items = items;
                    this.total = total;
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn ensure_form_inputs(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Entity<InputState>, Entity<InputState>) {
        if self.name_input.is_none() {
            self.name_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("名称")
                    .default_value(&self.form_name.to_string())
            }));
            self.key_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("Key（唯一）")
                    .default_value(&self.form_key.to_string())
            }));
        }

        let name = self.name_input.clone().expect("name input initialized");
        let key = self.key_input.clone().expect("key input initialized");
        self.form_name = name.read(cx).value().to_string().into();
        self.form_key = key.read(cx).value().to_string().into();
        (name, key)
    }

    fn render_data_field(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if self.form_type_idx == 3 {
            let is_true = self.form_data.as_ref() == "true";
            let entity = cx.entity();
            return div()
                .flex()
                .flex_row()
                .gap(px(8.0))
                .child(bool_radio("true", is_true, entity.clone()))
                .child(bool_radio("false", !is_true, entity))
                .into_any_element();
        }

        let is_number = self.form_type_idx == 1;
        let is_multiline = self.form_type_idx == 0 || self.form_type_idx == 2;
        if self.data_input.is_none() {
            let default_value = self.form_data.to_string();
            self.data_input = Some(cx.new(|cx| {
                let input = InputState::new(window, cx)
                    .placeholder("数据值")
                    .default_value(&default_value);
                if is_multiline {
                    input.multi_line(true).rows(5)
                } else {
                    input
                }
            }));
        }

        let input = self.data_input.clone().expect("data input initialized");
        self.form_data = input.read(cx).value().to_string().into();
        if is_number {
            NumberInput::new(&input).into_any_element()
        } else {
            let input = Input::new(&input);
            if is_multiline {
                input.h(px(100.0)).into_any_element()
            } else {
                input.into_any_element()
            }
        }
    }

    fn render_form_overlay(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if !self.show_form {
            return div().into_any_element();
        }

        let editing = self.edit_id.is_some();
        let (name, key) = self.ensure_form_inputs(window, cx);
        let data_field = self.render_data_field(window, cx);
        let type_selector = type_row(self.form_type_idx, cx);
        let error = self
            .error
            .as_ref()
            .map(|error| {
                div()
                    .text_size(px(13.0))
                    .text_color(rgb(0xff4444))
                    .child(error.clone())
                    .into_any_element()
            })
            .unwrap_or_else(|| div().into_any_element());

        div()
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .bottom_0()
            .bg(rgba(0x00000055))
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(12.0))
                    .p(px(24.0))
                    .w(px(460.0))
                    .bg(rgb(0xffffff))
                    .rounded(px(12.0))
                    .shadow_md()
                    .child(
                        div()
                            .text_size(px(18.0))
                            .font_weight(FontWeight::BOLD)
                            .child(if editing {
                                "编辑配置"
                            } else {
                                "添加配置"
                            }),
                    )
                    .child(field_with_input("名称", name))
                    .child(field_with_input("Key", key))
                    .child(type_selector)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(rgb(0x666666))
                                    .child("数据值"),
                            )
                            .child(data_field),
                    )
                    .child(error)
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap(px(8.0))
                            .justify_end()
                            .child(btn(
                                "取消",
                                rgb(0xe8e8f0),
                                rgb(0x666666),
                                cx.listener(|this, _, _, cx| this.close_form(cx)),
                            ))
                            .child(btn(
                                if editing { "更新" } else { "保存" },
                                rgb(0x6f5699),
                                rgb(0xffffff),
                                cx.listener(|this, _, _, cx| this.save_config(cx)),
                            )),
                    ),
            )
            .into_any_element()
    }

    fn render_header(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .items_center()
            .justify_between()
            .p(px(16.0))
            .border_b_1()
            .border_color(rgb(0xe0e0e0))
            .child(
                div()
                    .text_size(px(18.0))
                    .font_weight(FontWeight::BOLD)
                    .child("全局配置"),
            )
            .child(
                div()
                    .id("add-config-btn")
                    .px(px(12.0))
                    .py(px(6.0))
                    .rounded(px(4.0))
                    .bg(rgb(0x6f5699))
                    .text_color(rgb(0xffffff))
                    .text_size(px(13.0))
                    .cursor(CursorStyle::PointingHand)
                    .child("+ 添加配置")
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| this.open_add(cx)),
                    ),
            )
            .into_any_element()
    }

    fn render_search_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if self.search_input.is_none() {
            self.search_input =
                Some(cx.new(|cx| InputState::new(window, cx).placeholder("搜索名称或 Key")));
        }
        let search_input = self.search_input.clone().expect("search input initialized");

        div()
            .flex()
            .items_center()
            .p(px(12.0))
            .border_b_1()
            .border_color(rgb(0xe0e0e0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(div().text_size(px(13.0)).child("搜索:"))
                    .child(div().w(px(200.0)).child(Input::new(&search_input)))
                    .child(
                        div()
                            .id("search-btn")
                            .px(px(12.0))
                            .py(px(4.0))
                            .rounded(px(4.0))
                            .bg(rgb(0x6f5699))
                            .text_color(rgb(0xffffff))
                            .text_size(px(12.0))
                            .cursor(CursorStyle::PointingHand)
                            .child("搜索")
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| this.do_search(cx)),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_config_row(
        &self,
        item: &GlobalConfig,
        column_widths: &[Pixels; 6],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = item.id;
        let item_to_edit = item.clone();
        let store = self.store.clone();
        let entity = cx.entity();

        div()
            .flex()
            .border_b_1()
            .border_color(rgb(0xf0f0f0))
            .bg(rgb(0xffffff))
            .child(table_cell(column_widths[0]).child(item.id.to_string()))
            .child(
                table_cell(column_widths[1])
                    .text_size(px(13.0))
                    .font_weight(FontWeight::MEDIUM)
                    .child(item.name.clone()),
            )
            .child(table_cell(column_widths[2]).child(item.key.clone()))
            .child(
                table_cell(column_widths[3])
                    .text_size(px(11.0))
                    .text_color(rgb(0x999999))
                    .bg(rgb(0xe8e8f0))
                    .px(px(6.0))
                    .py(px(1.0))
                    .rounded(px(3.0))
                    .child(item.config_type.clone()),
            )
            .child(
                table_cell(column_widths[4])
                    .truncate()
                    .child(single_line_preview(&item.data)),
            )
            .child(
                div()
                    .w(column_widths[5])
                    .px(px(8.0))
                    .py(px(6.0))
                    .flex()
                    .gap(px(4.0))
                    .child(row_action("edit", id, "编辑", rgb(0x5cb85c), {
                        let entity = entity.clone();
                        move |_, _, cx| {
                            _ = entity.update(cx, |this, cx| this.open_edit(&item_to_edit, cx));
                        }
                    }))
                    .child(row_action(
                        "delete",
                        id,
                        "删除",
                        rgb(0xd9534f),
                        move |_, _, cx| {
                            if let Some(store) = store.clone() {
                                let entity = entity.clone();
                                cx.spawn(async move |cx| {
                                    _ = store.delete_global_config(id).await;
                                    _ = entity.update(cx, |this, cx| this.reload(cx));
                                })
                                .detach();
                            }
                        },
                    )),
            )
            .into_any_element()
    }

    fn render_config_list(&self, cx: &mut Context<Self>) -> AnyElement {
        let column_widths = config_column_widths();
        let mut rows = Vec::with_capacity(self.items.len());
        for item in &self.items {
            rows.push(self.render_config_row(item, &column_widths, cx));
        }

        div()
            .flex()
            .flex_col()
            .border_1()
            .border_color(rgb(0xe0e0e0))
            .rounded(px(4.0))
            .overflow_hidden()
            .child(render_table_header(&column_widths))
            .children(rows)
            .into_any_element()
    }

    fn render_list_area(&self, cx: &mut Context<Self>) -> AnyElement {
        let content = if self.items.is_empty() {
            div()
                .flex()
                .items_center()
                .justify_center()
                .h_full()
                .text_color(rgb(0x999999))
                .text_size(px(14.0))
                .child("暂无数据")
                .into_any_element()
        } else {
            self.render_config_list(cx)
        };

        div()
            .flex()
            .flex_col()
            .flex_1()
            .overflow_y_scrollbar()
            .p(px(16.0))
            .child(content)
            .into_any_element()
    }

    fn render_pagination(&self, cx: &mut Context<Self>) -> AnyElement {
        let total_pages = ((self.total + PAGE_SIZE - 1) / PAGE_SIZE).max(1);
        let has_previous = self.page > 1;
        let has_next = self.page < total_pages;

        div()
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_between()
            .p(px(12.0))
            .border_t_1()
            .border_color(rgb(0xe0e0e0))
            .child(
                div()
                    .text_size(px(13.0))
                    .text_color(rgb(0x666666))
                    .child(format!("共 {} 条", self.total)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(page_button(
                        "prev-page",
                        "上一页",
                        has_previous,
                        cx.listener(|this, _, _, cx| this.prev_page(cx)),
                    ))
                    .child(
                        div()
                            .text_size(px(13.0))
                            .text_color(rgb(0x666666))
                            .child(format!("第 {} / {} 页", self.page, total_pages)),
                    )
                    .child(page_button(
                        "next-page",
                        "下一页",
                        has_next,
                        cx.listener(|this, _, _, cx| this.next_page(cx)),
                    )),
            )
            .into_any_element()
    }
}

impl Render for GlobalConfigView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_loaded(cx);

        let form_overlay = self.render_form_overlay(window, cx);
        let header = self.render_header(cx);
        let search_bar = self.render_search_bar(window, cx);
        let list_area = self.render_list_area(cx);
        let pagination = self.render_pagination(cx);

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(header)
            .child(search_bar)
            .child(list_area)
            .child(pagination)
            .child(form_overlay)
    }
}

fn config_column_widths() -> [Pixels; 6] {
    [
        px(60.0),
        px(150.0),
        px(140.0),
        px(100.0),
        px(180.0),
        px(120.0),
    ]
}

fn render_table_header(column_widths: &[Pixels; 6]) -> impl IntoElement {
    const LABELS: [&str; 6] = ["ID", "名称", "Key", "类型", "数据值", "操作"];

    div()
        .flex()
        .bg(rgb(0xf5f5f5))
        .border_b_1()
        .border_color(rgb(0xe0e0e0))
        .children(
            LABELS
                .into_iter()
                .zip(column_widths.iter().copied())
                .map(|(label, width)| {
                    div()
                        .w(width)
                        .px(px(8.0))
                        .py(px(8.0))
                        .text_size(px(12.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(0x333333))
                        .child(label)
                }),
        )
}

fn table_cell(width: Pixels) -> Div {
    div()
        .w(width)
        .px(px(8.0))
        .py(px(6.0))
        .text_size(px(12.0))
        .text_color(rgb(0x666666))
}

fn row_action(
    id_prefix: &'static str,
    id: i64,
    label: &'static str,
    background: Rgba,
    handler: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id((id_prefix, id as u64))
        .px(px(8.0))
        .py(px(3.0))
        .rounded(px(3.0))
        .bg(background)
        .text_color(rgb(0xffffff))
        .text_size(px(11.0))
        .cursor(CursorStyle::PointingHand)
        .child(label)
        .on_mouse_down(MouseButton::Left, handler)
}

fn page_button(
    id: &'static str,
    label: &'static str,
    enabled: bool,
    handler: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .px(px(12.0))
        .py(px(4.0))
        .rounded(px(4.0))
        .bg(if enabled {
            rgb(0x6f5699)
        } else {
            rgb(0xcccccc)
        })
        .text_color(rgb(0xffffff))
        .text_size(px(12.0))
        .cursor(if enabled {
            CursorStyle::PointingHand
        } else {
            CursorStyle::Arrow
        })
        .child(label)
        .on_mouse_down(MouseButton::Left, handler)
}

fn field_with_input(label: &'static str, input: Entity<InputState>) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(4.0))
        .child(
            div()
                .text_size(px(12.0))
                .text_color(rgb(0x666666))
                .child(label),
        )
        .child(Input::new(&input))
}

fn type_row(selected: usize, cx: &mut Context<GlobalConfigView>) -> impl IntoElement {
    let mut btns = div().flex().flex_row().gap(px(4.0));
    for (i, t) in CONFIG_TYPES.iter().enumerate() {
        let is_active = i == selected;
        let entity = cx.entity();
        btns = btns.child(
            div()
                .px(px(12.0))
                .py(px(4.0))
                .rounded(px(4.0))
                .bg(if is_active {
                    rgb(0x6f5699)
                } else {
                    rgb(0xe8e8f0)
                })
                .text_color(if is_active {
                    rgb(0xffffff)
                } else {
                    rgb(0x666666)
                })
                .text_size(px(12.0))
                .cursor(CursorStyle::PointingHand)
                .child(*t)
                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    _ = entity.update(cx, |this, cx| {
                        let source_type = CONFIG_TYPES[this.form_type_idx];
                        let target_type = CONFIG_TYPES[i];
                        this.form_data =
                            convert_config_value(this.form_data.as_ref(), source_type, target_type)
                                .into();
                        this.form_type_idx = i;
                        this.data_input = None;
                        cx.notify();
                    });
                }),
        );
    }
    div()
        .flex()
        .flex_col()
        .gap(px(4.0))
        .child(
            div()
                .text_size(px(12.0))
                .text_color(rgb(0x666666))
                .child("类型"),
        )
        .child(btns)
}

fn btn(
    label: &'static str,
    bg: gpui::Rgba,
    fg: gpui::Rgba,
    handler: impl Fn(&gpui::MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .px(px(16.0))
        .py(px(8.0))
        .bg(bg)
        .rounded(px(6.0))
        .text_size(px(13.0))
        .text_color(fg)
        .cursor(CursorStyle::PointingHand)
        .hover(|s| {
            s.bg(gpui::Rgba {
                r: bg.r * 0.85,
                g: bg.g * 0.85,
                b: bg.b * 0.85,
                a: bg.a,
            })
        })
        .on_mouse_down(MouseButton::Left, handler)
        .child(label)
}

fn bool_radio(
    label: &'static str,
    active: bool,
    entity: Entity<GlobalConfigView>,
) -> impl IntoElement {
    let val = label.to_string();
    let dot_color = if active { rgb(0x6f5699) } else { rgb(0xffffff) };
    let border = if active { rgb(0x6f5699) } else { rgb(0xcccccc) };
    div()
        .flex()
        .flex_row()
        .gap(px(6.0))
        .items_center()
        .cursor(CursorStyle::PointingHand)
        .child(
            div()
                .w(px(16.0))
                .h(px(16.0))
                .rounded_full()
                .border_2()
                .border_color(border)
                .flex()
                .items_center()
                .justify_center()
                .child(div().w(px(8.0)).h(px(8.0)).rounded_full().bg(dot_color)),
        )
        .child(div().text_size(px(13.0)).child(label))
        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
            _ = entity.update(cx, |this, cx| {
                this.form_data = val.clone().into();
                cx.notify();
            });
        })
}

#[cfg(test)]
mod tests {
    use super::{convert_config_value, single_line_preview};

    #[test]
    fn multiline_config_data_is_safe_for_single_line_cells() {
        let preview = single_line_preview("{\r\n  \"enabled\": true\n}");

        assert_eq!(preview, "{ \"enabled\": true }");
        assert!(!preview.contains('\n'));
        assert!(!preview.contains('\r'));
    }

    #[test]
    fn text_values_are_converted_for_the_selected_type() {
        assert_eq!(convert_config_value(" 42.5 ", "text", "number"), "42.5");
        assert_eq!(convert_config_value("not-a-number", "text", "number"), "0");
        assert_eq!(convert_config_value(" true ", "text", "boolean"), "true");
        assert_eq!(convert_config_value("yes", "text", "boolean"), "false");
        assert_eq!(
            convert_config_value("{ \"enabled\": true }", "text", "json"),
            "{\"enabled\":true}"
        );
        assert_eq!(convert_config_value("not-json", "text", "json"), "{}");
    }

    #[test]
    fn json_values_follow_the_same_target_conversion_rules() {
        assert_eq!(convert_config_value("12", "json", "number"), "12");
        assert_eq!(convert_config_value("false", "json", "boolean"), "false");
        assert_eq!(
            convert_config_value("{\"name\":\"HiveClaw\"}", "json", "text"),
            "{\"name\":\"HiveClaw\"}"
        );
    }

    #[test]
    fn empty_or_same_type_values_are_not_changed() {
        assert_eq!(convert_config_value("", "text", "number"), "");
        assert_eq!(convert_config_value("   ", "json", "boolean"), "   ");
        assert_eq!(convert_config_value("001", "number", "number"), "001");
    }
}
