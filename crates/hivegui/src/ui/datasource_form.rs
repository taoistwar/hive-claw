use gpui::{
    App, Context, CursorStyle, Entity, FocusHandle, Focusable, FontWeight, MouseButton,
    SharedString, Window, div, prelude::*, px, rgb, rgba,
};
use gpui_component::input::InputState;

use crate::datasource::{DataSource, MysqlClient, Store};

#[derive(Clone)]
pub enum FormMode {
    Add,
    Edit(i64),
}

pub struct DataSourceForm {
    mode: FormMode,
    store: Entity<Store>,
    // Cached content synced to InputState in render
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
        let host = self.host.to_string();
        let port_str = self.port.to_string();
        let username = self.username.to_string();
        let password = self.password.to_string();
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
        let name = self.name.to_string();
        let host = self.host.to_string();
        let port_str = self.port.to_string();
        let username = self.username.to_string();
        let password = self.password.to_string();
        if name.is_empty() || host.is_empty() || username.is_empty() || password.is_empty() {
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

        // Sync content from InputState back to strings
        let sync = |opt: &Option<Entity<InputState>>,
                    target: &mut SharedString,
                    cx: &mut Context<Self>| {
            if let Some(s) = opt {
                *target = SharedString::from(s.read(cx).value());
            }
        };
        sync(&self.name_input, &mut self.name, cx);
        sync(&self.host_input, &mut self.host, cx);
        sync(&self.port_input, &mut self.port, cx);
        sync(&self.username_input, &mut self.username, cx);
        sync(&self.password_input, &mut self.password, cx);

        let name = self.name_input.clone().unwrap();
        let host = self.host_input.clone().unwrap();
        let port = self.port_input.clone().unwrap();
        let username = self.username_input.clone().unwrap();
        let password = self.password_input.clone().unwrap();

        let title = match &self.mode {
            FormMode::Add => "添加数据源",
            FormMode::Edit(_) => "编辑数据源",
        };
        let status_text: Option<(SharedString, gpui::Rgba)> = match &self.status {
            FormStatus::Idle => None,
            FormStatus::Testing => Some((SharedString::from("测试连接中..."), rgba(0x888888ff))),
            FormStatus::TestingFailed(msg) => Some((msg.clone(), rgba(0xcc0000ff))),
            FormStatus::TestingSuccess => {
                Some((SharedString::from("连接成功！"), rgba(0x008800ff)))
            }
            FormStatus::Saving => Some((SharedString::from("保存中..."), rgba(0x888888ff))),
            FormStatus::Saved => Some((SharedString::from("保存成功！"), rgba(0x008800ff))),
            FormStatus::Cancelled => None,
        };

        div()
            .id("form-overlay")
            .absolute()
            .top(px(0.0))
            .left(px(0.0))
            .right(px(0.0))
            .bottom(px(0.0))
            .bg(rgba(0x00000080))
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .id("form-modal")
                    .w(px(480.0))
                    .bg(rgb(0xffffff))
                    .rounded(px(8.0))
                    .shadow_lg()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .p(px(24.0))
                            .gap(px(16.0))
                            .child(
                                div()
                                    .text_size(px(18.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(title),
                            )
                            .child(form_field("名称", name.clone()))
                            .child(form_row(
                                form_field("主机", host.clone()),
                                form_field("端口", port.clone()),
                            ))
                            .child(form_field("用户名", username.clone()))
                            .child(form_field("密码", password.clone()))
                            .child(
                                div()
                                    .flex()
                                    .justify_between()
                                    .items_center()
                                    .child(div().flex().gap(px(8.0)).child(action_button(
                                        "连接测试",
                                        rgba(0x4a90d9ff),
                                        matches!(self.status, FormStatus::Testing),
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
                                            .child(action_button(
                                                "取消",
                                                rgba(0xccccccff),
                                                false,
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
                                            .child(action_button(
                                                "保存",
                                                rgba(0x2d8a4eff),
                                                matches!(self.status, FormStatus::Saving),
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

fn form_field(label: &'static str, input: Entity<InputState>) -> impl IntoElement {
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
        .child(
            div()
                .id(SharedString::from(format!("field-{}", label)))
                .h(px(32.0))
                .border_1()
                .border_color(rgb(0xd0d0d0))
                .rounded(px(4.0))
                .px(px(8.0))
                .bg(rgb(0xfafafa))
                .child(input),
        )
}
fn form_row(left: impl IntoElement, right: impl IntoElement) -> impl IntoElement {
    div()
        .flex()
        .gap(px(12.0))
        .child(div().child(left))
        .child(div().w(px(120.0)).child(right))
}
fn action_button(
    label: &'static str,
    bg: gpui::Rgba,
    disabled: bool,
    on_click: impl Fn(&gpui::MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let opacity = if disabled { 0.5 } else { 1.0 };
    div()
        .id(SharedString::from(label))
        .px(px(16.0))
        .py(px(8.0))
        .rounded(px(4.0))
        .bg(bg)
        .text_size(px(13.0))
        .text_color(rgb(0xffffff))
        .cursor(CursorStyle::PointingHand)
        .opacity(opacity)
        .on_mouse_down(MouseButton::Left, move |event, window, cx| {
            if !disabled {
                on_click(event, window, cx);
            }
        })
        .child(label)
}
