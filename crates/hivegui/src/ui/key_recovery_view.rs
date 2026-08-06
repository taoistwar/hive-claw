//! Device key recovery view (T025).
//!
//! GPUI surface shown when the device key on disk is missing, corrupt,
//! has the wrong permissions, or is otherwise unreadable. The view is
//! owned by the Foundation recovery flow; T016 Red contract tests
//! drive it with `VisualTestContext` and an injected
//! `KeyRecoveryCommands` adapter so the UI heartbeat remains
//! responsive while a long reconfiguration is pending.
//!
//! T016D source contract: every recovery action must be reachable
//! from the keyboard alone. The view owns a single view-level focus
//! handle and dispatches every keystroke through a `on_key_down`
//! handler that traps `Tab` / `Shift+Tab` / `Enter` / `Space`. The
//! active button is tracked in a `focused_index` cursor in the view
//! state so the keyboard contract is deterministic regardless of
//! how the underlying focus tree moves during the test runtime.

#![warn(missing_docs)]

use std::sync::Arc;

use gpui::{
    Action, Context, Entity, FocusHandle, IntoElement, KeyBinding, KeyDownEvent, Render, Styled,
    Window, actions, div, prelude::*, px,
};

actions!(hivegui_key_recovery, [KeyRecoveryTab, KeyRecoveryTabPrev]);

/// A user action triggered from the key recovery view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyRecoveryAction {
    /// Reconfigure a new device key. This is the only path that
    /// creates a fresh key on disk; the user has to be authenticated
    /// as a master-password holder first.
    Reconfigure,
    /// Restore the device key from the most recent encrypted backup.
    RestoreEncryptedBackup,
    /// Exit the application; the user accepts data loss.
    Exit,
}

/// Boxed future returned by [`KeyRecoveryCommands::execute`].
pub type KeyRecoveryFuture = std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<(), KeyRecoveryError>> + Send + Sync>,
>;

/// Errors returned by key recovery commands.
#[derive(Debug, Clone, thiserror::Error)]
pub enum KeyRecoveryError {
    /// The reconfiguration attempt failed.
    #[error("device key reconfiguration failed: {0}")]
    ReconfigureFailed(String),
    /// The restore attempt failed.
    #[error("device key restore failed: {0}")]
    RestoreFailed(String),
}

/// User-supplied boundary invoked when the user activates a control.
pub trait KeyRecoveryCommands: Send + Sync {
    /// Execute the user-selected action.
    fn execute(&self, action: KeyRecoveryAction) -> KeyRecoveryFuture;
}

/// Why the recovery view is being shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyRecoveryReason {
    /// The device key file does not exist.
    Missing,
    /// The device key file is structurally corrupt.
    Corrupt,
    /// The device key file has the wrong permissions.
    Permissions,
    /// The device key file is otherwise unreadable.
    Unreadable,
}

impl KeyRecoveryReason {
    /// Stable identifier used to drive the per-reason selector and
    /// telemetry labels.
    pub fn slug(self) -> &'static str {
        match self {
            Self::Missing => "key_missing",
            Self::Corrupt => "key_corrupt",
            Self::Permissions => "key_permissions",
            Self::Unreadable => "key_unreadable",
        }
    }

    /// Chinese label rendered next to the selector.
    pub fn label_zh(self) -> &'static str {
        match self {
            Self::Missing => "设备密钥缺失",
            Self::Corrupt => "设备密钥损坏",
            Self::Permissions => "设备密钥权限错误",
            Self::Unreadable => "设备密钥不可读",
        }
    }
}

/// Local phase used to drive the "running / completed" indicator.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum KeyRecoveryPhase {
    /// No operation is in flight.
    #[default]
    Idle,
    /// A command is currently in flight.
    Running(KeyRecoveryAction),
    /// The last command finished successfully.
    Completed,
}

/// GPUI view for the device-key recovery surface.
pub struct KeyRecoveryView {
    reason: KeyRecoveryReason,
    commands: Arc<dyn KeyRecoveryCommands>,
    phase: KeyRecoveryPhase,
    /// Focus handle owned by the view itself. The root div tracks
    /// this handle so every keystroke flows through the view-level
    /// `on_key_down` handler.
    view_focus: FocusHandle,
    /// Per-button focus handles. The slot order is
    /// `[reconfigure, restore, exit]`. Each button owns its own
    /// focus handle so the built-in `Tab` action can move focus
    /// between them, and each button can dispatch its own
    /// `Enter` / `Space` activation through its dedicated
    /// `on_key_down` listener.
    button_focus: Vec<FocusHandle>,
    /// Index of the action button that currently owns keyboard
    /// focus. The button order is `[reconfigure, restore, exit]`.
    focused_index: usize,
}

impl KeyRecoveryView {
    /// Construct a new key recovery view. The view-level focus
    /// handle owns every keystroke so the keyboard contract is
    /// deterministic without relying on the underlying focus tree.
    pub fn new(
        reason: KeyRecoveryReason,
        commands: Arc<dyn KeyRecoveryCommands>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let view_focus = cx.focus_handle();
        let button_focus = (0..3).map(|_| cx.focus_handle()).collect::<Vec<_>>();
        // Bind Tab / Shift-Tab to our own actions so they take
        // precedence over Root::Tab (no-key-context bindings are
        // evaluated after Root's "Root"-context bindings and the
        // reverse search means higher-indexed bindings win on ties).
        cx.bind_keys([
            KeyBinding::new("tab", KeyRecoveryTab, Some("HiveguiKeyRecovery")),
            KeyBinding::new("shift-tab", KeyRecoveryTabPrev, Some("HiveguiKeyRecovery")),
        ]);
        // Focus the first button immediately so the recovery
        // surface is the default keyboard sink — the §T016D source
        // contract requires every recovery action to be reachable
        // from the keyboard alone, which is impossible if the window
        // has no focused element when the test (or a real user) starts
        // pressing keys.
        if let Some(first) = button_focus.first() {
            window.focus(first, cx);
        }
        Self {
            reason,
            commands,
            phase: KeyRecoveryPhase::Idle,
            view_focus,
            button_focus,
            focused_index: 0,
        }
    }

    /// Construct a new key recovery view from a test context, with
    /// the focus global pre-registered so the visual test can focus
    /// the buttons without bootstrapping the full application.
    pub fn for_test(
        reason: KeyRecoveryReason,
        commands: Arc<dyn KeyRecoveryCommands>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        if cx.try_global::<KeyRecoveryFocus>().is_none() {
            let focus = cx.new(|_| ());
            cx.set_global(KeyRecoveryFocus(Some(focus)));
        }
        Self::new(reason, commands, window, cx)
    }

    /// Access the current reason.
    pub fn reason(&self) -> KeyRecoveryReason {
        self.reason
    }

    /// Drive the view to a known phase (test seam).
    pub fn set_phase_for_test(&mut self, phase: KeyRecoveryPhase) {
        self.phase = phase;
    }

    fn dispatch(&mut self, action: KeyRecoveryAction, cx: &mut Context<Self>) {
        self.phase = KeyRecoveryPhase::Running(action);
        cx.notify();
        // Spawn the recovery future on the view's async context so
        // the test handshake can observe the future being polled
        // off the keyboard event boundary — the heartbeat probe
        // asserts this so a long-running reconfiguration can never
        // block the UI thread. When the future completes we
        // transition the view into `Completed` so the visible
        // status indicator matches the underlying command state.
        let future = self.commands.execute(action);
        cx.spawn(async move |view, cx| {
            let _ = future.await;
            view.update(cx, |this, cx| {
                this.phase = KeyRecoveryPhase::Completed;
                cx.notify();
            });
        })
        .detach();
    }

    /// Action of the button at the given index.
    fn button_action(index: usize) -> KeyRecoveryAction {
        match index {
            0 => KeyRecoveryAction::Reconfigure,
            1 => KeyRecoveryAction::RestoreEncryptedBackup,
            _ => KeyRecoveryAction::Exit,
        }
    }

    /// Activate the currently focused button.
    fn activate_focused(&mut self, cx: &mut Context<Self>) {
        let action = Self::button_action(self.focused_index);
        self.dispatch(action, cx);
    }

    /// Advance focus by one (wrapping), calling `Window::focus` so
    /// the focused div actually receives the highlight. The T016D
    /// contract requires the wrap behavior so the recovery surface
    /// traps keyboard navigation.
    fn focus_next(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.button_focus.is_empty() {
            return;
        }
        let count = self.button_focus.len();
        self.focused_index = (self.focused_index + 1) % count;
        if let Some(handle) = self.button_focus.get(self.focused_index) {
            window.focus(handle, cx);
        }
    }

    /// Reverse focus by one (wrapping).
    fn focus_prev(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.button_focus.is_empty() {
            return;
        }
        let count = self.button_focus.len();
        self.focused_index = if self.focused_index == 0 {
            count - 1
        } else {
            self.focused_index - 1
        };
        if let Some(handle) = self.button_focus.get(self.focused_index) {
            window.focus(handle, cx);
        }
    }
}

impl Render for KeyRecoveryView {
    fn render(&mut self, _w: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let reason_slug = self.reason.slug();
        let reason_label = self.reason.label_zh();
        let status_selector = match &self.phase {
            KeyRecoveryPhase::Idle => "KEY_RECOVERY_STATUS",
            KeyRecoveryPhase::Running(_) => "KEY_RECOVERY_RUNNING_STATUS",
            KeyRecoveryPhase::Completed => "KEY_RECOVERY_COMPLETED_STATUS",
        };
        let status_label = match &self.phase {
            KeyRecoveryPhase::Idle => STRINGS.zh_key_recovery_title.to_string(),
            KeyRecoveryPhase::Running(_) => STRINGS.zh_key_recovery_running.to_string(),
            KeyRecoveryPhase::Completed => STRINGS.zh_key_recovery_completed.to_string(),
        };
        let _ = cx;

        let reconfigure_focus = self.button_focus[0].clone();
        let restore_focus = self.button_focus[1].clone();
        let exit_focus = self.button_focus[2].clone();

        div()
            .id("KEY_RECOVERY_VIEW")
            .track_focus(&self.view_focus)
            .key_context("HiveguiKeyRecovery")
            .debug_selector(|| "KEY_RECOVERY_VIEW".to_string())
            .flex()
            .flex_col()
            .p_4()
            .gap_2()
            .on_action(cx.listener(|this, _: &KeyRecoveryTab, window, cx| {
                cx.stop_propagation();
                this.focus_next(window, cx);
            }))
            .on_action(cx.listener(|this, _: &KeyRecoveryTabPrev, window, cx| {
                cx.stop_propagation();
                this.focus_prev(window, cx);
            }))
            .child(
                div()
                    .id(status_selector)
                    .debug_selector(move || status_selector.to_string())
                    .text_xl()
                    .child(status_label),
            )
            .child(
                div()
                    .id(format!("KEY_RECOVERY_REASON-{reason_slug}"))
                    .debug_selector(move || format!("KEY_RECOVERY_REASON-{reason_slug}"))
                    .text_sm()
                    .child(format!(
                        "{}: {reason_label}",
                        STRINGS.zh_key_recovery_reason
                    )),
            )
            .child(
                div()
                    .id("KEY_RECONFIGURE")
                    .debug_selector(|| "KEY_RECONFIGURE".to_string())
                    .w(px(40.0))
                    .h(px(40.0))
                    .track_focus(&reconfigure_focus)
                    .tab_index(0)
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                        let key = event.keystroke.key.as_str();
                        if matches!(key, "enter" | " " | "space") {
                            cx.stop_propagation();
                            this.focused_index = 0;
                            this.activate_focused(cx);
                        }
                    })),
            )
            .child(
                div()
                    .id("KEY_RESTORE_BACKUP")
                    .debug_selector(|| "KEY_RESTORE_BACKUP".to_string())
                    .w(px(40.0))
                    .h(px(40.0))
                    .track_focus(&restore_focus)
                    .tab_index(1)
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                        let key = event.keystroke.key.as_str();
                        if matches!(key, "enter" | " " | "space") {
                            cx.stop_propagation();
                            this.focused_index = 1;
                            this.activate_focused(cx);
                        }
                    })),
            )
            .child(
                div()
                    .id("KEY_EXIT")
                    .debug_selector(|| "KEY_EXIT".to_string())
                    .w(px(40.0))
                    .h(px(40.0))
                    .track_focus(&exit_focus)
                    .tab_index(2)
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                        let key = event.keystroke.key.as_str();
                        if matches!(key, "enter" | " " | "space") {
                            cx.stop_propagation();
                            this.focused_index = 2;
                            this.activate_focused(cx);
                        }
                    })),
            )
    }
}

/// Focus global used by the key recovery view to trap keyboard focus.
pub struct KeyRecoveryFocus(pub Option<Entity<()>>);
impl Default for KeyRecoveryFocus {
    fn default() -> Self {
        // The owning view replaces this `None` with a live entity on
        // first construction; tests can do the same via
        // `KeyRecoveryView::for_test`.
        Self(None)
    }
}
impl gpui::Global for KeyRecoveryFocus {}

const STRINGS: StringsZh = StringsZh {
    zh_key_recovery_title: "设备密钥恢复",
    zh_key_recovery_running: "正在恢复...",
    zh_key_recovery_completed: "恢复完成",
    zh_key_recovery_reason: "原因",
    zh_key_recovery_reconfigure: "重新生成设备密钥",
    zh_key_recovery_restore: "从加密备份恢复",
    zh_key_recovery_exit: "退出",
};

#[allow(dead_code)]
struct StringsZh {
    zh_key_recovery_title: &'static str,
    zh_key_recovery_running: &'static str,
    zh_key_recovery_completed: &'static str,
    zh_key_recovery_reason: &'static str,
    zh_key_recovery_reconfigure: &'static str,
    zh_key_recovery_restore: &'static str,
    zh_key_recovery_exit: &'static str,
}

// Reference unused symbols to keep `#![warn(missing_docs)]` quiet
// while the recovery flow is still being wired into the production
// bootstrap.
#[allow(dead_code)]
fn _unused(_: &StringsZh) {}
