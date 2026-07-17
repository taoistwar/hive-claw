use gpui::{
    App, Context, Entity, FocusHandle, Focusable, FontWeight, Hsla, MouseButton, ScrollHandle,
    SharedString, Window, div, prelude::*, px,
};
use gpui_component::ActiveTheme as _;
use gpui_component::input::{Input, InputState};

use crate::datasource::{DataSource, MysqlClient, Store};
use crate::ui::management_style::{
    ActionRole, ActionSize, ManagementStyle, action_button as management_action_button,
    management_modal_layer, management_modal_panel, management_modal_scroll,
};

#[derive(Clone)]
pub enum FormMode {
    Add,
    Edit(i64),
}

pub struct DataSourceForm {
    mode: FormMode,
    store: Entity<Store>,
    // Initial values used when the editable InputState entities are created.
    name: SharedString,
    host: SharedString,
    port: SharedString,
    username: SharedString,
    password: SharedString,
    placeholder_password: &'static str,
    name_input: Option<Entity<InputState>>,
    host_input: Option<Entity<InputState>>,
    port_input: Option<Entity<InputState>>,
    username_input: Option<Entity<InputState>>,
    password_input: Option<Entity<InputState>>,
    focus_handle: FocusHandle,
    form_scroll: ScrollHandle,
    status: FormStatus,
}

#[derive(Debug, Clone, PartialEq)]
enum FormStatus {
    Idle,
    Testing,
    TestingFailed(SharedString),
    TestingSuccess,
    Saving,
    Saved,
    Cancelled,
}

impl DataSourceForm {
    pub fn new(
        mode: FormMode,
        store: Entity<Store>,
        existing: Option<&DataSource>,
        cx: &mut Context<Self>,
    ) -> Self {
        let (name, host, port, username, password, placeholder_password) = match existing {
            Some(ds) => (
                ds.name.clone(),
                ds.host.clone(),
                ds.port.to_string(),
                ds.username.clone(),
                String::new(),
                "留空则不修改密码",
            ),
            None => (
                String::new(),
                "127.0.0.1".to_string(),
                "3306".to_string(),
                String::new(),
                String::new(),
                "密码",
            ),
        };
        Self {
            mode,
            store,
            name: SharedString::from(name),
            host: SharedString::from(host),
            port: SharedString::from(port),
            username: SharedString::from(username),
            password: SharedString::from(password),
            placeholder_password,
            name_input: None,
            host_input: None,
            port_input: None,
            username_input: None,
            password_input: None,
            focus_handle: cx.focus_handle(),
            form_scroll: ScrollHandle::default(),
            status: FormStatus::Idle,
        }
    }

    fn ensure_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.name_input.is_some() {
            return;
        }
        let name = self.name.clone();
        let host = self.host.clone();
        let port = self.port.clone();
        let username = self.username.clone();
        let password = self.password.clone();
        let pp = self.placeholder_password;
        self.name_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("数据源名称")
                .default_value(&name)
        }));
        self.host_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("主机地址")
                .default_value(&host)
        }));
        self.port_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("端口")
                .default_value(&port)
        }));
        self.username_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("用户名")
                .default_value(&username)
        }));
        self.password_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(pp)
                .default_value(&password)
        }));
    }

    fn test_connection(&mut self, cx: &mut Context<Self>) {
        let host = input_value(&self.host_input, &self.host, cx);
        let port_str = input_value(&self.port_input, &self.port, cx);
        let username = input_value(&self.username_input, &self.username, cx);
        let password = input_value(&self.password_input, &self.password, cx);
        let port: u16 = match port_str.parse() {
            Ok(p) => p,
            Err(_) => {
                self.status = FormStatus::TestingFailed(SharedString::from("端口必须是有效的数字"));
                cx.notify();
                return;
            }
        };
        self.status = FormStatus::Testing;
        cx.notify();
        let password_bytes = password.into_bytes();
        cx.spawn(async move |this, cx| {
            let result =
                MysqlClient::test_connection(&host, port, &username, &password_bytes).await;
            this.update(cx, |form, cx| {
                form.status = match result {
                    Ok(()) => FormStatus::TestingSuccess,
                    Err(e) => {
                        FormStatus::TestingFailed(SharedString::from(format!("连接失败: {}", e)))
                    }
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let name = input_value(&self.name_input, &self.name, cx);
        let host = input_value(&self.host_input, &self.host, cx);
        let port_str = input_value(&self.port_input, &self.port, cx);
        let username = input_value(&self.username_input, &self.username, cx);
        let password = input_value(&self.password_input, &self.password, cx);
        let password_missing = matches!(self.mode, FormMode::Add) && password.is_empty();
        if name.is_empty() || host.is_empty() || username.is_empty() || password_missing {
            self.status = FormStatus::TestingFailed(SharedString::from("请填写所有必填字段"));
            cx.notify();
            return;
        }
        let port: u16 = match port_str.parse() {
            Ok(p) => p,
            Err(_) => {
                self.status = FormStatus::TestingFailed(SharedString::from("端口必须是有效的数字"));
                cx.notify();
                return;
            }
        };
        self.status = FormStatus::Saving;
        cx.notify();
        let store = self.store.read(cx).clone();
        let mode = self.mode.clone();
        cx.spawn(async move |this, cx| {
            let result = match &mode {
                FormMode::Add => store
                    .create(&name, &host, port, &username, password.as_bytes())
                    .await
                    .map(|_| ()),
                FormMode::Edit(id) => {
                    if password.is_empty() {
                        store
                            .update(*id, &name, &host, port, &username, None)
                            .await
                            .map(|_| ())
                    } else {
                        store
                            .update(
                                *id,
                                &name,
                                &host,
                                port,
                                &username,
                                Some(password.as_bytes()),
                            )
                            .await
                            .map(|_| ())
                    }
                }
            };
            this.update(cx, |form, cx| {
                form.status = match result {
                    Ok(_) => FormStatus::Saved,
                    Err(e) => {
                        FormStatus::TestingFailed(SharedString::from(format!("保存失败: {}", e)))
                    }
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn is_done(&self) -> bool {
        matches!(self.status, FormStatus::Saved | FormStatus::Cancelled)
    }
}

impl Focusable for DataSourceForm {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for DataSourceForm {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_inputs(window, cx);
        let style = ManagementStyle::current(cx);
        let (
            overlay,
            popover,
            popover_foreground,
            border,
            muted,
            muted_foreground,
            danger,
            success,
        ) = {
            let theme = cx.theme();
            (
                theme.overlay,
                theme.popover,
                theme.popover_foreground,
                theme.border,
                theme.muted,
                theme.muted_foreground,
                theme.danger,
                theme.success,
            )
        };

        let name = self.name_input.clone().unwrap();
        let host = self.host_input.clone().unwrap();
        let port = self.port_input.clone().unwrap();
        let username = self.username_input.clone().unwrap();
        let password = self.password_input.clone().unwrap();

        let title = match &self.mode {
            FormMode::Add => "添加数据源",
            FormMode::Edit(_) => "编辑数据源",
        };
        let status_text: Option<(SharedString, Hsla)> = match &self.status {
            FormStatus::Idle => None,
            FormStatus::Testing => Some((SharedString::from("测试连接中..."), muted_foreground)),
            FormStatus::TestingFailed(msg) => Some((msg.clone(), danger)),
            FormStatus::TestingSuccess => Some((SharedString::from("连接成功！"), success)),
            FormStatus::Saving => Some((SharedString::from("保存中..."), muted_foreground)),
            FormStatus::Saved => Some((SharedString::from("保存成功！"), success)),
            FormStatus::Cancelled => None,
        };

        div()
            .id("form-overlay")
            .absolute()
            .top(px(0.0))
            .left(px(0.0))
            .right(px(0.0))
            .bottom(px(0.0))
            .bg(overlay)
            .flex()
            .items_center()
            .justify_center()
            .child(
                management_modal_panel(
                    management_modal_layer(px(480.0)),
                    popover,
                    popover_foreground,
                    border,
                )
                    .id("form-modal")
                    .child(
                            management_modal_scroll("datasource-form-scroll", &self.form_scroll)
                            .gap(px(16.0))
                            .child(
                                div()
                                    .text_size(px(18.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(title),
                            )
                            .child(form_field(
                                "名称",
                                name.clone(),
                                border,
                                muted,
                                muted_foreground,
                            ))
                            .child(form_row(
                                form_field("主机", host.clone(), border, muted, muted_foreground),
                                form_field("端口", port.clone(), border, muted, muted_foreground),
                            ))
                            .child(form_field(
                                "用户名",
                                username.clone(),
                                border,
                                muted,
                                muted_foreground,
                            ))
                            .child(form_field(
                                "密码",
                                password.clone(),
                                border,
                                muted,
                                muted_foreground,
                            ))
                            .child(
                                div()
                                    .flex()
                                    .justify_between()
                                    .items_center()
                                    .child(div().flex().gap(px(8.0)).child(form_action_button(
                                        "连接测试",
                                        ActionRole::Main,
                                        matches!(self.status, FormStatus::Testing),
                                        style,
                                        {
                                            let this = cx.weak_entity();
                                            move |_event, _window, cx| {
                                                this.update(cx, |form, cx| {
                                                    form.test_connection(cx)
                                                })
                                                .ok();
                                            }
                                        },
                                    )))
                                    .child(
                                        div()
                                            .flex()
                                            .gap(px(8.0))
                                            .child(form_action_button(
                                                "取消",
                                                ActionRole::Neutral,
                                                false,
                                                style,
                                                {
                                                    let this = cx.weak_entity();
                                                    move |_event, _window, cx| {
                                                        this.update(cx, |form, cx| {
                                                            form.status = FormStatus::Cancelled;
                                                            cx.notify();
                                                        })
                                                        .ok();
                                                    }
                                                },
                                            ))
                                            .child(form_action_button(
                                                "保存",
                                                ActionRole::Edit,
                                                matches!(self.status, FormStatus::Saving),
                                                style,
                                                {
                                                    let this = cx.weak_entity();
                                                    move |_event, _window, cx| {
                                                        this.update(cx, |form, cx| form.save(cx))
                                                            .ok();
                                                    }
                                                },
                                            )),
                                    ),
                            )
                            .child(if let Some((msg, color)) = status_text {
                                div()
                                    .text_size(px(13.0))
                                    .text_color(color)
                                    .child(msg)
                                    .into_any_element()
                            } else {
                                div().into_any_element()
                            }),
                    ),
            )
    }
}

fn input_value(
    input: &Option<Entity<InputState>>,
    fallback: &SharedString,
    cx: &Context<DataSourceForm>,
) -> String {
    input
        .as_ref()
        .map(|state| state.read(cx).value().to_string())
        .unwrap_or_else(|| fallback.to_string())
}

fn form_field(
    label: &'static str,
    input: Entity<InputState>,
    border: Hsla,
    background: Hsla,
    label_color: Hsla,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(4.0))
        .child(
            div()
                .text_size(px(12.0))
                .text_color(label_color)
                .child(label),
        )
        .child(
            div()
                .id(SharedString::from(format!("field-{}", label)))
                .h(px(32.0))
                .border_1()
                .border_color(border)
                .rounded(px(4.0))
                .bg(background)
                .child(Input::new(&input).w_full().h_full().px(px(8.0))),
        )
}
fn form_row(left: impl IntoElement, right: impl IntoElement) -> impl IntoElement {
    div()
        .flex()
        .gap(px(12.0))
        .child(div().flex_1().child(left))
        .child(div().w(px(120.0)).child(right))
}
fn form_action_button(
    label: &'static str,
    role: ActionRole,
    disabled: bool,
    style: ManagementStyle,
    on_click: impl Fn(&gpui::MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    management_action_button(
        SharedString::from(label),
        label,
        if disabled { ActionRole::Disabled } else { role },
        ActionSize::Dialog,
        style,
    )
    .when(!disabled, |button| {
        button.on_mouse_down(MouseButton::Left, on_click)
    })
}
