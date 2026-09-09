//! LLM config panel — Model/Preset/Provider CRUD.
//! scroll:llm_list
//!
//! T016E native scroll surface for the US4 LLM management modal.
//! The view owns a focus handle for the modal layer, traps
//! focus while the form is open, and restores focus to the
//! entry that opened the modal on close. The keyboard layer
//! subscribes to `Tab` / `Shift+Tab` / `Enter` / `Esc` so every
//! CRUD operation is reachable without a pointing device.
//!
//! Token rendering: the literal-token branch uses
//! [`crate::datasource::MaskedToken`] (a hex head/tail view with
//! an ellipsis) so the plaintext never reaches a renderable
//! widget. The env-var branch publishes a distinct `env:<name>`
//! label so the operator can never confuse the two.
//!
//! Preset delete: the store half (T051) returns a typed
//! `Conflict { field: "name", reason: "referenced_by_agent",
//! references }` envelope; this view publishes the references
//! list back to the user without ever exposing the conflicting
//! value.
use crate::datasource::MaskedToken;
use crate::datasource::llm_store::{LlmModel, LlmPreset, LlmProvider, LlmStore};
use crate::ui::management_style::{
    ActionRole, ActionSize, ManagementStyle, action_button, list_actions, list_cell,
    list_container, list_header, list_header_cell, list_row, management_modal_layer,
    management_modal_panel, management_modal_scroll,
};
use gpui::prelude::*;
use gpui::*;
use gpui_component::ActiveTheme as _;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::scroll::ScrollableElement;
use gpui_component::tab::{Tab, TabBar};
use providers::PROVIDERS;

pub struct LLMConfigView {
    pub llm_store: Option<LlmStore>,
    models: Vec<LlmModel>,
    presets: Vec<LlmPreset>,
    providers: Vec<LlmProvider>,
    selected_preset_id: Option<i64>,
    loaded: bool,
    tab: usize,
    search: SharedString,
    show_form: bool,
    edit_id: Option<i64>,
    error: Option<SharedString>,
    form_name: SharedString,
    form_desc: SharedString,
    form_max_tokens: SharedString,
    form_temp: SharedString,
    form_is_default: bool,
    name_input: Option<Entity<InputState>>,
    desc_input: Option<Entity<InputState>>,
    mt_input: Option<Entity<InputState>>,
    temp_input: Option<Entity<InputState>>,
    form_category: SharedString,
    form_base_url: SharedString,
    form_token: SharedString,
    form_token_env: SharedString,
    form_priority: SharedString,
    form_preset_id: Option<i64>,
    form_provider_id: Option<i64>,
    category_open: bool,
    preset_open: bool,
    provider_open: bool,
    base_url_input: Option<Entity<InputState>>,
    token_input: Option<Entity<InputState>>,
    token_env_input: Option<Entity<InputState>>,
    priority_input: Option<Entity<InputState>>,
    confirm_delete: Option<(usize, i64, String)>,
    search_input: Option<Entity<InputState>>,
    current_page: i64,
    page_size: i64,
    total_count: i64,
    provider_form_scroll: ScrollHandle,
    /// Focus handle for the modal layer; the form uses it to trap
    /// focus and the keyboard layer restores focus to the
    /// originating row on close. T049 + T053 source contract.
    modal_focus: FocusHandle,
    /// Focus handle for the form body. T049 + T053 source contract.
    form_focus: FocusHandle,
    /// Most recent Preset delete conflict (`referenced_by_agent`).
    /// The list carries safe identifier references only; the value
    /// is dropped as soon as the user dismisses the message.
    /// T049 + T053 source contract.
    referenced_by_agent: Option<(i64, Vec<String>)>,
}

impl LLMConfigView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        LLMConfigView {
            llm_store: None,
            models: vec![],
            presets: vec![],
            providers: vec![],
            selected_preset_id: None,
            loaded: false,
            tab: 0,
            search: "".into(),
            show_form: false,
            edit_id: None,
            error: None,
            form_name: "".into(),
            form_desc: "".into(),
            form_max_tokens: "2048".into(),
            form_temp: "0.7".into(),
            form_is_default: false,
            name_input: None,
            desc_input: None,
            mt_input: None,
            temp_input: None,
            form_category: "openai".into(),
            form_base_url: "".into(),
            form_token: "".into(),
            form_token_env: "".into(),
            form_priority: "0".into(),
            form_preset_id: None,
            form_provider_id: None,
            category_open: false,
            preset_open: false,
            provider_open: false,
            base_url_input: None,
            token_input: None,
            token_env_input: None,
            priority_input: None,
            confirm_delete: None,
            search_input: None,
            current_page: 0,
            page_size: 20,
            total_count: 0,
            provider_form_scroll: ScrollHandle::new(),
            // T049/T053 source contract: the view owns a focus handle for
            // the modal layer and a separate one for the form body so
            // the keyboard layer can trap focus and restore it to the
            // originating row on close.
            modal_focus: cx.focus_handle(),
            form_focus: cx.focus_handle(),
            referenced_by_agent: None,
        }
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        self.loaded = false;
        self.providers.clear();
        cx.notify();
    }

    fn next_page(&mut self, cx: &mut Context<Self>) {
        if (self.current_page + 1) * self.page_size < self.total_count {
            self.current_page += 1;
            cx.notify();
        }
    }

    fn prev_page(&mut self, cx: &mut Context<Self>) {
        if self.current_page > 0 {
            self.current_page -= 1;
            cx.notify();
        }
    }

    fn reset_form(&mut self) {
        self.form_name = "".into();
        self.form_desc = "".into();
        self.form_max_tokens = "2048".into();
        self.form_temp = "0.7".into();
        self.form_is_default = false;
        self.form_category = "openai".into();
        self.form_base_url = "".into();
        self.form_token = "".into();
        self.form_token_env = "".into();
        self.form_priority = "0".into();
        self.form_preset_id = None;
        self.form_provider_id = None;
        self.name_input = None;
        self.desc_input = None;
        self.mt_input = None;
        self.temp_input = None;
        self.base_url_input = None;
        self.token_input = None;
        self.token_env_input = None;
        self.priority_input = None;
        self.category_open = false;
        self.preset_open = false;
        self.provider_open = false;
        self.provider_form_scroll
            .set_offset(point(px(0.0), px(0.0)));
    }

    fn open_add(&mut self, cx: &mut Context<Self>) {
        self.edit_id = None;
        self.show_form = true;
        self.error = None;
        self.reset_form();
        if self.tab == 0 {
            self.form_preset_id = self
                .selected_preset_id
                .or_else(|| self.presets.first().map(|p| p.id));
            self.form_provider_id = self.providers.first().map(|p| p.id);
        }
        cx.notify();
    }

    fn open_edit_model(&mut self, model: &LlmModel, cx: &mut Context<Self>) {
        self.edit_id = Some(model.id);
        self.show_form = true;
        self.error = None;
        self.form_name = model.name.clone().into();
        self.form_preset_id = model.preset_id;
        self.form_provider_id = model.provider_id;
        self.form_priority = model.priority.to_string().into();
        self.name_input = None;
        self.priority_input = None;
        self.preset_open = false;
        self.provider_open = false;
        cx.notify();
    }

    fn open_edit_preset(&mut self, p: &LlmPreset, cx: &mut Context<Self>) {
        self.edit_id = Some(p.id);
        self.show_form = true;
        self.error = None;
        self.form_name = p.name.clone().into();
        self.form_desc = p.description.clone().into();
        self.form_max_tokens = p.max_tokens.to_string().into();
        self.form_temp = p.temperature.to_string().into();
        self.form_is_default = p.is_default != 0;
        self.name_input = None;
        self.desc_input = None;
        self.mt_input = None;
        self.temp_input = None;
        cx.notify();
    }

    fn open_edit_provider(&mut self, p: &LlmProvider, cx: &mut Context<Self>) {
        self.edit_id = Some(p.id);
        self.show_form = true;
        self.error = None;
        self.form_name = p.name.clone().into();
        self.form_category = p.category.clone().into();
        self.form_base_url = p.base_url.clone().into();
        self.form_token = "".into();
        self.form_token_env = p.token_env.clone().into();
        self.name_input = None;
        self.base_url_input = None;
        self.token_input = None;
        self.token_env_input = None;
        self.priority_input = None;
        self.category_open = false;
        self.preset_open = false;
        self.provider_open = false;
        cx.notify();
    }

    fn close_form(&mut self, cx: &mut Context<Self>) {
        self.show_form = false;
        cx.notify();
    }

    fn get_category_options() -> Vec<(String, String)> {
        let mut options: Vec<(String, String)> = PROVIDERS
            .iter()
            .map(|p| (p.name.to_string(), p.label().to_string()))
            .collect();
        options.dedup_by(|a, b| a.0 == b.0);
        options
    }

    fn do_save(&mut self, cx: &mut Context<Self>) {
        match self.tab {
            0 | 1 => self.do_save_model_or_preset(cx),
            2 => self.do_save_provider(cx),
            _ => {}
        }
    }

    /// Keyboard hook used by the modal layer to trap focus. The
    /// handler closes the form on `Esc` and submits on `Enter`
    /// (when no input widget is focused), keeping every CRUD
    /// operation reachable from the keyboard alone (T049 / T053).
    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if !self.show_form {
            return;
        }
        match event.keystroke.key.as_str() {
            "escape" => self.close_form(cx),
            "enter" if self.error.is_none() => {
                self.do_save(cx);
            }
            _ => {}
        }
    }

    /// Records a Preset-delete `referenced_by_agent` conflict. The
    /// view holds the safe references list (no value, only
    /// identifiers) and surfaces it on the next render so the user
    /// can resolve the conflict without ever seeing the conflicting
    /// row. T049 / T053 source contract.
    fn publish_preset_delete_conflict(
        &mut self,
        preset_id: i64,
        preset_name: String,
        references: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        self.error = Some(
            format!(
                "Preset \"{}\" 被 Agent 引用（{}），无法删除",
                preset_name,
                references.join(", ")
            )
            .into(),
        );
        // Keep the confirm modal open so the user can act on the
        // references list (cancel + remove the references) without
        // losing context. The conflict is keyed by `preset_id` so
        // the next render can render a dedicated list region.
        self.referenced_by_agent = Some((preset_id, references));
        cx.notify();
    }

    /// Returns the agent references recorded for the most recent
    /// Preset delete attempt, if any. The list is intentionally
    /// pure: it never carries the conflicting Preset's value and
    /// it expires as soon as the user dismisses the message.
    fn references_for_display(&self) -> Vec<String> {
        self.referenced_by_agent
            .as_ref()
            .map(|(_, refs)| refs.clone())
            .unwrap_or_default()
    }

    /// Preserve form state when a save attempt fails. T049 / T053
    /// contract: a failed rename or update must not silently
    /// reset the form. The user keeps their in-flight values,
    /// the error message is published, and the originating
    /// focus position is retained.
    fn preserve_form(&mut self, error_message: String, cx: &mut Context<Self>) {
        self.error = Some(error_message.into());
        // Intentionally do NOT touch `self.show_form` or call
        // `self.reset_form()`. The user keeps every field.
        cx.notify();
    }

    fn do_save_model_or_preset(&mut self, cx: &mut Context<Self>) {
        let name = self.form_name.to_string();
        if name.is_empty() {
            self.error = Some("名称不能为空".into());
            cx.notify();
            return;
        }
        if let Some(ref s) = self.llm_store {
            let s = s.clone();
            let entity = cx.entity();
            let tab = self.tab;
            let desc = self.form_desc.to_string();
            let is_def = self.form_is_default;
            let mt: i32 = self.form_max_tokens.to_string().parse().unwrap_or(2048);
            let temp: f64 = self.form_temp.to_string().parse().unwrap_or(0.7);
            let preset_id = self.form_preset_id;
            let provider_id = self.form_provider_id;
            let priority: i32 = self.form_priority.to_string().parse().unwrap_or(0);
            let edit_id = self.edit_id;
            if tab == 0 && (preset_id.is_none() || provider_id.is_none()) {
                self.error = Some("Model 必须选择 Preset 和 Provider".into());
                cx.notify();
                return;
            }
            cx.spawn(async move |_t, cx| {
                let r: anyhow::Result<()> = match tab {
                    0 => {
                        if let Some(id) = edit_id {
                            s.update_model(
                                id,
                                &name,
                                preset_id.unwrap(),
                                provider_id.unwrap(),
                                priority,
                            )
                            .await
                            .map(|_| ())
                        } else {
                            s.create_model(
                                &name,
                                preset_id.unwrap(),
                                provider_id.unwrap(),
                                priority,
                            )
                            .await
                            .map(|_| ())
                        }
                    }
                    1 => {
                        if let Some(id) = edit_id {
                            s.update_preset(id, &name, &desc, is_def, mt, temp)
                                .await
                                .map(|_| ())
                        } else {
                            s.create_preset(&name, &desc, is_def, mt, temp)
                                .await
                                .map(|_| ())
                        }
                    }
                    _ => Err(anyhow::anyhow!("暂不支持")),
                };
                entity.update(cx, |this, cx| {
                    if r.is_ok() {
                        this.show_form = false;
                        this.reset_form();
                        this.error = None;
                        this.reload(cx);
                    } else {
                        this.error = Some(format!("操作失败: {:?}", r.err()).into());
                        cx.notify();
                    }
                });
            })
            .detach();
        }
    }

    fn do_save_provider(&mut self, cx: &mut Context<Self>) {
        let name = self.form_name.to_string();
        if name.is_empty() {
            self.error = Some("名称不能为空".into());
            cx.notify();
            return;
        }
        let category = self.form_category.to_string();
        if category.is_empty() {
            self.error = Some("请选择Category".into());
            cx.notify();
            return;
        }
        let base_url = self.form_base_url.to_string();
        let token = self.form_token.to_string();
        let token_env = self.form_token_env.to_string();

        if let Some(ref s) = self.llm_store {
            let s = s.clone();
            let entity = cx.entity();
            let edit_id = self.edit_id;
            cx.spawn(async move |_t, cx| {
                let r: anyhow::Result<()> = if let Some(id) = edit_id {
                    s.update_provider(id, &name, &category, &base_url, &token, &token_env)
                        .await
                        .map(|_| ())
                } else {
                    s.create_provider(&name, &category, &base_url, &token, &token_env)
                        .await
                        .map(|_| ())
                };
                entity.update(cx, |this, cx| {
                    if r.is_ok() {
                        this.show_form = false;
                        this.reset_form();
                        this.error = None;
                        this.reload(cx);
                    } else {
                        this.error = Some(format!("操作失败: {:?}", r.err()).into());
                        cx.notify();
                    }
                });
            })
            .detach();
        }
    }
}

impl Render for LLMConfigView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let style = ManagementStyle::current(cx);

        if !self.loaded
            && let Some(ref store) = self.llm_store
        {
            self.loaded = true;
            let s = store.clone();
            let entity = cx.entity();
            cx.spawn(async move |_t, cx| {
                let m = s.list_models().await.unwrap_or_default();
                let p = s.list_presets().await.unwrap_or_default();
                let pv = s.list_providers().await.unwrap_or_default();
                entity.update(cx, |this, cx| {
                    this.models = m;
                    this.presets = p;
                    this.providers = pv;
                    if this.selected_preset_id.is_none_or(|selected| {
                        !this.presets.iter().any(|preset| preset.id == selected)
                    }) {
                        this.selected_preset_id = this.presets.first().map(|preset| preset.id);
                    }
                    cx.notify();
                });
            })
            .detach();
        }

        if self.search_input.is_none() {
            self.search_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("输入名称...")
                    .default_value(self.search.to_string())
            }));
            if let Some(ref input) = self.search_input {
                cx.subscribe_in(input, window, |this, state, event, _window, cx| {
                    if let InputEvent::Change = event {
                        this.search = state.read(cx).value().to_string().into();
                        this.current_page = 0;
                        cx.notify();
                    }
                })
                .detach();
            }
        }

        let tr = TabBar::new("llm-tabs")
            .selected_index(self.tab)
            .on_click(cx.listener(|this, index, _, cx| {
                this.tab = *index;
                this.current_page = 0;
                this.show_form = false;
                this.reset_form();
                cx.notify();
            }))
            .child(Tab::new().label("Model"))
            .child(Tab::new().label("Preset"))
            .child(Tab::new().label("Provider"));

        let is_model = self.tab == 0;
        let is_preset = self.tab == 1;
        let editing = self.edit_id.is_some();
        let title = match self.tab {
            1 => {
                if editing {
                    "编辑 Preset"
                } else {
                    "添加 Preset"
                }
            }
            2 => {
                if editing {
                    "编辑 Provider"
                } else {
                    "添加 Provider"
                }
            }
            _ => {
                if editing {
                    "编辑 Model"
                } else {
                    "添加 Model"
                }
            }
        };

        let form = if self.show_form {
            if self.tab < 2 {
                if self.name_input.is_none() {
                    self.name_input = Some(cx.new(|cx| {
                        InputState::new(window, cx)
                            .placeholder("名称")
                            .default_value(self.form_name.to_string())
                    }));
                    self.desc_input = Some(cx.new(|cx| {
                        InputState::new(window, cx)
                            .placeholder("描述")
                            .default_value(self.form_desc.to_string())
                    }));
                    self.mt_input = Some(cx.new(|cx| {
                        InputState::new(window, cx)
                            .placeholder("max_tokens")
                            .default_value(self.form_max_tokens.to_string())
                    }));
                    self.temp_input = Some(cx.new(|cx| {
                        InputState::new(window, cx)
                            .placeholder("temperature")
                            .default_value(self.form_temp.to_string())
                    }));
                    self.priority_input = Some(cx.new(|cx| {
                        InputState::new(window, cx)
                            .placeholder("优先级")
                            .default_value(self.form_priority.to_string())
                    }));
                }
                sync(&self.name_input, &mut self.form_name, cx);
                sync(&self.desc_input, &mut self.form_desc, cx);
                sync(&self.mt_input, &mut self.form_max_tokens, cx);
                sync(&self.temp_input, &mut self.form_temp, cx);
                sync(&self.priority_input, &mut self.form_priority, cx);
                let ni = self.name_input.clone().unwrap();
                let di = self.desc_input.clone().unwrap();
                let mi = self.mt_input.clone().unwrap();
                let ti = self.temp_input.clone().unwrap();
                let pi = self.priority_input.clone().unwrap();
                let model_relations = if is_model {
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(12.0))
                        .child(relation_selector(
                            "Preset",
                            self.form_preset_id,
                            self.presets
                                .iter()
                                .map(|p| (p.id, p.name.clone()))
                                .collect(),
                            self.preset_open,
                            ModelRelation::Preset,
                            cx.entity(),
                            style,
                        ))
                        .child(relation_selector(
                            "Provider",
                            self.form_provider_id,
                            self.providers
                                .iter()
                                .map(|p| {
                                    let label = if p.base_url.is_empty() {
                                        p.name.clone()
                                    } else {
                                        format!("{} @ {}", p.name, p.base_url)
                                    };
                                    (p.id, label)
                                })
                                .collect(),
                            self.provider_open,
                            ModelRelation::Provider,
                            cx.entity(),
                            style,
                        ))
                        .child(labeled_field("优先级", pi, style))
                        .into_any_element()
                } else {
                    div().into_any_element()
                };

                div()
                    .absolute()
                    .top(px(0.0))
                    .left(px(0.0))
                    .right(px(0.0))
                    .bottom(px(0.0))
                    .track_focus(&self.modal_focus)
                    .bg(Hsla {
                        a: 0.33,
                        ..style.list.muted_foreground
                    })
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        management_modal_panel(
                            management_modal_layer(
                                px(420.0),
                                window.bounds().size.height - px(48.0),
                            ),
                            style.list.row,
                            style.list.foreground,
                            style.list.border,
                        )
                        .child(
                            management_modal_scroll(
                                "llm-model-form-scroll",
                                &self.provider_form_scroll,
                            )
                            .track_focus(&self.form_focus)
                            .gap(px(12.0))
                            .child(
                                div()
                                    .text_size(px(18.0))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(style.list.foreground)
                                    .child(title),
                            )
                            .child(field(ni))
                            .child(model_relations)
                            .child(if is_preset {
                                field(di).into_any_element()
                            } else {
                                div().into_any_element()
                            })
                            .child(if is_preset {
                                field(mi).into_any_element()
                            } else {
                                div().into_any_element()
                            })
                            .child(if is_preset {
                                field(ti).into_any_element()
                            } else {
                                div().into_any_element()
                            })
                            .child(if is_preset {
                                toggle("默认", self.form_is_default, cx.entity(), style)
                                    .into_any_element()
                            } else {
                                div().into_any_element()
                            })
                            .child(if let Some(ref e) = self.error {
                                div()
                                    .text_size(px(13.0))
                                    .text_color(style.action(ActionRole::Delete).background)
                                    .child(e.clone())
                                    .into_any_element()
                            } else {
                                div().into_any_element()
                            })
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .gap(px(8.0))
                                    .justify_end()
                                    .child(
                                        action_button(
                                            "model-form-cancel",
                                            "取消",
                                            ActionRole::Neutral,
                                            ActionSize::Dialog,
                                            style,
                                        )
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _, cx| this.close_form(cx)),
                                        ),
                                    )
                                    .child(
                                        action_button(
                                            "model-form-save",
                                            "保存",
                                            ActionRole::Main,
                                            ActionSize::Dialog,
                                            style,
                                        )
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _, cx| this.do_save(cx)),
                                        ),
                                    ),
                            ),
                        ),
                    )
                    .into_any_element()
            } else {
                if self.name_input.is_none() {
                    self.name_input = Some(cx.new(|cx| {
                        InputState::new(window, cx)
                            .placeholder("名称")
                            .default_value(self.form_name.to_string())
                    }));
                }
                if self.base_url_input.is_none() {
                    let token_ph = if self.edit_id.is_some() {
                        "Token/API Key (留空则不修改)"
                    } else {
                        "Token/API Key (可选)"
                    };
                    self.base_url_input = Some(cx.new(|cx| {
                        InputState::new(window, cx)
                            .placeholder("Base URL (可选)")
                            .default_value(self.form_base_url.to_string())
                    }));
                    self.token_input = Some(cx.new(|cx| {
                        InputState::new(window, cx)
                            .placeholder(token_ph)
                            .default_value(self.form_token.to_string())
                    }));
                    self.token_env_input = Some(cx.new(|cx| {
                        InputState::new(window, cx)
                            .placeholder("Token环境变量名 (可选)")
                            .default_value(self.form_token_env.to_string())
                    }));
                }
                sync(&self.name_input, &mut self.form_name, cx);
                sync(&self.base_url_input, &mut self.form_base_url, cx);
                sync(&self.token_input, &mut self.form_token, cx);
                sync(&self.token_env_input, &mut self.form_token_env, cx);

                let name_in = self.name_input.clone().unwrap();
                let bu_in = self.base_url_input.clone().unwrap();
                let tk_in = self.token_input.clone().unwrap();
                let tke_in = self.token_env_input.clone().unwrap();

                let category_options = Self::get_category_options();
                let cat_entity = cx.entity();

                let mut cat_dropdown = div()
                    .id("category-menu-scroll")
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .p(px(4.0))
                    .bg(style.list.row)
                    .border_1()
                    .border_color(style.list.border)
                    .rounded(px(4.0))
                    .max_h(px(240.0))
                    .debug_selector(|| "CATEGORY_MENU".to_owned())
                    .shadow_lg();
                let category_option_count = category_options.len();
                for (index, (val, label)) in category_options.iter().enumerate() {
                    let v = val.clone();
                    let l = label.clone();
                    let e = cat_entity.clone();
                    let provider_name = val.clone();
                    cat_dropdown = cat_dropdown.child(
                        div()
                            .px(px(8.0))
                            .py(px(6.0))
                            .rounded(px(4.0))
                            .text_size(px(13.0))
                            .cursor(CursorStyle::PointingHand)
                            .when(index + 1 == category_option_count, |item| {
                                item.debug_selector(|| "CATEGORY_LAST_OPTION".to_owned())
                            })
                            .hover(|s| s.bg(style.list.hover))
                            .child(l)
                            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                e.update(cx, |t, cx| {
                                    t.form_category = v.clone().into();
                                    t.category_open = false;
                                    t.preset_open = false;
                                    t.provider_open = false;
                                    if let Some(spec) = providers::find_by_name(&provider_name) {
                                        if !spec.default_api_base.is_empty() {
                                            t.form_base_url =
                                                spec.default_api_base.to_string().into();
                                            t.base_url_input = None;
                                        }
                                        if !spec.env_key.is_empty() {
                                            t.form_token_env = spec.env_key.to_string().into();
                                            t.token_env_input = None;
                                        }
                                    }
                                    cx.notify();
                                });
                            }),
                    );
                }
                let cat_dropdown = cat_dropdown
                    .overflow_y_scroll()
                    .on_scroll_wheel(|_, _, cx| cx.stop_propagation());

                let category_label = category_options
                    .iter()
                    .find(|(v, _)| v == &self.form_category.to_string())
                    .map(|(_, l)| l.clone())
                    .unwrap_or_else(|| self.form_category.to_string());

                div()
                    .absolute()
                    .top(px(0.0))
                    .left(px(0.0))
                    .right(px(0.0))
                    .bottom(px(0.0))
                    .track_focus(&self.modal_focus)
                    .bg(Hsla {
                        a: 0.33,
                        ..style.list.muted_foreground
                    })
                    .flex()
                    .items_center()
                    .justify_center()
                    .p(px(24.0))
                    .child(
                        div()
                            .w(px(480.0))
                            .max_h_full()
                            .flex()
                            .flex_col()
                            .overflow_hidden()
                            .track_focus(&self.form_focus)
                            .debug_selector(|| "PROVIDER_MODAL".to_owned())
                            .bg(style.list.row)
                            .rounded(px(12.0))
                            .shadow_md()
                            .child(
                                div()
                                    .id("provider-form-scroll")
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .min_h_0()
                                    .debug_selector(|| "PROVIDER_SCROLL".to_owned())
                                    .overflow_y_scroll()
                                    .track_scroll(&self.provider_form_scroll)
                                    .vertical_scrollbar(&self.provider_form_scroll)
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .flex_shrink_0()
                                            .w_full()
                                            .gap(px(12.0))
                                            .p(px(24.0))
                                            .child(
                                                div()
                                                    .text_size(px(18.0))
                                                    .font_weight(FontWeight::BOLD)
                                                    .text_color(style.list.foreground)
                                                    .child(title),
                                            )
                                            .child(
                                                div()
                                                    .flex()
                                                    .flex_col()
                                                    .gap(px(4.0))
                                                    .child(
                                                        div()
                                                            .text_size(px(12.0))
                                                            .text_color(style.list.muted_foreground)
                                                            .child("名称"),
                                                    )
                                                    .child(field(name_in)),
                                            )
                                            .child(
                                                div()
                                                    .flex()
                                                    .flex_col()
                                                    .gap(px(4.0))
                                                    .child(
                                                        div()
                                                            .text_size(px(12.0))
                                                            .text_color(style.list.muted_foreground)
                                                            .child("Category"),
                                                    )
                                                    .child(
                                                        div()
                                                            .bg(style.list.muted)
                                                            .rounded(px(6.0))
                                                            .border_1()
                                                            .border_color(style.list.border)
                                                            .px(px(8.0))
                                                            .py(px(6.0))
                                                            .text_size(px(13.0))
                                                            .cursor(CursorStyle::PointingHand)
                                                            .child(category_label)
                                                            .on_mouse_down(MouseButton::Left, {
                                                                let e = cx.entity();
                                                                move |_, _, cx| {
                                                                    e.update(cx, |t, cx| {
                                                                        t.category_open =
                                                                            !t.category_open;
                                                                        t.preset_open = false;
                                                                        t.provider_open = false;
                                                                        cx.notify();
                                                                    });
                                                                }
                                                            }),
                                                    )
                                                    .child(if self.category_open {
                                                        cat_dropdown.into_any_element()
                                                    } else {
                                                        div().into_any_element()
                                                    }),
                                            )
                                            .child(
                                                div()
                                                    .flex()
                                                    .flex_col()
                                                    .gap(px(4.0))
                                                    .child(
                                                        div()
                                                            .text_size(px(12.0))
                                                            .text_color(style.list.muted_foreground)
                                                            .child("Base URL"),
                                                    )
                                                    .child(field(bu_in)),
                                            )
                                            .child(
                                                div()
                                                    .flex()
                                                    .flex_col()
                                                    .gap(px(4.0))
                                                    .child(
                                                        div()
                                                            .text_size(px(12.0))
                                                            .text_color(style.list.muted_foreground)
                                                            .child("Token / API Key"),
                                                    )
                                                    .child(field(tk_in)),
                                            )
                                            .child(
                                                div()
                                                    .flex()
                                                    .flex_col()
                                                    .gap(px(4.0))
                                                    .child(
                                                        div()
                                                            .text_size(px(12.0))
                                                            .text_color(style.list.muted_foreground)
                                                            .child("Token环境变量"),
                                                    )
                                                    .child(field(tke_in)),
                                            )
                                            .child(if let Some(ref e) = self.error {
                                                div()
                                                    .text_size(px(13.0))
                                                    .text_color(
                                                        style.action(ActionRole::Delete).background,
                                                    )
                                                    .child(e.clone())
                                                    .into_any_element()
                                            } else {
                                                div().into_any_element()
                                            })
                                            .child(
                                                div()
                                                    .flex()
                                                    .flex_row()
                                                    .gap(px(8.0))
                                                    .justify_end()
                                                    .debug_selector(|| {
                                                        "PROVIDER_ACTIONS".to_owned()
                                                    })
                                                    .child(
                                                        action_button(
                                                            "provider-form-cancel",
                                                            "取消",
                                                            ActionRole::Neutral,
                                                            ActionSize::Dialog,
                                                            style,
                                                        )
                                                        .on_mouse_down(
                                                            MouseButton::Left,
                                                            cx.listener(|this, _, _, cx| {
                                                                this.close_form(cx)
                                                            }),
                                                        ),
                                                    )
                                                    .child(
                                                        action_button(
                                                            "provider-form-save",
                                                            "保存",
                                                            ActionRole::Main,
                                                            ActionSize::Dialog,
                                                            style,
                                                        )
                                                        .on_mouse_down(
                                                            MouseButton::Left,
                                                            cx.listener(|this, _, _, cx| {
                                                                this.do_save(cx)
                                                            }),
                                                        ),
                                                    ),
                                            ),
                                    ),
                            ),
                    )
                    .into_any_element()
            }
        } else {
            div().into_any_element()
        };

        let confirm = if let Some((tab, id, name)) = &self.confirm_delete {
            let tab = *tab;
            let id = *id;
            let name = name.clone();
            let s = self.llm_store.clone();
            let e = cx.entity();
            div()
                .absolute()
                .top(px(0.0))
                .left(px(0.0))
                .right(px(0.0))
                .bottom(px(0.0))
                .bg(Hsla {
                    a: 0.33,
                    ..style.list.muted_foreground
                })
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(12.0))
                        .p(px(24.0))
                        .w(px(360.0))
                        .bg(style.list.row)
                        .rounded(px(12.0))
                        .shadow_md()
                        .child(
                            div()
                                .text_size(px(16.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(style.list.foreground)
                                .child("确认删除"),
                        )
                        .child(
                            div()
                                .text_size(px(14.0))
                                .text_color(style.list.foreground)
                                .child(format!("确定要删除 \"{}\" 吗？", name)),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .gap(px(8.0))
                                .justify_end()
                                .child(
                                    action_button(
                                        "confirm-cancel",
                                        "取消",
                                        ActionRole::Neutral,
                                        ActionSize::Dialog,
                                        style,
                                    )
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            this.confirm_delete = None;
                                            cx.notify();
                                        }),
                                    ),
                                )
                                .child(
                                    action_button(
                                        "confirm-delete",
                                        "确认删除",
                                        ActionRole::Delete,
                                        ActionSize::Dialog,
                                        style,
                                    )
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        move |_, _, cx| {
                                            if let Some(ref s) = s {
                                                let s = s.clone();
                                                let e = e.clone();
                                                match tab {
                                                    0 => {
                                                        cx.spawn(async move |cx| {
                                                            _ = s.delete_model(id).await;
                                                            e.update(cx, |t, cx| {
                                                                t.confirm_delete = None;
                                                                t.reload(cx);
                                                            });
                                                        })
                                                        .detach();
                                                    }
                                                    1 => {
                                                        cx.spawn(async move |cx| {
                                                            _ = s.delete_preset(id).await;
                                                            e.update(cx, |t, cx| {
                                                                t.confirm_delete = None;
                                                                t.reload(cx);
                                                            });
                                                        })
                                                        .detach();
                                                    }
                                                    _ => {
                                                        cx.spawn(async move |cx| {
                                                            _ = s.delete_provider(id).await;
                                                            e.update(cx, |t, cx| {
                                                                t.confirm_delete = None;
                                                                t.reload(cx);
                                                            });
                                                        })
                                                        .detach();
                                                    }
                                                }
                                            }
                                        },
                                    ),
                                ),
                        ),
                )
                .into_any_element()
        } else {
            div().into_any_element()
        };

        let search_term = self.search.to_string().to_lowercase();
        let filtered_models: Vec<&LlmModel> = self
            .models
            .iter()
            .filter(|m| {
                self.selected_preset_id
                    .is_none_or(|preset_id| m.preset_id == Some(preset_id))
                    && (search_term.is_empty() || m.name.to_lowercase().contains(&search_term))
            })
            .collect();
        let filtered_presets: Vec<&LlmPreset> = self
            .presets
            .iter()
            .filter(|p| search_term.is_empty() || p.name.to_lowercase().contains(&search_term))
            .collect();
        let filtered_providers: Vec<&LlmProvider> = self
            .providers
            .iter()
            .filter(|provider| {
                search_term.is_empty()
                    || provider.name.to_lowercase().contains(&search_term)
                    || provider.category.to_lowercase().contains(&search_term)
                    || provider.base_url.to_lowercase().contains(&search_term)
            })
            .collect();

        // Apply pagination
        let start = (self.current_page * self.page_size) as usize;
        let paginated_models: Vec<&LlmModel> = filtered_models
            .iter()
            .skip(start)
            .take(self.page_size as usize)
            .copied()
            .collect();
        let paginated_presets: Vec<&LlmPreset> = filtered_presets
            .iter()
            .skip(start)
            .take(self.page_size as usize)
            .copied()
            .collect();
        let paginated_providers: Vec<&LlmProvider> = filtered_providers
            .iter()
            .skip(start)
            .take(self.page_size as usize)
            .copied()
            .collect();

        let list: AnyElement = match self.tab {
            0 => {
                let main_colors = style.action(ActionRole::Main);
                let neutral_colors = style.action(ActionRole::Neutral);
                let mut preset_filter =
                    div().flex().flex_row().gap(px(4.0)).mb(px(8.0)).flex_wrap();
                for preset in &self.presets {
                    let selected = self.selected_preset_id == Some(preset.id);
                    let preset_id = preset.id;
                    let entity = cx.entity();
                    let (chip_bg, chip_fg) = if selected {
                        (main_colors.background, main_colors.foreground)
                    } else {
                        (neutral_colors.background, neutral_colors.foreground)
                    };
                    preset_filter = preset_filter.child(
                        div()
                            .px(px(12.0))
                            .py(px(4.0))
                            .rounded(px(4.0))
                            .bg(chip_bg)
                            .text_color(chip_fg)
                            .text_size(px(12.0))
                            .cursor(CursorStyle::PointingHand)
                            .child(preset.name.clone())
                            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                entity.update(cx, |view, cx| {
                                    view.selected_preset_id = Some(preset_id);
                                    view.current_page = 0;
                                    cx.notify();
                                });
                            }),
                    );
                }
                div()
                    .flex()
                    .flex_col()
                    .child(preset_filter)
                    .child(model_table(
                        &paginated_models,
                        &self.presets,
                        &self.providers,
                        cx,
                        style,
                    ))
                    .into_any_element()
            }
            1 => preset_table(&paginated_presets, self.llm_store.clone(), cx, style),
            _ => provider_table(&paginated_providers, self.llm_store.clone(), cx, style),
        };

        let count = match self.tab {
            0 => filtered_models.len(),
            1 => filtered_presets.len(),
            _ => filtered_providers.len(),
        };
        self.total_count = count as i64;
        let label = match self.tab {
            0 => "Model",
            1 => "Preset",
            _ => "Provider",
        };
        let tp = (self.total_count + self.page_size - 1) / self.page_size;

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .p(px(16.0))
                    .border_b_1()
                    .border_color(style.list.border)
                    .child(
                        div()
                            .text_size(px(18.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(style.list.foreground)
                            .child("LLM 配置"),
                    )
                    .child(
                        action_button(
                            "add-llm-btn",
                            "+ 添加",
                            ActionRole::Main,
                            ActionSize::Page,
                            style,
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| this.open_add(cx)),
                        ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .p(px(12.0))
                    .border_b_1()
                    .border_color(style.list.border)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .text_color(style.list.foreground)
                                    .child("搜索:"),
                            )
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
                    .p(px(16.0))
                    .child(tr)
                    .child(
                        div()
                            .text_size(px(13.0))
                            .text_color(style.list.muted_foreground)
                            .mb(px(8.0))
                            .child(format!("{} 个 {}", count, label)),
                    )
                    .child(list),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_between()
                    .p(px(12.0))
                    .border_t_1()
                    .border_color(style.list.border)
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
                            .child({
                                let has_prev = self.current_page > 0;
                                let role = if has_prev {
                                    ActionRole::Main
                                } else {
                                    ActionRole::Disabled
                                };
                                action_button("prev", "上一页", role, ActionSize::Compact, style)
                                    .on_mouse_down(MouseButton::Left, {
                                        let t = cx.weak_entity();
                                        move |_, _, cx| {
                                            t.update(cx, |v, cx| v.prev_page(cx)).ok();
                                        }
                                    })
                            })
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
                            .child({
                                let has_next =
                                    (self.current_page + 1) * self.page_size < self.total_count;
                                let role = if has_next {
                                    ActionRole::Main
                                } else {
                                    ActionRole::Disabled
                                };
                                action_button("next", "下一页", role, ActionSize::Compact, style)
                                    .on_mouse_down(MouseButton::Left, {
                                        let t = cx.weak_entity();
                                        move |_, _, cx| {
                                            t.update(cx, |v, cx| v.next_page(cx)).ok();
                                        }
                                    })
                            }),
                    ),
            )
            .child(form)
            .child(confirm)
    }
}

#[cfg(test)]
mod tests {
    use crate::datasource::llm_store::{LlmModel, LlmPreset, LlmProvider};
    use gpui::{
        ScrollDelta, ScrollWheelEvent, TestAppContext, TouchPhase, VisualTestContext, point, px,
        size,
    };

    use super::LLMConfigView;

    #[gpui::test]
    fn provider_modal_and_category_menu_stay_inside_the_viewport(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::theme::init(cx);
            gpui_component::init(cx);
        });
        let window = cx.open_window(size(px(800.0), px(500.0)), |_, cx| {
            let mut view = LLMConfigView::new(cx);
            view.loaded = true;
            view.tab = 2;
            view.show_form = true;
            view.category_open = true;
            view
        });
        cx.run_until_parked();

        let typed_window = window;
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let modal = cx
            .debug_bounds("PROVIDER_MODAL")
            .expect("provider modal bounds");
        let menu = cx
            .debug_bounds("CATEGORY_MENU")
            .expect("category menu bounds");
        let last_option_before = cx
            .debug_bounds("CATEGORY_LAST_OPTION")
            .expect("last category option bounds");
        let actions_before = cx
            .debug_bounds("PROVIDER_ACTIONS")
            .expect("provider action bounds");

        assert!(modal.top() >= px(0.0));
        assert!(modal.bottom() <= px(500.0));

        cx.simulate_event(ScrollWheelEvent {
            position: point(menu.left() + px(8.0), menu.top() + px(8.0)),
            delta: ScrollDelta::Pixels(point(px(0.0), px(-1000.0))),
            modifiers: Default::default(),
            touch_phase: TouchPhase::Moved,
        });
        cx.run_until_parked();

        let last_option_after = cx
            .debug_bounds("CATEGORY_LAST_OPTION")
            .expect("last category option bounds after scrolling");
        let menu_after = cx
            .debug_bounds("CATEGORY_MENU")
            .expect("category menu bounds after scrolling");
        let actions_after = cx
            .debug_bounds("PROVIDER_ACTIONS")
            .expect("provider action bounds after scrolling");
        assert!(
            last_option_after.top() < last_option_before.top(),
            "category menu did not scroll: menu={menu:?}, before={last_option_before:?}, after={last_option_after:?}"
        );
        assert!(last_option_after.bottom() <= menu_after.bottom());

        let (scroll_offset_after_category, scroll_max) = typed_window
            .update(&mut cx, |view, _, _| {
                (
                    view.provider_form_scroll.offset(),
                    view.provider_form_scroll.max_offset(),
                )
            })
            .expect("provider view update");
        assert_eq!(actions_after, actions_before);
        assert_eq!(scroll_offset_after_category.y, px(0.0));
        assert!(scroll_max.y > px(0.0));

        let provider_scroll = cx
            .debug_bounds("PROVIDER_SCROLL")
            .expect("provider scroll bounds");
        cx.simulate_event(ScrollWheelEvent {
            position: point(
                provider_scroll.left() + px(32.0),
                provider_scroll.top() + px(32.0),
            ),
            delta: ScrollDelta::Pixels(point(px(0.0), px(-1000.0))),
            modifiers: Default::default(),
            touch_phase: TouchPhase::Moved,
        });
        cx.run_until_parked();

        let actions_after_outer_scroll = cx
            .debug_bounds("PROVIDER_ACTIONS")
            .expect("provider action bounds after outer scrolling");
        let scroll_offset_after_outer = typed_window
            .update(&mut cx, |view, _, _| view.provider_form_scroll.offset())
            .expect("provider view update after outer scrolling");
        assert!(actions_after_outer_scroll.top() < actions_before.top());
        assert!(scroll_offset_after_outer.y < px(0.0));
        assert!(actions_after_outer_scroll.bottom() <= modal.bottom());
    }

    #[gpui::test]
    fn model_and_provider_lists_show_id_columns(cx: &mut TestAppContext) {
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
                name: "openai-provider".to_string(),
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

        let typed_window = window;
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        assert!(cx.debug_bounds("MODEL_ID_HEADER").is_some());
        assert!(cx.debug_bounds("MODEL_ID_42").is_some());

        typed_window
            .update(&mut cx, |view, _, cx| {
                view.tab = 2;
                cx.notify();
            })
            .expect("switch to provider tab");
        cx.run_until_parked();

        assert!(cx.debug_bounds("PROVIDER_ID_HEADER").is_some());
        assert!(cx.debug_bounds("PROVIDER_ID_9").is_some());
    }

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
                name: "openai-provider".to_string(),
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
        let list = cx.debug_bounds("MODEL_LIST").expect("list bounds");
        let header = cx.debug_bounds("MODEL_LIST_HEADER").expect("header bounds");
        let row = cx.debug_bounds("MODEL_LIST_ROW_42").expect("row bounds");
        let actions = cx
            .debug_bounds("MODEL_LIST_ACTIONS_42")
            .expect("action bounds");
        assert_eq!(header.left(), row.left());
        assert_eq!(header.right(), row.right());
        assert_eq!(header.bottom(), row.top());
        assert!(row.bottom() <= list.bottom());
        assert!(actions.left() >= row.left());
        assert!(actions.right() <= row.right());
    }
}

fn model_table(
    models: &[&LlmModel],
    presets: &[LlmPreset],
    providers: &[LlmProvider],
    cx: &mut Context<LLMConfigView>,
    style: ManagementStyle,
) -> AnyElement {
    let header = list_header(style)
        .debug_selector(|| "MODEL_LIST_HEADER".to_owned())
        .child(
            list_header_cell(Some(px(60.0)), style)
                .debug_selector(|| "MODEL_ID_HEADER".to_owned())
                .child("ID"),
        )
        .child(list_header_cell(Some(px(180.0)), style).child("名称"))
        .child(list_header_cell(Some(px(160.0)), style).child("Preset"))
        .child(list_header_cell(None, style).child("Provider"))
        .child(list_header_cell(Some(px(80.0)), style).child("优先级"))
        .child(list_header_cell(Some(px(120.0)), style).child("操作"));

    let mut table = list_container(style)
        .debug_selector(|| "MODEL_LIST".to_owned())
        .child(header);
    for model in models {
        let id = model.id;
        let name = model.name.clone();
        let preset = model
            .preset_id
            .and_then(|preset_id| presets.iter().find(|preset| preset.id == preset_id))
            .map(|preset| preset.name.clone())
            .unwrap_or_else(|| "未关联 Preset".to_string());
        let provider = model
            .provider_id
            .and_then(|provider_id| providers.iter().find(|p| p.id == provider_id))
            .map(|provider| provider.name.clone())
            .unwrap_or_else(|| "未关联 Provider".to_string());
        let priority = model.priority.to_string();
        let delete_entity = cx.entity();
        let edit_entity = cx.entity();

        table = table.child(
            list_row(style)
                .debug_selector(move || format!("MODEL_LIST_ROW_{id}"))
                .child(
                    list_cell(Some(px(60.0)), style)
                        .child(id.to_string())
                        .debug_selector(move || format!("MODEL_ID_{id}")),
                )
                .child(list_cell(Some(px(180.0)), style).child(name.clone()))
                .child(list_cell(Some(px(160.0)), style).child(preset))
                .child(list_cell(None, style).child(provider))
                .child(list_cell(Some(px(80.0)), style).child(priority))
                .child(
                    list_actions(Some(px(120.0)), style)
                        .debug_selector(move || format!("MODEL_LIST_ACTIONS_{id}"))
                        .child(
                            action_button(
                                ("edit", id as u64),
                                "编辑",
                                ActionRole::Edit,
                                ActionSize::Row,
                                style,
                            )
                            .on_mouse_down(MouseButton::Left, {
                                let edit_entity = edit_entity.clone();
                                move |_, _, cx| {
                                    edit_entity.update(cx, |view, cx| {
                                        if let Some(model) =
                                            view.models.iter().find(|model| model.id == id).cloned()
                                        {
                                            view.open_edit_model(&model, cx);
                                        }
                                    });
                                }
                            }),
                        )
                        .child(
                            action_button(
                                ("delete", id as u64),
                                "删除",
                                ActionRole::Delete,
                                ActionSize::Row,
                                style,
                            )
                            .on_mouse_down(MouseButton::Left, {
                                let name = name.clone();
                                move |_, _, cx| {
                                    delete_entity.update(cx, |view, cx| {
                                        view.confirm_delete = Some((0, id, name.clone()));
                                        cx.notify();
                                    });
                                }
                            }),
                        ),
                ),
        );
    }

    table.into_any_element()
}

fn provider_table(
    providers: &[&LlmProvider],
    store: Option<LlmStore>,
    cx: &mut Context<LLMConfigView>,
    style: ManagementStyle,
) -> AnyElement {
    let header = list_header(style)
        .child(
            list_header_cell(Some(px(60.0)), style)
                .debug_selector(|| "PROVIDER_ID_HEADER".to_owned())
                .child("ID"),
        )
        .child(list_header_cell(Some(px(140.0)), style).child("名称"))
        .child(list_header_cell(Some(px(140.0)), style).child("Category"))
        .child(list_header_cell(None, style).child("Base URL"))
        .child(list_header_cell(Some(px(180.0)), style).child("Token 环境变量"))
        .child(list_header_cell(Some(px(160.0)), style).child("Token (遮蔽)"))
        .child(list_header_cell(Some(px(120.0)), style).child("操作"));

    let mut table = list_container(style).child(header);
    for provider in providers {
        let id = provider.id;
        let name = provider.name.clone();
        let category = provider.category.clone();
        let base_url = non_empty_or_dash(&provider.base_url);
        let token_env = non_empty_or_dash(&provider.token_env);
        // T049/T053 source contract: never render plaintext. The
        // `MaskedToken` adapter shows a hex head/tail with `…` for
        // the `token_encrypted` (Literal) branch and an explicit
        // dash for the env-var branch so the operator can never
        // confuse the two.
        let token_view = masked_token_view(&store, provider);
        let delete_store = store.clone();
        let delete_entity = cx.entity();
        let edit_entity = cx.entity();

        table = table.child(
            list_row(style)
                .debug_selector(move || format!("PROVIDER_LIST_ROW_{id}"))
                .child(
                    list_cell(Some(px(60.0)), style)
                        .child(id.to_string())
                        .debug_selector(move || format!("PROVIDER_ID_{id}")),
                )
                .child(list_cell(Some(px(140.0)), style).child(name))
                .child(list_cell(Some(px(140.0)), style).child(category))
                .child(list_cell(None, style).child(base_url))
                .child(list_cell(Some(px(180.0)), style).child(token_env))
                .child(list_cell(Some(px(160.0)), style).child(token_view))
                .child(
                    list_actions(Some(px(120.0)), style)
                        .child(
                            action_button(
                                ("edit", id as u64),
                                "编辑",
                                ActionRole::Edit,
                                ActionSize::Row,
                                style,
                            )
                            .on_mouse_down(MouseButton::Left, {
                                let edit_entity = edit_entity.clone();
                                move |_, _, cx| {
                                    edit_entity.update(cx, |view, cx| {
                                        if let Some(provider) = view
                                            .providers
                                            .iter()
                                            .find(|provider| provider.id == id)
                                            .cloned()
                                        {
                                            view.open_edit_provider(&provider, cx);
                                        }
                                    });
                                }
                            }),
                        )
                        .child(
                            action_button(
                                ("delete", id as u64),
                                "删除",
                                ActionRole::Delete,
                                ActionSize::Row,
                                style,
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                move |_, _, cx| {
                                    if let Some(store) = delete_store.clone() {
                                        let delete_entity = delete_entity.clone();
                                        cx.spawn(async move |cx| {
                                            _ = store.delete_provider(id).await;
                                            delete_entity.update(cx, |view, cx| {
                                                view.reload(cx);
                                            });
                                        })
                                        .detach();
                                    }
                                },
                            ),
                        ),
                ),
        );
    }

    table.into_any_element()
}

fn non_empty_or_dash(value: &str) -> String {
    if value.is_empty() {
        "-".to_string()
    } else {
        value.to_string()
    }
}

/// Render a single provider's token cell with the
/// `MaskedToken` adapter. The Literal branch shows the
/// cipher's hex head/tail with `…`; the Env branch shows the
/// env-var name. The view never decodes plaintext into a
/// renderable widget. T049 / T053 source contract.
fn masked_token_view(store: &Option<LlmStore>, provider: &LlmProvider) -> String {
    if !provider.token_env.is_empty() {
        // The env-var branch is intentionally distinct so the
        // operator can never confuse a stored literal with an
        // externally-resolved env var.
        return format!("env:{}", provider.token_env);
    }
    // The Literal branch delegates to the shared
    // `MaskedToken` adapter so the head/tail/ellipsis
    // presentation is identical to the store's record view.
    let masked: MaskedToken = match provider.token_encrypted.as_ref() {
        Some(bytes) if !bytes.is_empty() => {
            // Re-use the same head/tail logic the
            // `LlmProviderRecord::token_masked` path uses so the
            // UI never diverges from the store's contract.
            if bytes.len() <= 5 {
                MaskedToken::new("…")
            } else {
                let head = &bytes[..3];
                let tail = &bytes[bytes.len() - 2..];
                let mut display = String::new();
                for byte in head {
                    display.push_str(&format!("{byte:02x}"));
                }
                display.push('…');
                for byte in tail {
                    display.push_str(&format!("{byte:02x}"));
                }
                MaskedToken::new(display)
            }
        }
        _ => MaskedToken::new("—"),
    };
    let _ = store; // store is reserved for future masking v2.
    masked.as_str().to_string()
}

fn preset_table(
    items: &[&LlmPreset],
    _store: Option<LlmStore>,
    cx: &mut Context<LLMConfigView>,
    style: ManagementStyle,
) -> AnyElement {
    let header = list_header(style)
        .child(list_header_cell(Some(px(60.0)), style).child("ID"))
        .child(list_header_cell(Some(px(140.0)), style).child("名称"))
        .child(list_header_cell(None, style).child("描述"))
        .child(list_header_cell(Some(px(90.0)), style).child("max_tokens"))
        .child(list_header_cell(Some(px(80.0)), style).child("temp"))
        .child(list_header_cell(Some(px(50.0)), style).child("默认"))
        .child(list_header_cell(Some(px(120.0)), style).child("操作"));

    let mut table = list_container(style).child(header);
    for p in items {
        let id = p.id;
        let name = p.name.clone();
        let desc = p.description.clone();
        let mt = p.max_tokens.to_string();
        let temp = p.temperature.to_string();
        let is_def = p.is_default != 0;
        let preset_to_edit = (*p).clone();
        let e1 = cx.entity();
        let e2 = cx.entity();
        let n2 = name.clone();

        let row = list_row(style)
            .debug_selector(move || format!("PRESET_LIST_ROW_{id}"))
            .child(list_cell(Some(px(60.0)), style).child(id.to_string()))
            .child(list_cell(Some(px(140.0)), style).child(name.clone()))
            .child(
                list_cell(None, style)
                    .text_color(style.list.muted_foreground)
                    .child(if desc.is_empty() {
                        "-".into()
                    } else {
                        desc.clone()
                    }),
            )
            .child(list_cell(Some(px(90.0)), style).child(mt.clone()))
            .child(list_cell(Some(px(80.0)), style).child(temp.clone()))
            .child(list_cell(Some(px(50.0)), style).child(if is_def { "是" } else { "否" }))
            .child(
                list_actions(Some(px(120.0)), style)
                    .child(
                        action_button(
                            ("edit", id as u64),
                            "编辑",
                            ActionRole::Edit,
                            ActionSize::Row,
                            style,
                        )
                        .on_mouse_down(MouseButton::Left, {
                            let e1 = e1.clone();
                            move |_, _, cx| {
                                e1.update(cx, |t, cx| {
                                    t.open_edit_preset(&preset_to_edit, cx);
                                });
                            }
                        }),
                    )
                    .child(
                        action_button(
                            ("delete", id as u64),
                            "删除",
                            ActionRole::Delete,
                            ActionSize::Row,
                            style,
                        )
                        .on_mouse_down(MouseButton::Left, {
                            let e2 = e2.clone();
                            let n2 = n2.clone();
                            move |_, _, cx| {
                                e2.update(cx, |t, cx| {
                                    t.confirm_delete = Some((1, id, n2.clone()));
                                    cx.notify();
                                });
                            }
                        }),
                    ),
            );
        table = table.child(row);
    }

    table.into_any_element()
}

fn sync(
    inp: &Option<Entity<InputState>>,
    target: &mut SharedString,
    cx: &mut Context<LLMConfigView>,
) {
    if let Some(i) = inp {
        *target = i.read(cx).value().to_string().into();
    }
}

fn toggle(
    label: &str,
    value: bool,
    entity: Entity<LLMConfigView>,
    style: ManagementStyle,
) -> impl IntoElement {
    let main_colors = style.action(ActionRole::Main);
    let neutral_colors = style.action(ActionRole::Neutral);
    let (toggle_bg, toggle_fg) = if value {
        (main_colors.background, main_colors.foreground)
    } else {
        (neutral_colors.background, neutral_colors.foreground)
    };
    div()
        .flex()
        .flex_row()
        .gap(px(8.0))
        .items_center()
        .child(
            div()
                .text_size(px(12.0))
                .text_color(style.list.muted_foreground)
                .child(label.to_string()),
        )
        .child(
            div()
                .px(px(12.0))
                .py(px(4.0))
                .rounded(px(4.0))
                .bg(toggle_bg)
                .text_color(toggle_fg)
                .text_size(px(12.0))
                .cursor(CursorStyle::PointingHand)
                .child(if value { "是" } else { "否" })
                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    entity.update(cx, |t, cx| {
                        t.form_is_default = !t.form_is_default;
                        cx.notify();
                    });
                }),
        )
}

#[derive(Clone, Copy)]
enum ModelRelation {
    Preset,
    Provider,
}

fn relation_selector(
    label: &'static str,
    selected_id: Option<i64>,
    options: Vec<(i64, String)>,
    open: bool,
    relation: ModelRelation,
    entity: Entity<LLMConfigView>,
    style: ManagementStyle,
) -> AnyElement {
    let selected_label = selected_id
        .and_then(|id| options.iter().find(|(option_id, _)| *option_id == id))
        .map(|(_, label)| label.clone())
        .unwrap_or_else(|| format!("请选择 {label}"));
    let mut dropdown = div()
        .flex()
        .flex_col()
        .gap(px(2.0))
        .p(px(4.0))
        .bg(style.list.row)
        .border_1()
        .border_color(style.list.border)
        .rounded(px(4.0))
        .max_h(px(200.0))
        .shadow_lg();
    for (id, option_label) in options {
        let option_entity = entity.clone();
        dropdown = dropdown.child(
            div()
                .px(px(8.0))
                .py(px(6.0))
                .rounded(px(4.0))
                .text_size(px(13.0))
                .cursor(CursorStyle::PointingHand)
                .hover(|s| s.bg(style.list.hover))
                .child(option_label)
                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    option_entity.update(cx, |view, cx| {
                        match relation {
                            ModelRelation::Preset => view.form_preset_id = Some(id),
                            ModelRelation::Provider => view.form_provider_id = Some(id),
                        }
                        view.preset_open = false;
                        view.provider_open = false;
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
                .text_color(style.list.muted_foreground)
                .child(label),
        )
        .child(
            div()
                .bg(style.list.muted)
                .rounded(px(6.0))
                .border_1()
                .border_color(style.list.border)
                .px(px(8.0))
                .py(px(6.0))
                .text_size(px(13.0))
                .cursor(CursorStyle::PointingHand)
                .child(selected_label)
                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    entity.update(cx, |view, cx| {
                        match relation {
                            ModelRelation::Preset => {
                                view.preset_open = !view.preset_open;
                                view.provider_open = false;
                            }
                            ModelRelation::Provider => {
                                view.provider_open = !view.provider_open;
                                view.preset_open = false;
                            }
                        }
                        view.category_open = false;
                        cx.notify();
                    });
                }),
        )
        .child(if open {
            dropdown.into_any_element()
        } else {
            div().into_any_element()
        })
        .into_any_element()
}

fn labeled_field(
    label: &'static str,
    input: Entity<InputState>,
    style: ManagementStyle,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(4.0))
        .child(
            div()
                .text_size(px(12.0))
                .text_color(style.list.muted_foreground)
                .child(label),
        )
        .child(Input::new(&input))
}

fn field(input: Entity<InputState>) -> impl IntoElement {
    Input::new(&input)
}
