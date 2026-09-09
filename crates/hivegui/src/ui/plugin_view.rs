//! scroll:plugin_list

use std::collections::{BTreeSet, HashSet};
use std::path::PathBuf;

use sha2::{Digest, Sha256};

use crate::datasource::{
    Store,
    entity_store::{Capability, Plugin},
    plugin_manifest::{
        build_v1_manifest, manifest_exports as extract_exports, manifest_required_capabilities,
        validate_manifest,
    },
    wasm_exports::extract_wasm_exports,
};
use crate::plugin::plugin_store::{PluginArtifactInput, PluginMetadata, PluginStore};
use crate::ui::management_style::{
    ActionRole, ActionSize, ManagementStyle, action_button, list_actions, list_cell,
    list_container, list_header, list_header_cell, list_row, management_modal_layer,
    management_modal_panel, management_modal_scroll_content,
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
    form_timeout_secs: String,
    form_memory_mb: String,
    form_output_mb: String,
    form_wasm_path: Option<PathBuf>,
    form_file_path: Option<PathBuf>,
    form_manifest: Option<String>,
    form_original_s3_key: String,
    capabilities: Vec<Capability>,
    form_capabilities: HashSet<String>,
    capability_select_open: bool,
    error_message: Option<String>,
    confirm_delete_id: Option<i64>,
    identifier_input: Option<Entity<InputState>>,
    name_input: Option<Entity<InputState>>,
    description_input: Option<Entity<InputState>>,
    version_input: Option<Entity<InputState>>,
    sha256_input: Option<Entity<InputState>>,
    size_bytes_input: Option<Entity<InputState>>,
    timeout_input: Option<Entity<InputState>>,
    memory_input: Option<Entity<InputState>>,
    output_input: Option<Entity<InputState>>,
    search_input: Option<Entity<InputState>>,
    form_focus: FocusHandle,
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
            form_timeout_secs: "30".into(),
            form_memory_mb: "128".into(),
            form_output_mb: "10".into(),
            form_wasm_path: None,
            form_file_path: None,
            form_manifest: None,
            form_original_s3_key: String::new(),
            capabilities: Vec::new(),
            form_capabilities: HashSet::new(),
            capability_select_open: false,
            error_message: None,
            confirm_delete_id: None,
            identifier_input: None,
            name_input: None,
            description_input: None,
            version_input: None,
            sha256_input: None,
            size_bytes_input: None,
            timeout_input: None,
            memory_input: None,
            output_input: None,
            search_input: None,
            form_focus: cx.focus_handle(),
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
        let manifest = build_v1_manifest(&exports, &[]);
        Ok((hex, size, manifest))
    }

    fn manifest_exports(manifest: Option<&str>) -> Vec<String> {
        extract_exports(manifest)
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
            let capabilities = Capability::list(store.pool(), None, 1_000, 0).await?;
            this.update(cx, |v, cx| {
                v.items = items;
                v.total_count = count;
                v.capabilities = capabilities;
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
        self.timeout_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("超时（秒）")
                .default_value(&self.form_timeout_secs)
        }));
        self.memory_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("内存上限（MiB）")
                .default_value(&self.form_memory_mb)
        }));
        self.output_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("输出上限（MiB）")
                .default_value(&self.form_output_mb)
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
        self.form_timeout_secs = "30".into();
        self.form_memory_mb = "128".into();
        self.form_output_mb = "10".into();
        self.form_wasm_path = None;
        self.form_file_path = None;
        self.form_manifest = None;
        self.form_original_s3_key.clear();
        self.form_capabilities.clear();
        self.capability_select_open = false;
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
        let (timeout, memory, output) = parse_resource_limits(&item.resource_limits);
        self.form_timeout_secs = timeout;
        self.form_memory_mb = memory;
        self.form_output_mb = output;
        self.form_wasm_path = None;
        self.form_file_path = Some(item.wasm_path(&base_dir));
        self.form_manifest = manifest;
        self.form_original_s3_key = item.s3_key.clone();
        self.form_capabilities = manifest_required_capabilities(item.manifest.as_deref())
            .into_iter()
            .collect();
        self.capability_select_open = false;
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
        self.timeout_input = None;
        self.memory_input = None;
        self.output_input = None;
        cx.notify();
    }

    /// Keyboard handler for the plugin form. Esc closes the form,
    /// Enter submits when the form is visible.
    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if !self.show_form {
            return;
        }
        match event.keystroke.key.as_str() {
            "escape" => self.hide_form(cx),
            "enter" => self.save(cx),
            _ => {}
        }
    }

    /// Toggle the capability picker dropdown.
    fn toggle_capability_select(&mut self, cx: &mut Context<Self>) {
        self.capability_select_open = !self.capability_select_open;
        cx.notify();
    }

    /// Toggle a capability in the required-capability set.
    fn toggle_capability(&mut self, name: &str, cx: &mut Context<Self>) {
        if !self.form_capabilities.remove(name) {
            self.form_capabilities.insert(name.to_string());
        }
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
        if let Some(ref inp) = self.timeout_input {
            self.form_timeout_secs = inp.read(cx).value().to_string();
        }
        if let Some(ref inp) = self.memory_input {
            self.form_memory_mb = inp.read(cx).value().to_string();
        }
        if let Some(ref inp) = self.output_input {
            self.form_output_mb = inp.read(cx).value().to_string();
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
        // Import / edit fork:
        // - new plugin (import): a WASM file is required (checked above).
        // - editing: the artifact file is immutable (identifier/version/runtime
        //   are locked in the UI), so the original s3_key, sha256 and size are
        //   preserved and only display + runtime metadata are updated.
        let original_s3_key = self.form_original_s3_key.clone();
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
        // Normalize the manifest to the v1 shape (migrating any legacy
        // `{"exports":[strings]}` record) and validate declared capabilities.
        let exports = Self::manifest_exports(self.form_manifest.as_deref());
        let mut required_caps: Vec<String> = self.form_capabilities.iter().cloned().collect();
        required_caps.sort();
        let normalized_manifest = Some(build_v1_manifest(&exports, &required_caps));
        let host_caps: BTreeSet<String> = self
            .capabilities
            .iter()
            .map(|cap| cap.name.clone())
            .collect();
        if let Err(issues) = validate_manifest(
            normalized_manifest.as_deref().unwrap_or_default(),
            &host_caps,
        ) {
            self.error_message = Some(format!(
                "Plugin manifest 不兼容（{}），请在导入前修复",
                issues.join(", ")
            ));
            cx.notify();
            return;
        }
        let wasm_path = self.form_wasm_path.clone();
        let capabilities_json =
            serde_json::to_string(&required_caps).unwrap_or_else(|_| "[]".to_string());
        let resource_limits = serialize_resource_limits(
            &self.form_timeout_secs,
            &self.form_memory_mb,
            &self.form_output_mb,
        );
        if let Some(eid) = self.editing_id {
            // T077: editing never replaces the WASM artifact. identifier/
            // version/runtime are locked in the UI, so the original s3_key,
            // sha256 and size are preserved and only metadata + limits update.
            let s3k = original_s3_key;
            cx.spawn(async move |this, cx| {
                match Plugin::update(
                    store.pool(),
                    eid,
                    idf,
                    name,
                    desc,
                    normalized_manifest.clone(),
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
                        if let Err(e) = Plugin::update_limits(
                            store.pool(),
                            eid,
                            capabilities_json.clone(),
                            resource_limits.clone(),
                        )
                        .await
                        {
                            tracing::error!("Failed to save resource limits: {}", e);
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
            let manifest = normalized_manifest;
            // T077: the controlled import walks the full durability state
            // machine (`prepared` → … → `done`) through `PluginStore::install`,
            // replacing the old `Plugin::create` + `set_artifact_key` +
            // `std::fs::copy` flow. The store writes the artifact to an
            // immutable no-replace key under `plugin_root` and creates the
            // live row with the full UI metadata in a single flow.
            let plugin_root = store.plugin_root().to_path_buf();
            cx.spawn(async move |this, cx| {
                let src = match wasm_path {
                    Some(path) => path,
                    None => {
                        this.update(cx, |v, cx| {
                            v.error_message = Some("请先选择 WASM 文件".to_string());
                            cx.notify();
                        })
                        .ok();
                        return;
                    }
                };
                let bytes = match std::fs::read(&src) {
                    Ok(b) => b,
                    Err(e) => {
                        this.update(cx, |v, cx| {
                            v.error_message = Some(format!("读取 WASM 文件失败: {e}"));
                            cx.notify();
                        })
                        .ok();
                        return;
                    }
                };
                let metadata = PluginMetadata::new(
                    name,
                    desc,
                    manifest,
                    rt,
                    capabilities_json,
                    resource_limits,
                );
                let input = match PluginArtifactInput::new(idf, ver, &bytes) {
                    Ok(i) => i,
                    Err(e) => {
                        this.update(cx, |v, cx| {
                            v.error_message = Some(format!("插件输入无效: {e}"));
                            cx.notify();
                        })
                        .ok();
                        return;
                    }
                };
                let plugin_store = match PluginStore::new(store.pool().clone(), plugin_root) {
                    Ok(s) => s,
                    Err(e) => {
                        this.update(cx, |v, cx| {
                            v.error_message = Some(format!("初始化插件存储失败: {e}"));
                            cx.notify();
                        })
                        .ok();
                        return;
                    }
                };
                match plugin_store.install_with_metadata(input, metadata).await {
                    Ok(_record) => {
                        this.update(cx, |v, cx| {
                            v.hide_form(cx);
                            v.load(cx);
                        })
                        .ok();
                    }
                    Err(e) => {
                        this.update(cx, |v, cx| {
                            v.error_message = Some(format!("导入失败: {e}"));
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
        let plugin_root = store.plugin_root().to_path_buf();
        cx.spawn(async move |this, cx| {
            // T079 ③: deletion goes through the controlled store — soft-delete
            // + `pending` GC ledger row + protected drain — instead of the
            // legacy `Plugin::delete` + immediate `remove_dir_all`.
            let plugin_store = match PluginStore::new(store.pool().clone(), plugin_root) {
                Ok(s) => s,
                Err(e) => {
                    this.update(cx, |v, cx| {
                        v.error_message = Some(format!("初始化插件存储失败: {e}"));
                        cx.notify();
                    })
                    .ok();
                    return;
                }
            };
            match plugin_store.delete(id).await {
                Ok(_) => {
                    this.update(cx, |v, cx| {
                        v.load(cx);
                    })
                    .ok();
                }
                Err(e) => {
                    this.update(cx, |v, cx| {
                        v.error_message = Some(format!("删除失败: {e}"));
                        cx.notify();
                    })
                    .ok();
                }
            }
        })
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
                cx.subscribe_in(input, window, |this, state, event, _window, cx| {
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
                let timeout_input = self.timeout_input.clone().unwrap();
                let memory_input = self.memory_input.clone().unwrap();
                let output_input = self.output_input.clone().unwrap();
                let capabilities = self.capabilities.clone();
                let selected_caps = self.form_capabilities.clone();
                let cap_select_open = self.capability_select_open;
                let capability_summary = if selected_caps.is_empty() {
                    "未声明（默认无权限）".to_string()
                } else {
                    let mut sorted: Vec<String> = selected_caps.iter().cloned().collect();
                    sorted.sort();
                    sorted.join(", ")
                };
                let form_scroll =
                    management_modal_scroll_content("plugin-form-scroll", &self.form_scroll);
                let editing = self.editing_id.is_some();
                let edit_identifier = self.form_identifier.clone();
                let edit_version = self.form_version.clone();
                let form_wasm_path = self.form_wasm_path.clone();
                let pick_weak = cx.weak_entity();
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
                        management_modal_layer(px(550.0), window.bounds().size.height - px(48.0)),
                        theme.popover,
                        theme.foreground,
                        theme.border,
                    )
                    .debug_selector(|| "PLUGIN_MODAL".to_owned())
                    .track_focus(&self.form_focus)
                    .on_key_down(cx.listener(|v, event: &KeyDownEvent, window, cx| {
                        v.on_key_down(event, window, cx);
                    }))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .child(
                        div()
                            .relative()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_h_0()
                            .overflow_hidden()
                            .debug_selector(|| "PLUGIN_SCROLLBAR_HOST".to_owned())
                            .child(
                                form_scroll
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
                                    .when(!editing, move |this| {
                                        this.child(form_field(
                                            "Identifier *",
                                            identifier_input.clone(),
                                            theme,
                                        ))
                                    })
                                    .when(editing, move |this| {
                                        this.child(form_field_readonly(
                                            "Identifier",
                                            &edit_identifier,
                                            theme,
                                        ))
                                    })
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
                                                            .text_color(
                                                                theme.foreground.opacity(0.6),
                                                            )
                                                            .child("extism"),
                                                    ),
                                            ),
                                    )
                                    .when(!editing, move |this| {
                                        this.child(form_field(
                                            "Version *",
                                            version_input.clone(),
                                            theme,
                                        ))
                                    })
                                    .when(editing, move |this| {
                                        this.child(form_field_readonly(
                                            "Version",
                                            &edit_version,
                                            theme,
                                        ))
                                    })
                                    .when(!editing, move |this| {
                                        this.child(
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
                                                            .on_mouse_down(
                                                                MouseButton::Left,
                                                                {
                                                                    let t = pick_weak;
                                                                    move |_, _, cx| {
                                                                        t.update(cx, |v, cx| {
                                                                            v.pick_wasm_file(cx)
                                                                        })
                                                                        .ok();
                                                                    }
                                                                },
                                                            ),
                                                        )
                                                        .child(
                                                            div()
                                                                .flex_1()
                                                                .text_size(px(12.0))
                                                                .text_color(
                                                                    theme.foreground.opacity(0.6),
                                                                )
                                                                .debug_selector(|| {
                                                                    "PLUGIN_WASM_SELECTION"
                                                                        .to_owned()
                                                                })
                                                                .child(
                                                                    form_wasm_path
                                                                        .as_ref()
                                                                        .map(|p| {
                                                                            p.file_name()
                                                                                .and_then(|n| {
                                                                                    n.to_str()
                                                                                })
                                                                                .unwrap_or("")
                                                                                .to_string()
                                                                        })
                                                                        .unwrap_or_else(|| {
                                                                            "未选择文件".to_string()
                                                                        }),
                                                                ),
                                                        ),
                                                ),
                                        )
                                    })
                                    .when(editing, move |this| {
                                        this.child(form_field_readonly(
                                            "WASM 文件",
                                            "不可修改（添加后文件不可替换）",
                                            theme,
                                        ))
                                    })
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
                                                        .debug_selector(|| {
                                                            "PLUGIN_FILE_ADDRESS".to_owned()
                                                        })
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
                                                        .debug_selector(|| {
                                                            "PLUGIN_EXPORTS".to_owned()
                                                        })
                                                        .children(exports.iter().cloned().map(
                                                            |export| {
                                                                div()
                                                                    .px(px(7.0))
                                                                    .py(px(3.0))
                                                                    .rounded(px(4.0))
                                                                    .bg(export_background)
                                                                    .text_color(export_foreground)
                                                                    .text_size(px(11.0))
                                                                    .child(export)
                                                            },
                                                        )),
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
                                                                .text_color(
                                                                    theme.foreground.opacity(0.6),
                                                                )
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
                                                                .text_color(
                                                                    theme.foreground.opacity(0.6),
                                                                )
                                                                .child(format!(
                                                                    "{} bytes",
                                                                    self.form_size_bytes
                                                                )),
                                                        ),
                                                ),
                                        )
                                    })
                                    .child(form_field("超时（秒）", timeout_input, theme))
                                    .child(form_field("内存上限（MiB）", memory_input, theme))
                                    .child(form_field("输出上限（MiB）", output_input, theme))
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap(px(4.0))
                                            .child(
                                                div()
                                                    .text_size(px(13.0))
                                                    .text_color(theme.foreground)
                                                    .child("所需 Capabilities"),
                                            )
                                            .child(
                                                div()
                                                    .w_full()
                                                    .border_1()
                                                    .border_color(theme.border)
                                                    .rounded(px(4.0))
                                                    .flex()
                                                    .flex_col()
                                                    .child(
                                                        div()
                                                            .w_full()
                                                            .h(px(32.0))
                                                            .px(px(8.0))
                                                            .flex()
                                                            .items_center()
                                                            .cursor(CursorStyle::PointingHand)
                                                            .debug_selector(|| {
                                                                "PLUGIN_CAPABILITY_SELECTOR"
                                                                    .to_owned()
                                                            })
                                                            .on_mouse_down(MouseButton::Left, {
                                                                let t = cx.weak_entity();
                                                                move |_, _, cx| {
                                                                    t.update(cx, |v, cx| {
                                                                        v.toggle_capability_select(
                                                                            cx,
                                                                        )
                                                                    })
                                                                    .ok();
                                                                }
                                                            })
                                                            .child(
                                                                div()
                                                                    .text_size(px(12.0))
                                                                    .text_color(
                                                                        theme.foreground.opacity(0.7),
                                                                    )
                                                                    .child(capability_summary),
                                                            ),
                                                    )
                                                    .when(cap_select_open, |this| {
                                                        this.child(
                                                            div()
                                                                .w_full()
                                                                .border_t_1()
                                                                .border_color(theme.border)
                                                                .flex()
                                                                .flex_col()
                                                                .children(capabilities.iter().map(
                                                                    |cap| {
                                                                        let name = cap.name.clone();
                                                                        let selected =
                                                                            selected_caps.contains(
                                                                                &name,
                                                                            );
                                                                        div()
                                                                            .w_full()
                                                                            .px(px(8.0))
                                                                            .py(px(5.0))
                                                                            .flex()
                                                                            .items_center()
                                                                            .gap(px(6.0))
                                                                            .cursor(
                                                                                CursorStyle::PointingHand,
                                                                            )
                                                                            .on_mouse_down(
                                                                                MouseButton::Left,
                                                                                {
                                                                                    let name =
                                                                                        name.clone();
                                                                                    let t =
                                                                                        cx.weak_entity(
                                                                                        );
                                                                                    move |_, _, cx| {
                                                                                        t.update(
                                                                                            cx,
                                                                                            |v,
                                                                                             cx| {
                                                                                                v.toggle_capability(
                                                                                                    &name,
                                                                                                    cx,
                                                                                                )
                                                                                            },
                                                                                        )
                                                                                        .ok();
                                                                                    }
                                                                                },
                                                                            )
                                                                            .child(
                                                                                div()
                                                                                    .w(px(10.0))
                                                                                    .h(px(10.0))
                                                                                    .rounded(px(2.0))
                                                                                    .border_1()
                                                                                    .border_color(
                                                                                        theme.border,
                                                                                    )
                                                                                    .bg(if selected {
                                                                                        style.action(
                                                                                            ActionRole::Main,
                                                                                        )
                                                                                        .background
                                                                                    } else {
                                                                                        theme.background
                                                                                    }),
                                                                            )
                                                                            .child(
                                                                                div()
                                                                                    .text_size(
                                                                                        px(12.0),
                                                                                    )
                                                                                    .text_color(
                                                                                        theme
                                                                                            .foreground,
                                                                                    )
                                                                                    .child(name),
                                                                            )
                                                                    },
                                                                )),
                                                        )
                                                    }),
                                            ),
                                    )
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
                                                .on_mouse_down(MouseButton::Left, {
                                                    let t = cx.weak_entity();
                                                    move |_, _, cx| {
                                                        t.update(cx, |v, cx| v.hide_form(cx)).ok();
                                                    }
                                                }),
                                            )
                                            .child(
                                                action_button(
                                                    "save",
                                                    "保存",
                                                    ActionRole::Main,
                                                    ActionSize::Page,
                                                    style,
                                                )
                                                .on_mouse_down(MouseButton::Left, {
                                                    let t = cx.weak_entity();
                                                    move |_, _, cx| {
                                                        t.update(cx, |v, cx| v.save(cx)).ok();
                                                    }
                                                }),
                                            ),
                                    ),
                            )
                            .vertical_scrollbar(&self.form_scroll),
                    ),
                )
            })
            .when(self.confirm_delete_id.is_some(), |this| {
                let _id = self.confirm_delete_id.unwrap();
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

/// Serialize the three resource-limit form fields into the plugin
/// `resource_limits` JSON object. The persisted shape matches the
/// plugin-abi contract: `{"timeout_ms", "memory_limit_mb",
/// "output_limit_bytes"}`. Invalid/empty inputs fall back to the
/// defaults (30 s / 128 MiB / 10 MiB).
fn serialize_resource_limits(timeout_secs: &str, memory_mb: &str, output_mb: &str) -> String {
    let timeout_ms = timeout_secs.trim().parse::<u64>().unwrap_or(30) * 1000;
    let memory = memory_mb.trim().parse::<u64>().unwrap_or(128);
    let output_bytes = output_mb.trim().parse::<u64>().unwrap_or(10) * 1024 * 1024;
    serde_json::json!({
        "timeout_ms": timeout_ms,
        "memory_limit_mb": memory,
        "output_limit_bytes": output_bytes,
    })
    .to_string()
}

/// Parse a plugin `resource_limits` JSON object back into the three
/// human-friendly form fields `(timeout_secs, memory_mb, output_mb)`.
/// Unknown/missing fields fall back to the defaults.
fn parse_resource_limits(json: &str) -> (String, String, String) {
    #[derive(serde::Deserialize)]
    struct Limits {
        #[serde(default)]
        timeout_ms: u64,
        #[serde(default)]
        memory_limit_mb: u64,
        #[serde(default)]
        output_limit_bytes: u64,
    }
    let defaults = Limits {
        timeout_ms: 30_000,
        memory_limit_mb: 128,
        output_limit_bytes: 10 * 1024 * 1024,
    };
    let limits = serde_json::from_str::<Limits>(json).unwrap_or(defaults);
    (
        (limits.timeout_ms / 1000).to_string(),
        limits.memory_limit_mb.to_string(),
        (limits.output_limit_bytes / (1024 * 1024)).to_string(),
    )
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

/// Read-only field rendering (same visual as the locked `Runtime` row).
/// Used for `identifier` / `version` during edit, where the plugin artifact
/// file is immutable.
fn form_field_readonly(
    label: &'static str,
    value: &str,
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
                        .child(value.to_string()),
                ),
        )
}

#[cfg(test)]
mod tests {
    use gpui::{
        AppContext, Entity, ScrollDelta, ScrollWheelEvent, TestAppContext, TouchPhase,
        VisualTestContext, point, px, size,
    };

    use std::collections::HashSet;

    use super::{PluginView, parse_resource_limits, serialize_resource_limits};
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
            capabilities: "[]".into(),
            resource_limits: "{}".into(),
            row_revision: 0,
            created_at: "2026-07-20T00:00:00Z".into(),
            updated_at: "2026-07-20T00:00:00Z".into(),
            deleted_at: None,
        }
    }

    #[test]
    fn resource_limits_serialize_parse_round_trip() {
        // Human-friendly fields round-trip through the persisted JSON shape.
        let json = serialize_resource_limits("60", "256", "20");
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(parsed["timeout_ms"], 60_000);
        assert_eq!(parsed["memory_limit_mb"], 256);
        assert_eq!(parsed["output_limit_bytes"], 20 * 1024 * 1024);

        let (timeout, memory, output) = parse_resource_limits(&json);
        assert_eq!(timeout, "60");
        assert_eq!(memory, "256");
        assert_eq!(output, "20");
    }

    #[test]
    fn resource_limits_parse_falls_back_to_defaults() {
        // Malformed or empty JSON falls back to 30 s / 128 MiB / 10 MiB.
        let (timeout, memory, output) = parse_resource_limits("not-json");
        assert_eq!(timeout, "30");
        assert_eq!(memory, "128");
        assert_eq!(output, "10");
    }

    fn test_view(store: Entity<Store>, cx: &mut gpui::Context<PluginView>) -> PluginView {
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
            form_timeout_secs: "30".into(),
            form_memory_mb: "128".into(),
            form_output_mb: "10".into(),
            form_wasm_path: None,
            form_file_path: None,
            form_manifest: None,
            form_original_s3_key: String::new(),
            capabilities: Vec::new(),
            form_capabilities: HashSet::new(),
            capability_select_open: false,
            error_message: None,
            confirm_delete_id: None,
            identifier_input: None,
            name_input: None,
            description_input: None,
            version_input: None,
            sha256_input: None,
            size_bytes_input: None,
            timeout_input: None,
            memory_input: None,
            output_input: None,
            search_input: None,
            form_focus: cx.focus_handle(),
        }
    }

    #[test]
    fn inspect_wasm_emits_v1_manifest() {
        let temp_dir = tempfile::tempdir().expect("create temporary directory");
        let wasm_path = temp_dir.path().join("plugin.wasm");
        let wasm = wat::parse_str(
            r#"(module
                (func (export "run"))
                (func (export "health"))
            )"#,
        )
        .expect("build wasm fixture");
        std::fs::write(&wasm_path, wasm).expect("write wasm fixture");

        let (_, _, manifest) = PluginView::inspect_wasm(&wasm_path).expect("inspect wasm");
        let value: serde_json::Value = serde_json::from_str(&manifest).expect("valid JSON");
        assert_eq!(value["abi_version"], "hive-extism/v1");
        assert_eq!(value["required_capabilities"], serde_json::json!([]));
        assert_eq!(
            PluginView::manifest_exports(Some(&manifest)),
            ["run", "health"]
        );
    }

    #[test]
    fn edit_manifest_is_rebuilt_from_the_persisted_wasm_when_missing() {
        let temp_dir = tempfile::tempdir().expect("create temporary plugin directory");
        let mut plugin = test_plugin(42);
        plugin.manifest = None;
        let plugin_dir = Plugin::ensure_plugin_dir(
            temp_dir.path(),
            &plugin.identifier,
            &plugin.version,
            plugin.id,
        )
        .expect("create plugin directory");
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
            let mut view = test_view(store.clone(), cx);
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
            let mut view = test_view(store.clone(), cx);
            view.show_form = true;
            view.init_inputs(window, cx);
            view
        });
        cx.run_until_parked();

        let typed_window = window;
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let modal = cx
            .debug_bounds("PLUGIN_MODAL")
            .expect("plugin modal bounds");
        let scroll = cx
            .debug_bounds("PLUGIN_FORM_SCROLL")
            .expect("plugin form scroll bounds");
        let scrollbar_host = cx
            .debug_bounds("PLUGIN_SCROLLBAR_HOST")
            .expect("GPUI Component scrollbar host bounds");
        let actions_before = cx
            .debug_bounds("PLUGIN_FORM_ACTIONS")
            .expect("plugin form action bounds");

        assert!(modal.top() >= px(0.0));
        assert!(modal.bottom() <= px(500.0));
        assert_eq!(modal.left(), px(125.0));
        assert_eq!(modal.right(), px(675.0));
        assert!(scroll.bottom() <= modal.bottom());
        assert_eq!(scrollbar_host, scroll);
        assert!(cx.debug_bounds("PLUGIN_SCROLL_UP").is_none());
        assert!(cx.debug_bounds("PLUGIN_SCROLL_DOWN").is_none());
        assert!(cx.debug_bounds("PLUGIN_SCROLLBAR_TRACK").is_none());
        let (offset_at_top, max_offset) = typed_window
            .update(&mut cx, |view, _, _| {
                (view.form_scroll.offset(), view.form_scroll.max_offset())
            })
            .expect("plugin view update at top");
        assert_eq!(offset_at_top.y, px(0.0));
        assert!(max_offset.y > px(0.0));
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

        let (offset_at_bottom, max_offset_at_bottom) = typed_window
            .update(&mut cx, |view, _, _| {
                (view.form_scroll.offset(), view.form_scroll.max_offset())
            })
            .expect("plugin view update at bottom");
        assert_eq!(offset_at_bottom.y, -max_offset_at_bottom.y);

        let actions_after = cx
            .debug_bounds("PLUGIN_FORM_ACTIONS")
            .expect("plugin form action bounds after scrolling");
        assert!(
            actions_after.top() < actions_before.top(),
            "plugin form did not scroll: before={actions_before:?}, after={actions_after:?}"
        );
        assert!(actions_after.bottom() <= modal.bottom());

        assert_eq!(
            cx.debug_bounds("PLUGIN_SCROLLBAR_HOST")
                .expect("GPUI Component scrollbar host bounds after scrolling"),
            scrollbar_host
        );

        cx.simulate_event(ScrollWheelEvent {
            position: point(scroll.left() + px(32.0), scroll.top() + px(32.0)),
            delta: ScrollDelta::Pixels(point(px(0.0), px(1000.0))),
            modifiers: Default::default(),
            touch_phase: TouchPhase::Moved,
        });
        cx.run_until_parked();
        let offset_back_at_top = typed_window
            .update(&mut cx, |view, _, _| view.form_scroll.offset())
            .expect("plugin view update after scrolling back to top");
        assert_eq!(offset_back_at_top.y, px(0.0));
        assert_eq!(
            cx.debug_bounds("PLUGIN_SCROLLBAR_HOST")
                .expect("GPUI Component scrollbar host bounds back at top"),
            scrollbar_host
        );
    }
}
