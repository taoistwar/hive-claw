use std::path::PathBuf;

use sha2::{Digest, Sha256};

use crate::datasource::{Store, entity_store::Plugin, wasm_exports::extract_wasm_exports};
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

pub struct PluginView {
    store: Entity<Store>,
    items: Vec<Plugin>,
    loading: bool,
    search_text: String,
    current_page: i64,
    page_size: i64,
    total_count: i64,
    show_form: bool,
    form_scroll: ScrollHandle,
    editing_id: Option<i64>,
    form_identifier: String,
    form_name: String,
    form_description: String,
    form_version: String,
    form_sha256: String,
    form_size_bytes: String,
    form_wasm_path: Option<PathBuf>,
    form_file_path: Option<PathBuf>,
    form_manifest: Option<String>,
    error_message: Option<String>,
    confirm_delete_id: Option<i64>,
    identifier_input: Option<Entity<InputState>>,
    name_input: Option<Entity<InputState>>,
    description_input: Option<Entity<InputState>>,
    version_input: Option<Entity<InputState>>,
    sha256_input: Option<Entity<InputState>>,
    size_bytes_input: Option<Entity<InputState>>,
    search_input: Option<Entity<InputState>>,
}

impl PluginView {
    pub fn new(store: Entity<Store>, cx: &mut Context<Self>) -> Self {
        let mut v = Self {
            store,
            items: Vec::new(),
            loading: false,
            search_text: String::new(),
            current_page: 0,
            page_size: 20,
            total_count: 0,
            show_form: false,
            form_scroll: ScrollHandle::default(),
            editing_id: None,
            form_identifier: String::new(),
            form_name: String::new(),
            form_description: String::new(),
            form_version: String::new(),
            form_sha256: String::new(),
            form_size_bytes: String::new(),
            form_wasm_path: None,
            form_file_path: None,
            form_manifest: None,
            error_message: None,
            confirm_delete_id: None,
            identifier_input: None,
            name_input: None,
            description_input: None,
            version_input: None,
            sha256_input: None,
            size_bytes_input: None,
            search_input: None,
        };
        v.load(cx);
        v
    }

    fn inspect_wasm(path: &PathBuf) -> anyhow::Result<(String, i64, String)> {
        let data = std::fs::read(path)?;
        let size = data.len() as i64;
        let mut hasher = Sha256::new();
        hasher.update(&data);
        let hash = hasher.finalize();
        let hex = hash
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect::<String>();
        let exports = extract_wasm_exports(&data)?;
        let manifest = serde_json::json!({ "exports": exports }).to_string();
        Ok((hex, size, manifest))
    }

    fn manifest_exports(manifest: Option<&str>) -> Vec<String> {
        manifest
            .and_then(|json| serde_json::from_str::<serde_json::Value>(json).ok())
            .and_then(|value| {
                value
                    .get("exports")
                    .and_then(|exports| exports.as_array())
                    .cloned()
            })
            .unwrap_or_default()
            .into_iter()
            .filter_map(|export| export.as_str().map(str::to_owned))
            .filter(|export| !export.is_empty())
            .collect()
    }

    fn manifest_for_edit(item: &Plugin, base_dir: &std::path::Path) -> Option<String> {
        if !Self::manifest_exports(item.manifest.as_deref()).is_empty() {
            return item.manifest.clone();
        }

        Self::inspect_wasm(&item.wasm_path(base_dir))
            .map(|(_, _, manifest)| manifest)
            .ok()
            .or_else(|| item.manifest.clone())
    }

    fn save_wasm_locally(
        base_dir: &std::path::Path,
        plugin_id: i64,
        source_path: &std::path::Path,
    ) -> anyhow::Result<()> {
        let plugin_dir = Plugin::ensure_plugin_dir(base_dir, plugin_id)?;
        let dest_path = plugin_dir.join("plugin.wasm");
        std::fs::copy(source_path, &dest_path)?;
        tracing::info!(
            plugin_id = plugin_id,
            dest = %dest_path.display(),
            "WASM file saved locally"
        );
        Ok(())
    }

    fn pick_wasm_file(&mut self, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("选择 WASM 文件".into()),
        });
        cx.spawn(async move |this, cx| {
            let result = receiver.await;
            match result {
                Ok(Ok(Some(mut paths))) => {
                    if let Some(path) = paths.pop() {
                        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                        if ext.to_lowercase() != "wasm" {
                            this.update(cx, |v, cx| {
                                v.error_message = Some("请选择 .wasm 文件".into());
                                cx.notify();
                            })
                            .ok();
                            return;
                        }
                        match Self::inspect_wasm(&path) {
                            Ok((sha256, size, manifest)) => {
                                this.update(cx, |v, cx| {
                                    v.form_wasm_path = Some(path.clone());
                                    v.form_sha256 = sha256;
                                    v.form_size_bytes = size.to_string();
                                    v.form_manifest = Some(manifest);
                                    // Null out inputs so render re-inits them with new values
                                    v.sha256_input = None;
                                    v.size_bytes_input = None;
                                    v.error_message = None;
                                    cx.notify();
                                })
                                .ok();
                            }
                            Err(e) => {
                                this.update(cx, |v, cx| {
                                    v.error_message = Some(format!("读取文件失败: {}", e));
                                    cx.notify();
                                })
                                .ok();
                            }
                        }
                    }
                }
                Ok(Ok(None)) => {}
                Ok(Err(e)) => {
                    this.update(cx, |v, cx| {
                        v.error_message = Some(format!("文件选择失败: {}", e));
                        cx.notify();
                    })
                    .ok();
                }
                Err(_) => {}
            }
        })
        .detach();
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        let store = self.store.read(cx).clone();
        let search = if self.search_text.is_empty() {
            None
        } else {
            Some(self.search_text.clone())
        };
        let offset = self.current_page * self.page_size;
        cx.spawn(async move |this, cx| {
            let items = Plugin::list(store.pool(), search.clone(), 20, offset).await?;
            let count = Plugin::count(store.pool(), search).await?;
            this.update(cx, |v, cx| {
                v.items = items;
                v.total_count = count;
                v.loading = false;
                cx.notify();
            })
        })
        .detach();
    }

    fn init_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.identifier_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("输入唯一标识符")
                .default_value(&self.form_identifier)
        }));
        self.name_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("输入名称")
                .default_value(&self.form_name)
        }));
        self.description_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("输入描述（可选）")
                .default_value(&self.form_description)
        }));
        self.version_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("如 1.0.0")
                .default_value(&self.form_version)
        }));
        self.sha256_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("SHA256 哈希")
                .default_value(&self.form_sha256)
        }));
        self.size_bytes_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("文件大小")
                .default_value(&self.form_size_bytes)
        }));
    }

    fn show_add_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_form = true;
        self.editing_id = None;
        self.form_identifier.clear();
        self.form_name.clear();
        self.form_description.clear();
        self.form_version.clear();
        self.form_sha256.clear();
        self.form_size_bytes.clear();
        self.form_wasm_path = None;
        self.form_file_path = None;
        self.form_manifest = None;
        self.error_message = None;
        self.init_inputs(window, cx);
    }

    fn show_edit_form(&mut self, window: &mut Window, item: Plugin, cx: &mut Context<Self>) {
        let base_dir = Store::default_db_path();
        let manifest = Self::manifest_for_edit(&item, &base_dir);
        self.show_form = true;
        self.editing_id = Some(item.id);
        self.form_identifier = item.identifier.clone();
        self.form_name = item.name.clone();
        self.form_description = item.description.clone().unwrap_or_default();
        self.form_version = item.version.clone();
        self.form_sha256 = item.sha256.clone();
        self.form_size_bytes = item.size_bytes.to_string();
        self.form_wasm_path = None;
        self.form_file_path = Some(item.wasm_path(&base_dir));
        self.form_manifest = manifest;
        self.error_message = None;
        self.init_inputs(window, cx);
    }

    fn hide_form(&mut self, cx: &mut Context<Self>) {
        self.form_scroll.set_offset(point(px(0.0), px(0.0)));
        self.show_form = false;
        self.editing_id = None;
        self.form_file_path = None;
        self.error_message = None;
        self.identifier_input = None;
        self.name_input = None;
        self.description_input = None;
        self.version_input = None;
        self.sha256_input = None;
        self.size_bytes_input = None;
        cx.notify();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        if let Some(ref inp) = self.identifier_input {
            self.form_identifier = inp.read(cx).value().to_string();
        }
        if let Some(ref inp) = self.name_input {
            self.form_name = inp.read(cx).value().to_string();
        }
        if let Some(ref inp) = self.description_input {
            self.form_description = inp.read(cx).value().to_string();
        }
        if let Some(ref inp) = self.version_input {
            self.form_version = inp.read(cx).value().to_string();
        }
        if self.form_identifier.trim().is_empty()
            || self.form_name.trim().is_empty()
            || self.form_version.trim().is_empty()
            || self.form_sha256.trim().is_empty()
        {
            self.error_message = Some("必填字段不能为空".into());
            cx.notify();
            return;
        }
        // For new plugins, require WASM file selection
        if self.editing_id.is_none() && self.form_wasm_path.is_none() {
            self.error_message = Some("请先选择 WASM 文件".into());
            cx.notify();
            return;
        }
        // Auto-generate S3 key from identifier + version
        let s3k = format!(
            "plugins/{}/{}.wasm",
            self.form_identifier, self.form_version
        );
        let size_bytes: i64 = self.form_size_bytes.parse().unwrap_or(0);
        let store = self.store.read(cx).clone();
        let idf = self.form_identifier.clone();
        let name = self.form_name.clone();
        let desc = if self.form_description.is_empty() {
            None
        } else {
            Some(self.form_description.clone())
        };
        let rt = "extism".to_string();
        let ver = self.form_version.clone();
        let sha = self.form_sha256.clone();
        let manifest = self.form_manifest.clone();
        let wasm_path = self.form_wasm_path.clone();
        let base_dir = Store::default_db_path();

        if let Some(eid) = self.editing_id {
            cx.spawn(async move |this, cx| {
                match Plugin::update(
                    store.pool(),
                    eid,
                    idf,
                    name,
                    desc,
                    manifest,
                    rt,
                    ver,
                    None,
                    None,
                    s3k,
                    sha,
                    size_bytes,
                    None,
                )
                .await
                {
                    Ok(_) => {
                        // Copy WASM file locally if a new one was selected
                        if let Some(ref src) = wasm_path {
                            if let Err(e) = Self::save_wasm_locally(&base_dir, eid, src) {
                                tracing::error!("Failed to save WASM locally: {}", e);
                            }
                        }
                        this.update(cx, |v, cx| {
                            v.hide_form(cx);
                            v.load(cx);
                        })
                        .ok();
                    }
                    Err(e) => {
                        this.update(cx, |v, cx| {
                            v.error_message = Some(format!("更新失败: {}", e));
                            cx.notify();
                        })
                        .ok();
                    }
                }
            })
            .detach();
        } else {
            let manifest = self.form_manifest.clone();
            cx.spawn(async move |this, cx| {
                match Plugin::create(
                    store.pool(),
                    idf,
                    name,
                    desc,
                    manifest,
                    rt,
                    ver,
                    None,
                    None,
                    s3k,
                    sha,
                    size_bytes,
                    None,
                )
                .await
                {
                    Ok(plugin) => {
                        // Copy WASM file locally
                        if let Some(ref src) = wasm_path {
                            if let Err(e) = Self::save_wasm_locally(&base_dir, plugin.id, src) {
                                tracing::error!("Failed to save WASM locally: {}", e);
                            }
                        }
                        this.update(cx, |v, cx| {
                            v.hide_form(cx);
                            v.load(cx);
                        })
                        .ok();
                    }
                    Err(e) => {
                        this.update(cx, |v, cx| {
                            v.error_message = Some(format!("创建失败: {}", e));
                            cx.notify();
                        })
                        .ok();
                    }
                }
            })
            .detach();
        }
    }

    fn delete(&mut self, id: i64, cx: &mut Context<Self>) {
        let store = self.store.read(cx).clone();
        let base_dir = Store::default_db_path();
        cx.spawn(
            async move |this, cx| match Plugin::delete(store.pool(), id).await {
                Ok(_) => {
                    // Clean up local WASM directory
                    let wasm_dir = base_dir.join("plugins").join(id.to_string());
                    if wasm_dir.exists() {
                        if let Err(e) = std::fs::remove_dir_all(&wasm_dir) {
                            tracing::warn!(
                                plugin_id = id,
                                error = %e,
                                "Failed to cleanup plugin WASM directory"
                            );
                        }
                    }
                    this.update(cx, |v, cx| {
                        v.load(cx);
                    })
                    .ok();
                }
                Err(e) => {
                    this.update(cx, |v, cx| {
                        v.error_message = Some(format!("删除失败: {}", e));
                        cx.notify();
                    })
                    .ok();
                }
            },
        )
        .detach();
    }

    fn next_page(&mut self, cx: &mut Context<Self>) {
        if (self.current_page + 1) * self.page_size < self.total_count {
            self.current_page += 1;
            self.load(cx);
        }
    }
    fn prev_page(&mut self, cx: &mut Context<Self>) {
        if self.current_page > 0 {
            self.current_page -= 1;
            self.load(cx);
        }
    }
}

impl Render for PluginView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tp = (self.total_count + self.page_size - 1) / self.page_size;
        if self.show_form && self.identifier_input.is_none() {
            self.init_inputs(window, cx);
        }

        if self.search_input.is_none() {
            self.search_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("输入名称...")
                    .default_value(&self.search_text)
            }));
            if let Some(ref input) = self.search_input {
                cx.subscribe_in(input, window, |this, state, event, window, cx| {
                    if let InputEvent::Change = event {
                        this.search_text = state.read(cx).value().to_string();
                        this.current_page = 0;
                        this.load(cx);
                    }
                })
                .detach();
            }
        }
        let style = ManagementStyle::current(cx);
        let theme = cx.theme();
        let exports = Self::manifest_exports(self.form_manifest.as_deref());
        let export_background = style.action(ActionRole::Main).background;
        let export_foreground = style.action(ActionRole::Main).foreground;

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .p(px(16.0))
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .text_size(px(18.0))
                            .font_weight(FontWeight::BOLD)
                            .child("插件管理"),
                    )
                    .child(
                        action_button(
                            "add-btn",
                            "+ 添加插件",
                            ActionRole::Main,
                            ActionSize::Page,
                            style,
                        )
                        .on_mouse_down(MouseButton::Left, {
                            let t = cx.weak_entity();
                            move |_, window, cx| {
                                t.update(cx, |v, cx| v.show_add_form(window, cx)).ok();
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
                    .border_color(theme.border)
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
                    .child(if self.items.is_empty() {
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .h_full()
                            .text_color(style.list.muted_foreground)
                            .text_size(px(14.0))
                            .child("暂无数据")
                    } else {
                        let col_widths = [px(60.0), px(120.0), px(100.0), px(120.0), px(120.0)];
                        list_container(style)
                            .child(
                                list_header(style)
                                    .child(list_header_cell(Some(col_widths[0]), style).child("ID"))
                                    .child(
                                        list_header_cell(Some(col_widths[1]), style).child("名称"),
                                    )
                                    .child(
                                        list_header_cell(Some(col_widths[2]), style)
                                            .child("Identifier"),
                                    )
                                    .child(
                                        list_header_cell(Some(col_widths[3]), style)
                                            .child("Runtime/Version"),
                                    )
                                    .child(
                                        list_header_cell(Some(col_widths[4]), style).child("操作"),
                                    ),
                            )
                            .children(self.items.iter().map(|item| {
                                let id = item.id;
                                let ic = item.clone();
                                list_row(style)
                                    .child(
                                        list_cell(Some(col_widths[0]), style)
                                            .child(format!("{}", item.id)),
                                    )
                                    .child(
                                        list_cell(Some(col_widths[1]), style)
                                            .child(item.name.clone()),
                                    )
                                    .child(
                                        list_cell(Some(col_widths[2]), style)
                                            .child(item.identifier.clone()),
                                    )
                                    .child(
                                        list_cell(Some(col_widths[3]), style)
                                            .child(format!("{} v{}", item.runtime, item.version)),
                                    )
                                    .child(
                                        list_actions(None, style)
                                            .child(
                                                action_button(
                                                    ("edit", id as u64),
                                                    "编辑",
                                                    ActionRole::Edit,
                                                    ActionSize::Row,
                                                    style,
                                                )
                                                .on_mouse_down(MouseButton::Left, {
                                                    let t = cx.weak_entity();
                                                    move |_, window, cx| {
                                                        t.update(cx, |v, cx| {
                                                            v.show_edit_form(window, ic.clone(), cx)
                                                        })
                                                        .ok();
                                                    }
                                                }),
                                            )
                                            .child(
                                                action_button(
                                                    ("del", id as u64),
                                                    "删除",
                                                    ActionRole::Delete,
                                                    ActionSize::Row,
                                                    style,
                                                )
                                                .on_mouse_down(MouseButton::Left, {
                                                    let t = cx.weak_entity();
                                                    move |_, _, cx| {
                                                        t.update(cx, |v, cx| {
                                                            v.confirm_delete_id = Some(id);
                                                            cx.notify();
                                                        })
                                                        .ok();
                                                    }
                                                }),
                                            ),
                                    )
                            }))
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
                    .border_color(theme.border)
                    .child(
                        div()
                            .text_size(px(13.0))
                            .text_color(style.list.muted_foreground)
                            .child(format!("共 {} 条", self.total_count)),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(8.0))
                            .items_center()
                            .child(
                                action_button(
                                    "prev",
                                    "上一页",
                                    if self.current_page > 0 {
                                        ActionRole::Neutral
                                    } else {
                                        ActionRole::Disabled
                                    },
                                    ActionSize::Page,
                                    style,
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    {
                                        let t = cx.weak_entity();
                                        move |_, _, cx| {
                                            t.update(cx, |v, cx| v.prev_page(cx)).ok();
                                        }
                                    },
                                ),
                            )
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .text_color(style.list.muted_foreground)
                                    .child(format!(
                                        "第 {} / {} 页",
                                        self.current_page + 1,
                                        tp.max(1)
                                    )),
                            )
                            .child(
                                action_button(
                                    "next",
                                    "下一页",
                                    if (self.current_page + 1) * self.page_size < self.total_count {
                                        ActionRole::Neutral
                                    } else {
                                        ActionRole::Disabled
                                    },
                                    ActionSize::Page,
                                    style,
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    {
                                        let t = cx.weak_entity();
                                        move |_, _, cx| {
                                            t.update(cx, |v, cx| v.next_page(cx)).ok();
                                        }
                                    },
                                ),
                            ),
                    ),
            )
            .when(self.show_form, |this| {
                let identifier_input = self.identifier_input.clone().unwrap();
                let name_input = self.name_input.clone().unwrap();
                let description_input = self.description_input.clone().unwrap();
                let version_input = self.version_input.clone().unwrap();
                this.child(
                    div()
                        .absolute()
                        .top(px(0.0))
                        .left(px(0.0))
                        .right(px(0.0))
                        .bottom(px(0.0))
                        .bg(theme.overlay)
                        .opacity(0.3)
                        .cursor(CursorStyle::PointingHand)
                        .on_mouse_down(MouseButton::Left, {
                            let t = cx.weak_entity();
                            move |_, _, cx| {
                                t.update(cx, |v, cx| v.hide_form(cx)).ok();
                            }
                        }),
                )
                .child(
                    management_modal_panel(
                        management_modal_layer(px(550.0)),
                        theme.popover,
                        theme.foreground,
                        theme.border,
                    )
                    .debug_selector(|| "PLUGIN_MODAL".to_owned())
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .child(
                        management_modal_scroll("plugin-form-scroll", &self.form_scroll)
                            .gap(px(10.0))
                            .debug_selector(|| "PLUGIN_FORM_SCROLL".to_owned())
                            .child(
                                div()
                                    .text_size(px(18.0))
                                    .font_weight(FontWeight::BOLD)
                                    .child(if self.editing_id.is_some() {
                                        "编辑插件"
                                    } else {
                                        "添加插件"
                                    }),
                            )
                            .child(form_field("Identifier *", identifier_input, theme))
                            .child(form_field("名称 *", name_input, theme))
                            .child(form_field("描述", description_input, theme))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(4.0))
                                    .child(
                                        div()
                                            .text_size(px(13.0))
                                            .text_color(theme.foreground)
                                            .child("Runtime *"),
                                    )
                                    .child(
                                        div()
                                            .w_full()
                                            .h(px(32.0))
                                            .px(px(8.0))
                                            .border_1()
                                            .border_color(theme.border)
                                            .rounded(px(4.0))
                                            .bg(theme.background.opacity(0.3))
                                            .flex()
                                            .items_center()
                                            .child(
                                                div()
                                                    .text_size(px(13.0))
                                                    .text_color(theme.foreground.opacity(0.6))
                                                    .child("extism"),
                                            ),
                                    ),
                            )
                            .child(form_field("Version *", version_input, theme))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(4.0))
                                    .child(
                                        div()
                                            .text_size(px(13.0))
                                            .text_color(theme.foreground)
                                            .child("WASM 文件 *"),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(8.0))
                                            .child(
                                                action_button(
                                                    "pick-wasm",
                                                    "选择 WASM 文件",
                                                    ActionRole::Main,
                                                    ActionSize::Page,
                                                    style,
                                                )
                                                .on_mouse_down(MouseButton::Left, {
                                                    let t = cx.weak_entity();
                                                    move |_, _, cx| {
                                                        t.update(cx, |v, cx| v.pick_wasm_file(cx))
                                                            .ok();
                                                    }
                                                }),
                                            )
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .text_size(px(12.0))
                                                    .text_color(theme.foreground.opacity(0.6))
                                                    .child(
                                                        self.form_wasm_path
                                                            .as_ref()
                                                            .map(|p| {
                                                                p.file_name()
                                                                    .and_then(|n| n.to_str())
                                                                    .unwrap_or("")
                                                                    .to_string()
                                                            })
                                                            .unwrap_or_else(|| {
                                                                if self.editing_id.is_some() {
                                                                    "未选择新文件".to_string()
                                                                } else {
                                                                    "未选择文件".to_string()
                                                                }
                                                            }),
                                                    ),
                                            ),
                                    ),
                            )
                            .when_some(self.form_file_path.as_ref(), |this, path| {
                                this.child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(px(4.0))
                                        .child(
                                            div()
                                                .text_size(px(13.0))
                                                .text_color(theme.foreground)
                                                .child("文件地址"),
                                        )
                                        .child(
                                            div()
                                                .w_full()
                                                .min_h(px(32.0))
                                                .px(px(8.0))
                                                .py(px(6.0))
                                                .border_1()
                                                .border_color(theme.border)
                                                .rounded(px(4.0))
                                                .bg(theme.background.opacity(0.3))
                                                .text_size(px(11.0))
                                                .text_color(theme.foreground.opacity(0.7))
                                                .debug_selector(|| "PLUGIN_FILE_ADDRESS".to_owned())
                                                .child(path.display().to_string()),
                                        ),
                                )
                            })
                            .when(!exports.is_empty(), |this| {
                                this.child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(px(4.0))
                                        .child(
                                            div()
                                                .text_size(px(13.0))
                                                .text_color(theme.foreground)
                                                .child("exports"),
                                        )
                                        .child(
                                            div()
                                                .flex()
                                                .flex_wrap()
                                                .gap(px(6.0))
                                                .p(px(8.0))
                                                .border_1()
                                                .border_color(theme.border)
                                                .rounded(px(4.0))
                                                .debug_selector(|| "PLUGIN_EXPORTS".to_owned())
                                                .children(exports.iter().cloned().map(|export| {
                                                    div()
                                                        .px(px(7.0))
                                                        .py(px(3.0))
                                                        .rounded(px(4.0))
                                                        .bg(export_background)
                                                        .text_color(export_foreground)
                                                        .text_size(px(11.0))
                                                        .child(export)
                                                })),
                                        ),
                                )
                            })
                            .when(!self.form_sha256.is_empty(), |this| {
                                this.child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(px(4.0))
                                        .child(
                                            div()
                                                .text_size(px(13.0))
                                                .text_color(theme.foreground)
                                                .child("SHA256"),
                                        )
                                        .child(
                                            div()
                                                .w_full()
                                                .h(px(32.0))
                                                .px(px(8.0))
                                                .border_1()
                                                .border_color(theme.border)
                                                .rounded(px(4.0))
                                                .bg(theme.background.opacity(0.3))
                                                .flex()
                                                .items_center()
                                                .child(
                                                    div()
                                                        .text_size(px(12.0))
                                                        .text_color(theme.foreground.opacity(0.6))
                                                        .child(self.form_sha256.clone()),
                                                ),
                                        ),
                                )
                            })
                            .when(!self.form_size_bytes.is_empty(), |this| {
                                this.child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(px(4.0))
                                        .child(
                                            div()
                                                .text_size(px(13.0))
                                                .text_color(theme.foreground)
                                                .child("文件大小"),
                                        )
                                        .child(
                                            div()
                                                .w_full()
                                                .h(px(32.0))
                                                .px(px(8.0))
                                                .border_1()
                                                .border_color(theme.border)
                                                .rounded(px(4.0))
                                                .bg(theme.background.opacity(0.3))
                                                .flex()
                                                .items_center()
                                                .child(
                                                    div()
                                                        .text_size(px(12.0))
                                                        .text_color(theme.foreground.opacity(0.6))
                                                        .child(format!(
                                                            "{} bytes",
                                                            self.form_size_bytes
                                                        )),
                                                ),
                                        ),
                                )
                            })
                            .when_some(self.error_message.as_ref(), |this, err| {
                                this.child(
                                    div()
                                        .p(px(8.0))
                                        .bg(theme.warning.opacity(0.1))
                                        .rounded(px(4.0))
                                        .text_size(px(12.0))
                                        .text_color(theme.warning)
                                        .child(err.clone()),
                                )
                            })
                            .child(
                                div()
                                    .flex()
                                    .justify_end()
                                    .gap(px(8.0))
                                    .debug_selector(|| "PLUGIN_FORM_ACTIONS".to_owned())
                                    .child(
                                        action_button(
                                            "cancel",
                                            "取消",
                                            ActionRole::Neutral,
                                            ActionSize::Page,
                                            style,
                                        )
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            {
                                                let t = cx.weak_entity();
                                                move |_, _, cx| {
                                                    t.update(cx, |v, cx| v.hide_form(cx)).ok();
                                                }
                                            },
                                        ),
                                    )
                                    .child(
                                        action_button(
                                            "save",
                                            "保存",
                                            ActionRole::Main,
                                            ActionSize::Page,
                                            style,
                                        )
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            {
                                                let t = cx.weak_entity();
                                                move |_, _, cx| {
                                                    t.update(cx, |v, cx| v.save(cx)).ok();
                                                }
                                            },
                                        ),
                                    ),
                            ),
                    ),
                )
            })
            .when(self.confirm_delete_id.is_some(), |this| {
                let id = self.confirm_delete_id.unwrap();
                this.child(
                    div()
                        .absolute()
                        .top(px(0.0))
                        .left(px(0.0))
                        .right(px(0.0))
                        .bottom(px(0.0))
                        .bg(theme.overlay)
                        .opacity(0.3)
                        .on_mouse_down(MouseButton::Left, {
                            let t = cx.weak_entity();
                            move |_, _, cx| {
                                t.update(cx, |v, cx| {
                                    v.confirm_delete_id = None;
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
                                .bg(theme.popover)
                                .rounded(px(8.0))
                                .shadow_lg()
                                .border_1()
                                .border_color(theme.border)
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
                                                .text_color(theme.foreground)
                                                .child("确认删除"),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(14.0))
                                                .text_color(style.list.muted_foreground)
                                                .child("确定要删除这个插件吗？此操作不可恢复。"),
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
                                                        ActionSize::Page,
                                                        style,
                                                    )
                                                    .on_mouse_down(MouseButton::Left, {
                                                        let t = cx.weak_entity();
                                                        move |_, _, cx| {
                                                            t.update(cx, |v, cx| {
                                                                v.confirm_delete_id = None;
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
                                                        ActionSize::Page,
                                                        style,
                                                    )
                                                    .on_mouse_down(MouseButton::Left, {
                                                        let t = cx.weak_entity();
                                                        move |_, _, cx| {
                                                            t.update(cx, |v, cx| {
                                                                if let Some(did) =
                                                                    v.confirm_delete_id
                                                                {
                                                                    v.delete(did, cx);
                                                                    v.confirm_delete_id = None;
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

fn form_field(
    label: &'static str,
    input: Entity<InputState>,
    theme: &gpui_component::theme::Theme,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(4.0))
        .child(
            div()
                .text_size(px(13.0))
                .text_color(theme.foreground)
                .child(label),
        )
        .child(
            Input::new(&input)
                .w_full()
                .h(px(32.0))
                .px(px(8.0))
                .border_1()
                .border_color(theme.border)
                .rounded(px(4.0)),
        )
}

#[cfg(test)]
mod tests {
    use gpui::{
        AppContext, Entity, ScrollDelta, ScrollWheelEvent, TestAppContext, TouchPhase,
        VisualTestContext, point, px, size,
    };

    use super::PluginView;
    use crate::datasource::{Store, entity_store::Plugin};

    fn test_plugin(id: i64) -> Plugin {
        Plugin {
            id,
            identifier: "weather".into(),
            name: "Weather".into(),
            description: Some("Weather plugin".into()),
            manifest: Some(r#"{"exports":["run","health"]}"#.into()),
            runtime: "extism".into(),
            version: "1.0.0".into(),
            author: None,
            repository_url: None,
            s3_key: "plugins/weather/1.0.0.wasm".into(),
            sha256: "abc123".into(),
            size_bytes: 1024,
            category_id: None,
            created_at: "2026-07-20T00:00:00Z".into(),
            updated_at: "2026-07-20T00:00:00Z".into(),
            deleted_at: None,
        }
    }

    fn test_view(store: Entity<Store>) -> PluginView {
        PluginView {
            store,
            items: Vec::new(),
            loading: false,
            search_text: String::new(),
            current_page: 0,
            page_size: 20,
            total_count: 0,
            show_form: false,
            form_scroll: gpui::ScrollHandle::default(),
            editing_id: None,
            form_identifier: String::new(),
            form_name: String::new(),
            form_description: String::new(),
            form_version: String::new(),
            form_sha256: String::new(),
            form_size_bytes: String::new(),
            form_wasm_path: None,
            form_file_path: None,
            form_manifest: None,
            error_message: None,
            confirm_delete_id: None,
            identifier_input: None,
            name_input: None,
            description_input: None,
            version_input: None,
            sha256_input: None,
            size_bytes_input: None,
            search_input: None,
        }
    }

    #[test]
    fn edit_manifest_is_rebuilt_from_the_persisted_wasm_when_missing() {
        let temp_dir = tempfile::tempdir().expect("create temporary plugin directory");
        let mut plugin = test_plugin(42);
        plugin.manifest = None;
        let plugin_dir =
            Plugin::ensure_plugin_dir(temp_dir.path(), plugin.id).expect("create plugin directory");
        let wasm = wat::parse_str(
            r#"(module
                (func (export "run"))
                (func (export "health"))
            )"#,
        )
        .expect("build wasm fixture");
        std::fs::write(plugin_dir.join("plugin.wasm"), wasm).expect("write wasm fixture");

        let manifest = PluginView::manifest_for_edit(&plugin, temp_dir.path());
        assert_eq!(
            PluginView::manifest_exports(manifest.as_deref()),
            ["run", "health"]
        );
    }

    #[gpui::test]
    fn editing_plugin_records_the_local_wasm_address(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::theme::init(cx);
            gpui_component::init(cx);
        });

        let temp_dir = tempfile::tempdir().expect("create temporary data directory");
        let runtime = tokio::runtime::Runtime::new().expect("create Tokio runtime");
        let store = runtime
            .block_on(Store::new(temp_dir.path()))
            .expect("create test store");
        let store = cx.new(|_| store);
        let plugin = test_plugin(42);
        let expected = plugin.wasm_path(&Store::default_db_path());

        let window = cx.open_window(size(px(800.0), px(500.0)), move |window, cx| {
            let mut view = test_view(store.clone());
            view.show_edit_form(window, plugin, cx);
            assert_eq!(view.form_file_path.as_deref(), Some(expected.as_path()));
            view
        });
        cx.run_until_parked();

        let mut cx = VisualTestContext::from_window(window.into(), cx);
        assert!(cx.debug_bounds("PLUGIN_FILE_ADDRESS").is_some());
        assert!(cx.debug_bounds("PLUGIN_EXPORTS").is_some());
    }

    #[gpui::test]
    fn plugin_form_stays_inside_the_viewport_and_scrolls(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::theme::init(cx);
            gpui_component::init(cx);
        });

        let temp_dir = tempfile::tempdir().expect("create temporary data directory");
        let runtime = tokio::runtime::Runtime::new().expect("create Tokio runtime");
        let store = runtime
            .block_on(Store::new(temp_dir.path()))
            .expect("create test store");
        let store = cx.new(|_| store);

        let window = cx.open_window(size(px(800.0), px(500.0)), move |window, cx| {
            let mut view = test_view(store.clone());
            view.show_form = true;
            view.init_inputs(window, cx);
            view
        });
        cx.run_until_parked();

        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let modal = cx
            .debug_bounds("PLUGIN_MODAL")
            .expect("plugin modal bounds");
        let scroll = cx
            .debug_bounds("PLUGIN_FORM_SCROLL")
            .expect("plugin form scroll bounds");
        let actions_before = cx
            .debug_bounds("PLUGIN_FORM_ACTIONS")
            .expect("plugin form action bounds");

        assert!(modal.top() >= px(0.0));
        assert!(modal.bottom() <= px(500.0));
        assert_eq!(modal.left(), px(125.0));
        assert_eq!(modal.right(), px(675.0));
        assert!(scroll.bottom() <= modal.bottom());
        assert!(
            actions_before.right() <= scroll.right() - px(16.0),
            "plugin form content overlaps the scrollbar gutter: scroll={scroll:?}, actions={actions_before:?}"
        );

        cx.simulate_event(ScrollWheelEvent {
            position: point(scroll.left() + px(32.0), scroll.top() + px(32.0)),
            delta: ScrollDelta::Pixels(point(px(0.0), px(-1000.0))),
            modifiers: Default::default(),
            touch_phase: TouchPhase::Moved,
        });
        cx.run_until_parked();

        let actions_after = cx
            .debug_bounds("PLUGIN_FORM_ACTIONS")
            .expect("plugin form action bounds after scrolling");
        assert!(
            actions_after.top() < actions_before.top(),
            "plugin form did not scroll: before={actions_before:?}, after={actions_after:?}"
        );
        assert!(actions_after.bottom() <= modal.bottom());
    }
}
