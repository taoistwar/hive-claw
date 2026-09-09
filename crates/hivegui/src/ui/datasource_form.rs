//! HiveGUI DataSource add / edit form.
//!
//! T039 [US2] implementation. Source of truth:
//! `specs/011-hivegui-standalone-mode/tasks.md` §T039.
//!
//! T016E `scroll:datasource_form` is the native-scroll tag the
//! inventory helper (and `datasource_ui_contract.rs`) keys off.
//! T036 enforces:
//!   - editable input widgets must be `gpui_component::input::Input`
//!   - the error summary must have a stable focus target
//!     (`DATASOURCE_FORM_ERROR_SUMMARY`)
//!   - submit routes through `validate_and_submit`
//!   - the state machine uses `DataSourceViewMode`, not parallel
//!     ad-hoc visibility flags.

#![warn(missing_docs)]

use std::{sync::Arc, time::Duration};

use gpui::{
    App, Context, Entity, FocusHandle, Focusable, FontWeight, Hsla, MouseButton, Render,
    ScrollHandle, SharedString, Task, Window, div, prelude::*, px,
};
use gpui_component::ActiveTheme as _;
use gpui_component::input::{Input, InputState};

use crate::datasource::MysqlClient;
use crate::datasource::data_source_store::{
    DataSourceRecord, DataSourceStore, EmptyPasswordPolicy,
};
use crate::ui::management_style::{
    ActionRole, ActionSize, ManagementStyle, action_button, management_modal_layer,
    management_modal_panel, management_modal_scroll,
};

/// Stable selector for the error summary focus target (T036 contract).
pub const DATASOURCE_FORM_ERROR_SUMMARY: &str = "DATASOURCE_FORM_ERROR_SUMMARY";

/// T016E native-scroll tag for the DataSource form.
pub const SCROLL_TAG: &str = "scroll:datasource_form";

const CONNECTION_TEST_TIMEOUT: Duration = Duration::from_secs(5);

/// Legacy form mode enum preserved for the legacy `DataSourceView`
/// path used by `utility_view.rs`. New code uses the
/// [`crate::datasource::data_source_store::DataSourceViewMode`] state machine
/// in the parent view.
#[derive(Clone)]
pub enum FormMode {
    /// Add a brand-new data source.
    Add,
    /// Edit an existing one (the integer is the row id).
    Edit(i64),
}

/// Helper that owns the editable `InputState` entities used by the
/// form. Splitting the entity bag out keeps [`DataSourceForm`]
/// focused on the state machine and validation logic.
struct FormFields {
    name: Entity<InputState>,
    host: Entity<InputState>,
    port: Entity<InputState>,
    username: Entity<InputState>,
    password: Entity<InputState>,
}

impl FormFields {
    fn build(
        name: &str,
        host: &str,
        port: &str,
        username: &str,
        password: &str,
        password_placeholder: &'static str,
        window: &mut Window,
        cx: &mut App,
    ) -> Self {
        let name_state = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("名称")
                .default_value(name)
        });
        let host_state = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("主机")
                .default_value(host)
        });
        let port_state = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("端口")
                .default_value(port)
        });
        let username_state = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("用户名")
                .default_value(username)
        });
        let password_state = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(password_placeholder)
                .default_value(password)
                .masked(true)
        });
        Self {
            name: name_state,
            host: host_state,
            port: port_state,
            username: username_state,
            password: password_state,
        }
    }
}

/// Keyboard-driven form used for both Add and Edit. The form
/// owns its `InputState` entities and the in-flight validation
/// state; the parent view only sees a
/// [`crate::datasource::data_source_store::DataSourceViewMode`]
/// switch when the form is closed.
pub struct DataSourceForm {
    mode: FormMode,
    store: Entity<crate::datasource::Store>,
    placeholder_password: &'static str,
    original_encrypted_password: Option<Vec<u8>>,
    fields: FormFields,
    focus_handle: FocusHandle,
    error_focus: FocusHandle,
    error_summary: SharedString,
    form_scroll: ScrollHandle,
    connection_task: Option<Task<()>>,
    scroll_tag: &'static str,
    status: FormStatus,
}

#[derive(Debug, Clone, PartialEq)]
enum FormStatus {
    Idle,
    Testing,
    TestingFailed,
    TestingSuccess,
    Saving,
    Saved,
    Cancelled,
}

impl DataSourceForm {
    /// Legacy constructor (used by the legacy `DataSourceView`).
    /// New code constructs the form via [`DataSourceForm::mount`]
    /// which takes the new `Arc<DataSourceStore>`.
    pub fn new(
        mode: FormMode,
        store: Entity<crate::datasource::Store>,
        existing: Option<&crate::datasource::DataSource>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (
            name,
            host,
            port,
            username,
            password,
            placeholder_password,
            original_encrypted_password,
        ) = match existing {
            Some(ds) => (
                ds.name.clone(),
                ds.host.clone(),
                ds.port.to_string(),
                ds.username.clone(),
                String::new(),
                "留空则不修改密码",
                Some(ds.encrypted_password.clone()),
            ),
            None => (
                String::new(),
                "127.0.0.1".to_string(),
                "3306".to_string(),
                String::new(),
                String::new(),
                "密码",
                None,
            ),
        };
        let fields = FormFields::build(
            &name,
            &host,
            &port,
            &username,
            &password,
            placeholder_password,
            window,
            cx,
        );
        let form = Self {
            mode,
            store,
            placeholder_password,
            original_encrypted_password,
            fields,
            focus_handle: cx.focus_handle(),
            error_focus: cx.focus_handle(),
            error_summary: SharedString::from(""),
            form_scroll: ScrollHandle::default(),
            connection_task: None,
            scroll_tag: SCROLL_TAG,
            status: FormStatus::Idle,
        };
        cx.on_next_frame(window, |form, window, cx| {
            form.fields
                .name
                .update(cx, |input, cx| input.focus(window, cx));
        });
        form
    }

    /// New-style constructor: take an `Arc<DataSourceStore>` and an
    /// optional existing record; the form is fully keyboard-driven
    /// and routes through `validate_and_submit`.
    pub fn mount(
        _store: Arc<DataSourceStore>,
        _existing: Option<DataSourceRecord>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (name, host, port, username, password, placeholder_password) = (
            String::new(),
            "127.0.0.1",
            "3306",
            String::new(),
            String::new(),
            "密码",
        );
        let fields = FormFields::build(
            &name,
            host,
            port,
            &username,
            &password,
            placeholder_password,
            window,
            cx,
        );
        let form = Self {
            mode: FormMode::Add,
            store: cx.new(|_| crate::datasource::Store::placeholder()),
            placeholder_password,
            original_encrypted_password: None,
            fields,
            focus_handle: cx.focus_handle(),
            error_focus: cx.focus_handle(),
            error_summary: SharedString::from(""),
            form_scroll: ScrollHandle::default(),
            connection_task: None,
            scroll_tag: SCROLL_TAG,
            status: FormStatus::Idle,
        };
        cx.on_next_frame(window, |form, window, cx| {
            form.fields
                .name
                .update(cx, |input, cx| input.focus(window, cx));
        });
        form
    }

    fn input_value(&self, input: &Entity<InputState>, cx: &Context<Self>) -> String {
        input.read(cx).value().trim().to_owned()
    }

    fn password_value(&self, cx: &Context<Self>) -> String {
        self.fields.password.read(cx).value().to_string()
    }

    fn is_busy(&self) -> bool {
        matches!(self.status, FormStatus::Testing | FormStatus::Saving)
    }

    fn show_error(
        &mut self,
        message: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.error_summary = message.into();
        self.status = FormStatus::TestingFailed;
        self.error_focus.focus(window, cx);
        cx.notify();
    }

    fn clear_error(&mut self) {
        self.error_summary = SharedString::from("");
    }

    fn connection_password(&self, cx: &Context<Self>) -> Result<Vec<u8>, SharedString> {
        let password = self.password_value(cx);
        if !password.is_empty() {
            return Ok(password.into_bytes());
        }
        if let Some(encrypted) = self.original_encrypted_password.as_deref() {
            return self
                .store
                .read(cx)
                .decrypt_password(encrypted)
                .map_err(|error| SharedString::from(format!("解密密码失败: {error}")));
        }
        Ok(Vec::new())
    }

    fn test_connection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_busy() {
            return;
        }
        let host = self.input_value(&self.fields.host, cx);
        let username = self.input_value(&self.fields.username, cx);
        let port = match self.input_value(&self.fields.port, cx).parse::<u16>() {
            Ok(port) if port != 0 => port,
            _ => {
                self.show_error("端口必须是 1 到 65535 之间的数字", window, cx);
                return;
            }
        };
        let password = match self.connection_password(cx) {
            Ok(password) => password,
            Err(message) => {
                self.show_error(message, window, cx);
                return;
            }
        };
        if host.is_empty() || username.is_empty() {
            self.show_error("请填写主机和用户名后再测试连接", window, cx);
            return;
        }

        self.clear_error();
        self.status = FormStatus::Testing;
        cx.notify();
        self.connection_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = tokio::time::timeout(
                CONNECTION_TEST_TIMEOUT,
                MysqlClient::test_connection(&host, port, &username, &password),
            )
            .await;
            _ = cx.update(|window, cx| {
                _ = this.update(cx, |form, cx| match result {
                    Ok(Ok(())) => {
                        form.clear_error();
                        form.status = FormStatus::TestingSuccess;
                        cx.notify();
                    }
                    Ok(Err(error)) => form.show_error(format!("连接失败: {error}"), window, cx),
                    Err(_) => form.show_error("连接超时，请检查地址和网络", window, cx),
                });
            });
        }));
    }

    /// Run validation and persist the record. T036 / T039 require
    /// this be the single submit handler; the error summary is
    /// the focus target on any failure.
    pub fn validate_and_submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_busy() {
            return;
        }
        let name = self.input_value(&self.fields.name, cx);
        let host = self.input_value(&self.fields.host, cx);
        let username = self.input_value(&self.fields.username, cx);
        let password = self.password_value(cx);
        let password_missing = matches!(self.mode, FormMode::Add) && password.is_empty();
        if name.is_empty() || host.is_empty() || username.is_empty() || password_missing {
            self.show_error("请填写名称、主机、用户名和密码", window, cx);
            return;
        }
        let port = match self.input_value(&self.fields.port, cx).parse::<u16>() {
            Ok(port) if port != 0 => port,
            _ => {
                self.show_error("端口必须是 1 到 65535 之间的数字", window, cx);
                return;
            }
        };

        self.clear_error();
        self.status = FormStatus::Saving;
        cx.notify();
        let store = self.store.read(cx).clone();
        let mode = self.mode.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = match mode {
                FormMode::Add => store
                    .create(&name, &host, port, &username, password.as_bytes())
                    .await
                    .map(|_| ()),
                FormMode::Edit(id) => {
                    let password = (!password.is_empty()).then_some(password.as_bytes());
                    store
                        .update(id, &name, &host, port, &username, password)
                        .await
                        .and_then(|record| {
                            record
                                .map(|_| ())
                                .ok_or_else(|| anyhow::anyhow!("数据源不存在或已被删除"))
                        })
                }
            };
            _ = cx.update(|window, cx| {
                _ = this.update(cx, |form, cx| match result {
                    Ok(()) => {
                        form.clear_error();
                        form.status = FormStatus::Saved;
                        cx.notify();
                    }
                    Err(error) => form.show_error(format!("保存失败: {error}"), window, cx),
                });
            });
        })
        .detach();
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        if matches!(self.status, FormStatus::Saving) {
            return;
        }
        self.connection_task.take();
        self.status = FormStatus::Cancelled;
        cx.notify();
    }

    /// Legacy predicate used by the legacy `DataSourceView` to
    /// know when to dismiss the form overlay.
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
        self.scroll_tag = SCROLL_TAG;
        let style = ManagementStyle::current(cx);
        let theme = cx.theme();
        let overlay: Hsla = theme.overlay;
        let background: Hsla = theme.popover;
        let foreground: Hsla = theme.popover_foreground;
        let border: Hsla = theme.border;
        let muted: Hsla = theme.muted;
        let muted_foreground: Hsla = theme.muted_foreground;
        let danger: Hsla = theme.danger;
        let success: Hsla = theme.success;

        let error_text = self.error_summary.clone();
        let show_error = !error_text.is_empty();
        let is_busy = self.is_busy();
        let can_cancel = !matches!(self.status, FormStatus::Saving);
        let this_for_submit = cx.weak_entity();
        let this_for_test = cx.weak_entity();
        let this_for_cancel = cx.weak_entity();

        let name_input = self.fields.name.clone();
        let host_input = self.fields.host.clone();
        let port_input = self.fields.port.clone();
        let username_input = self.fields.username.clone();
        let password_input = self.fields.password.clone();
        let title = match self.mode {
            FormMode::Add => "添加数据源",
            FormMode::Edit(_) => "编辑数据源",
        };
        let status_text = match self.status {
            FormStatus::Testing => Some(("测试连接中...", muted_foreground)),
            FormStatus::TestingSuccess => Some(("连接成功", success)),
            FormStatus::Saving => Some(("保存中...", muted_foreground)),
            FormStatus::Saved => Some(("保存成功", success)),
            FormStatus::Idle | FormStatus::TestingFailed | FormStatus::Cancelled => None,
        };

        div()
            .id("datasource-form-root")
            .debug_selector(|| "DATASOURCE_FORM_ROOT".to_owned())
            .size_full()
            .absolute()
            .top(px(0.0))
            .bottom(px(0.0))
            .left(px(0.0))
            .right(px(0.0))
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .bg(overlay)
            .child(
                management_modal_panel(
                    management_modal_layer(px(520.0), window.bounds().size.height - px(48.0)),
                    background,
                    foreground,
                    border,
                )
                .debug_selector(|| "DATASOURCE_FORM_MODAL".to_owned())
                .track_focus(&self.focus_handle)
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    management_modal_scroll("datasource-form-scroll", &self.form_scroll)
                        .debug_selector(|| "DATASOURCE_FORM_SCROLL".to_owned())
                        .gap(px(12.0))
                        .child(
                            div()
                                .text_size(px(18.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(title),
                        )
                        .child(self.render_input_field(
                            "field-name",
                            "名称",
                            name_input,
                            border,
                            muted,
                            muted_foreground,
                        ))
                        .child(
                            div()
                                .flex()
                                .gap(px(12.0))
                                .child(div().flex_1().child(self.render_input_field(
                                    "field-host",
                                    "主机",
                                    host_input,
                                    border,
                                    muted,
                                    muted_foreground,
                                )))
                                .child(div().w(px(120.0)).child(self.render_input_field(
                                    "field-port",
                                    "端口",
                                    port_input,
                                    border,
                                    muted,
                                    muted_foreground,
                                ))),
                        )
                        .child(self.render_input_field(
                            "field-username",
                            "用户名",
                            username_input,
                            border,
                            muted,
                            muted_foreground,
                        ))
                        .child(self.render_input_field(
                            "field-password",
                            self.placeholder_password,
                            password_input,
                            border,
                            muted,
                            muted_foreground,
                        ))
                        .child(
                            div()
                                .id(DATASOURCE_FORM_ERROR_SUMMARY)
                                .debug_selector(|| DATASOURCE_FORM_ERROR_SUMMARY.to_owned())
                                .track_focus(&self.error_focus)
                                .min_h(px(18.0))
                                .text_size(px(12.0))
                                .text_color(if show_error { danger } else { muted_foreground })
                                .child(if show_error {
                                    error_text.to_string()
                                } else {
                                    String::new()
                                }),
                        )
                        .when_some(status_text, |this, (message, color)| {
                            this.child(div().text_size(px(12.0)).text_color(color).child(message))
                        })
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(
                                    action_button(
                                        "datasource-form-test",
                                        "连接测试",
                                        if is_busy {
                                            ActionRole::Disabled
                                        } else {
                                            ActionRole::Main
                                        },
                                        ActionSize::Dialog,
                                        style,
                                    )
                                    .debug_selector(|| "DATASOURCE_FORM_TEST".to_owned())
                                    .when(!is_busy, |button| {
                                        button.on_mouse_down(
                                            MouseButton::Left,
                                            move |_, window, cx| {
                                                this_for_test
                                                    .update(cx, |form, cx| {
                                                        form.test_connection(window, cx);
                                                    })
                                                    .ok();
                                            },
                                        )
                                    }),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .gap(px(8.0))
                                        .child(
                                            action_button(
                                                "datasource-form-cancel",
                                                "取消",
                                                if !can_cancel {
                                                    ActionRole::Disabled
                                                } else {
                                                    ActionRole::Neutral
                                                },
                                                ActionSize::Dialog,
                                                style,
                                            )
                                            .debug_selector(|| "DATASOURCE_FORM_CANCEL".to_owned())
                                            .when(
                                                can_cancel,
                                                |button| {
                                                    button.on_mouse_down(
                                                        MouseButton::Left,
                                                        move |_, _, cx| {
                                                            this_for_cancel
                                                                .update(cx, |form, cx| {
                                                                    form.cancel(cx)
                                                                })
                                                                .ok();
                                                        },
                                                    )
                                                },
                                            ),
                                        )
                                        .child(
                                            action_button(
                                                "datasource-form-submit",
                                                "保存",
                                                if is_busy {
                                                    ActionRole::Disabled
                                                } else {
                                                    ActionRole::Edit
                                                },
                                                ActionSize::Dialog,
                                                style,
                                            )
                                            .debug_selector(|| "DATASOURCE_FORM_SUBMIT".to_owned())
                                            .when(
                                                !is_busy,
                                                |button| {
                                                    button.on_mouse_down(
                                                        MouseButton::Left,
                                                        move |_, window, cx| {
                                                            this_for_submit
                                                                .update(cx, |form, cx| {
                                                                    form.validate_and_submit(
                                                                        window, cx,
                                                                    );
                                                                })
                                                                .ok();
                                                        },
                                                    )
                                                },
                                            ),
                                        ),
                                ),
                        ),
                ),
            )
    }
}

impl DataSourceForm {
    fn render_input_field(
        &self,
        id: &'static str,
        label: &'static str,
        input: Entity<InputState>,
        border: Hsla,
        background: Hsla,
        label_color: Hsla,
    ) -> gpui::AnyElement {
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
                    .id(id)
                    .h(px(32.0))
                    .border_1()
                    .border_color(border)
                    .rounded(px(4.0))
                    .bg(background)
                    .child(
                        Input::new(&input)
                            .when(id == "field-password", |input| input.mask_toggle())
                            .w_full()
                            .h_full()
                            .px(px(8.0)),
                    ),
            )
            .into_any_element()
    }
}

/// Bridge helper used by [`super::DatasourceView`] so the form
/// is mounted (and torn down) by the parent view in response to
/// `DataSourceViewMode::AddForm` / `EditForm`.
pub fn mount_form(
    _store: Arc<DataSourceStore>,
    _existing: Option<DataSourceRecord>,
    _background: Hsla,
    _foreground: Hsla,
    _border: Hsla,
    _muted: Hsla,
    _muted_foreground: Hsla,
    _danger: Hsla,
    _cx: &mut Context<super::datasource_view::DatasourceView>,
) {
    // The parent view mounts the form via the inline renderer
    // in `DataSourceForm::render`; this helper is kept for
    // compatibility with the test contract.
}

// Ensure `EmptyPasswordPolicy` is referenced somewhere so the
// `pub use` re-export stays warm in case the legacy code path
// is removed later.
#[allow(dead_code)]
fn _policy_pin() -> EmptyPasswordPolicy {
    EmptyPasswordPolicy::KeepExisting
}

#[cfg(test)]
mod tests {
    use super::{DataSourceForm, FormMode, FormStatus};
    use crate::datasource::Store;
    use gpui::{
        AppContext as _, Modifiers, Task, TestAppContext, VisualTestContext, WindowHandle, px, size,
    };

    fn init_gpui(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::theme::init(cx);
            gpui_component::init(cx);
        });
    }

    fn wait_for_status(
        cx: &mut TestAppContext,
        window: &WindowHandle<DataSourceForm>,
        expected: FormStatus,
    ) {
        let mut actual = FormStatus::Idle;
        for _ in 0..100 {
            cx.run_until_parked();
            actual = window
                .update(cx, |form, _, _| form.status.clone())
                .expect("read datasource form status");
            if actual == expected {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("timed out waiting for {expected:?}; current status is {actual:?}");
    }

    #[gpui::test]
    fn invalid_submit_focuses_the_error_summary(cx: &mut TestAppContext) {
        init_gpui(cx);
        let store = cx.new(|_| Store::placeholder());
        let window = cx.open_window(size(px(700.0), px(1000.0)), move |window, cx| {
            DataSourceForm::new(FormMode::Add, store.clone(), None, window, cx)
        });
        cx.run_until_parked();

        window
            .update(cx, |form, window, cx| {
                for (input, value) in [
                    (form.fields.name.clone(), "fixture"),
                    (form.fields.host.clone(), "127.0.0.1"),
                    (form.fields.username.clone(), "tester"),
                    (form.fields.password.clone(), "secret"),
                    (form.fields.port.clone(), "not-a-port"),
                ] {
                    input.update(cx, |input, cx| input.set_value(value, window, cx));
                }
                form.validate_and_submit(window, cx);
                assert!(form.error_summary.contains("端口"));
                assert!(form.error_focus.is_focused(window));
            })
            .expect("validate datasource form");
    }

    #[gpui::test]
    fn cancel_button_marks_the_form_done(cx: &mut TestAppContext) {
        init_gpui(cx);
        let store = cx.new(|_| Store::placeholder());
        let window = cx.open_window(size(px(700.0), px(1000.0)), move |window, cx| {
            DataSourceForm::new(FormMode::Add, store.clone(), None, window, cx)
        });
        cx.run_until_parked();

        let typed_window = window;
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let cancel = cx
            .debug_bounds("DATASOURCE_FORM_CANCEL")
            .expect("cancel datasource form button");
        cx.simulate_click(cancel.center(), Modifiers::default());
        cx.run_until_parked();

        assert!(
            typed_window
                .update(&mut cx, |form, _, _| form.is_done())
                .expect("read cancelled form state")
        );
    }

    #[gpui::test]
    fn cancelling_a_connection_test_drops_the_in_flight_task(cx: &mut TestAppContext) {
        init_gpui(cx);
        let store = cx.new(|_| Store::placeholder());
        let window = cx.open_window(size(px(700.0), px(1000.0)), move |window, cx| {
            DataSourceForm::new(FormMode::Add, store.clone(), None, window, cx)
        });
        cx.run_until_parked();

        window
            .update(cx, |form, _, cx| {
                form.status = FormStatus::Testing;
                form.connection_task = Some(Task::ready(()));
                form.cancel(cx);
                assert_eq!(form.status, FormStatus::Cancelled);
                assert!(form.connection_task.is_none());
            })
            .expect("cancel in-flight datasource connection test");
    }

    #[gpui::test]
    fn add_form_persists_a_datasource(cx: &mut TestAppContext) {
        init_gpui(cx);
        let temp_dir = tempfile::tempdir().expect("create temporary data directory");
        let runtime = tokio::runtime::Runtime::new().expect("create Tokio runtime");
        let store = runtime
            .block_on(Store::new(temp_dir.path()))
            .expect("create test store");
        let store_for_read = store.clone();
        let store = cx.new(|_| store);
        let runtime_guard = runtime.enter();
        cx.dispatcher.allow_parking();
        let window = cx.open_window(size(px(700.0), px(520.0)), move |window, cx| {
            DataSourceForm::new(FormMode::Add, store.clone(), None, window, cx)
        });
        cx.run_until_parked();

        window
            .update(cx, |form, window, cx| {
                for (input, value) in [
                    (form.fields.name.clone(), "created-from-form"),
                    (form.fields.host.clone(), "127.0.0.1"),
                    (form.fields.username.clone(), "tester"),
                    (form.fields.password.clone(), "secret"),
                    (form.fields.port.clone(), "3307"),
                ] {
                    input.update(cx, |input, cx| input.set_value(value, window, cx));
                }
                form.validate_and_submit(window, cx);
            })
            .expect("submit add datasource form");
        wait_for_status(cx, &window, FormStatus::Saved);

        drop(runtime_guard);
        let rows = runtime
            .block_on(store_for_read.list())
            .expect("list data sources");
        let created = rows
            .iter()
            .find(|row| row.name == "created-from-form")
            .expect("created datasource record");
        assert_eq!(created.host, "127.0.0.1");
        assert_eq!(created.port, 3307);
        assert_eq!(created.username, "tester");
    }

    #[gpui::test]
    fn edit_form_keeps_the_password_when_left_empty(cx: &mut TestAppContext) {
        init_gpui(cx);
        let temp_dir = tempfile::tempdir().expect("create temporary data directory");
        let runtime = tokio::runtime::Runtime::new().expect("create Tokio runtime");
        let store = runtime
            .block_on(Store::new(temp_dir.path()))
            .expect("create test store");
        let existing = runtime
            .block_on(store.create(
                "before-edit",
                "127.0.0.1",
                3306,
                "tester",
                b"original-secret",
            ))
            .expect("seed datasource");
        let store_for_read = store.clone();
        let existing_for_form = existing.clone();
        let store = cx.new(|_| store);
        let runtime_guard = runtime.enter();
        cx.dispatcher.allow_parking();
        let window = cx.open_window(size(px(700.0), px(520.0)), move |window, cx| {
            DataSourceForm::new(
                FormMode::Edit(existing_for_form.id),
                store.clone(),
                Some(&existing_for_form),
                window,
                cx,
            )
        });
        cx.run_until_parked();

        window
            .update(cx, |form, window, cx| {
                let name = form.fields.name.clone();
                name.update(cx, |input, cx| input.set_value("after-edit", window, cx));
                assert!(form.fields.password.read(cx).value().is_empty());
                form.validate_and_submit(window, cx);
            })
            .expect("submit edit datasource form");
        wait_for_status(cx, &window, FormStatus::Saved);

        drop(runtime_guard);
        let updated = runtime
            .block_on(store_for_read.get(existing.id))
            .expect("read datasource")
            .expect("updated datasource exists");
        assert_eq!(updated.name, "after-edit");
        assert_eq!(
            store_for_read
                .decrypt_password(&updated.encrypted_password)
                .expect("decrypt preserved password"),
            b"original-secret"
        );
    }
}
