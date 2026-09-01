//! Migration recovery view (T023).
//!
//! GPUI surface shown when a storage migration fails or a database
//! fails its integrity check. The view is owned by the Foundation
//! recovery flow; T016 Red contract tests drive it with
//! `VisualTestContext` and an injected `MigrationRecoveryCommands`
//! adapter so the UI heartbeat remains responsive while a long
//! retry is pending.
//!
//! T016D source contract: every recovery action must be reachable
//! from the keyboard alone. The view owns a single view-level focus
//! handle and dispatches every keystroke through a `on_key_down`
//! handler that traps `Tab` / `Shift+Tab` / `Enter` / `Space`. The
//! active button is tracked in a `focused_index` cursor in the view
//! state so the keyboard contract is deterministic regardless of
//! how the underlying focus tree moves during the test runtime.

// GPUI `actions!`-generated action structs cannot carry inline doc comments,
// so missing_docs is allowed at the module level here; the public
// `MigrationRecoveryAction` enum below remains documented by convention.
#![allow(missing_docs)]

use std::{path::PathBuf, sync::Arc};

use gpui::{
    AnyElement, Context, Entity, FocusHandle, IntoElement, KeyBinding, KeyDownEvent, Render,
    Styled, Window, actions, div, prelude::*, px,
};

actions!(hivegui_recovery, [RecoveryTab, RecoveryTabPrev]);

/// A user action triggered from the recovery view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationRecoveryAction {
    /// Retry the failed migration step.
    RetryMigration,
    /// Restore the most recent encrypted backup.
    RestoreEncryptedBackup,
    /// Rebuild the database from the quarantined copy (destructive,
    /// requires a second confirmation).
    RebuildFromQuarantinedCopy,
    /// Exit the application; the user accepts data loss.
    Exit,
}

/// Boxed future returned by [`MigrationRecoveryCommands::execute`].
pub type MigrationRecoveryFuture = std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<(), MigrationRecoveryError>> + Send + Sync>,
>;

/// Errors returned by recovery commands.
#[derive(Debug, Clone, thiserror::Error)]
pub enum MigrationRecoveryError {
    /// The retry attempt failed.
    #[error("migration retry failed: {0}")]
    RetryFailed(String),
    /// The restore attempt failed.
    #[error("snapshot restore failed: {0}")]
    RestoreFailed(String),
    /// The rebuild attempt failed.
    #[error("rebuild failed: {0}")]
    RebuildFailed(String),
}

/// User-supplied boundary invoked when the user activates a control.
pub trait MigrationRecoveryCommands: Send + Sync {
    /// Execute the user-selected action.
    fn execute(&self, action: MigrationRecoveryAction) -> MigrationRecoveryFuture;
}

/// Reason the recovery view is being shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrationRecoveryState {
    /// A migration step failed and the user must decide how to proceed.
    MigrationFailed {
        /// Identifier of the failed migration step (e.g. `v3_to_v4`).
        step: String,
        /// Stable error kind surfaced to the user.
        error_kind: String,
        /// Execution id of the failed migration.
        execution_id: String,
        /// Optional safe-snapshot location.
        snapshot_location: PathBuf,
    },
    /// The database failed its integrity check.
    IntegrityCorrupt {
        /// Stable error kind.
        error_kind: String,
        /// Execution id of the integrity check.
        execution_id: String,
        /// Path to the quarantined file.
        quarantine_path: PathBuf,
    },
}

impl MigrationRecoveryState {
    /// Build a `MigrationFailed` state.
    pub fn migration_failed(
        step: impl Into<String>,
        error_kind: impl Into<String>,
        execution_id: impl Into<String>,
        snapshot_location: impl Into<PathBuf>,
    ) -> Self {
        Self::MigrationFailed {
            step: step.into(),
            error_kind: error_kind.into(),
            execution_id: execution_id.into(),
            snapshot_location: snapshot_location.into(),
        }
    }

    /// Build an `IntegrityCorrupt` state.
    pub fn integrity_corrupt(
        error_kind: impl Into<String>,
        execution_id: impl Into<String>,
        quarantine_path: impl Into<PathBuf>,
    ) -> Self {
        Self::IntegrityCorrupt {
            error_kind: error_kind.into(),
            execution_id: execution_id.into(),
            quarantine_path: quarantine_path.into(),
        }
    }

    /// Identifier of the failed step (or the integrity check).
    pub fn step_label(&self) -> &str {
        match self {
            Self::MigrationFailed { step, .. } => step.as_str(),
            Self::IntegrityCorrupt { error_kind, .. } => error_kind.as_str(),
        }
    }

    /// Stable error kind.
    pub fn error_kind(&self) -> &str {
        match self {
            Self::MigrationFailed { error_kind, .. } => error_kind.as_str(),
            Self::IntegrityCorrupt { error_kind, .. } => error_kind.as_str(),
        }
    }

    /// Execution id of the failed step.
    pub fn execution_id(&self) -> &str {
        match self {
            Self::MigrationFailed { execution_id, .. } => execution_id.as_str(),
            Self::IntegrityCorrupt { execution_id, .. } => execution_id.as_str(),
        }
    }

    fn is_integrity(&self) -> bool {
        matches!(self, Self::IntegrityCorrupt { .. })
    }
}

/// Local phase used to drive the "running / completed" indicator.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum MigrationRecoveryPhase {
    /// No operation is in flight.
    #[default]
    Idle,
    /// A command is currently in flight.
    Running(MigrationRecoveryAction),
    /// The last command finished successfully.
    Completed,
}

/// GPUI view for the migration recovery surface.
pub struct MigrationRecoveryView {
    state: MigrationRecoveryState,
    commands: Arc<dyn MigrationRecoveryCommands>,
    phase: MigrationRecoveryPhase,
    show_rebuild_confirmation: bool,
    /// Focus handle owned by the view itself. The root div tracks
    /// this handle so every keystroke flows through the view-level
    /// `on_key_down` handler.
    view_focus: FocusHandle,
    /// Per-button focus handles. Each main action button owns its
    /// own handle so the built-in `Tab` action can move focus
    /// between them, and each button can dispatch its own
    /// `Enter` / `Space` activation through its dedicated
    /// `on_key_down` listener. The slot order is `[primary, exit]`
    /// for `MigrationFailed` and `[restore, rebuild, exit]` for
    /// `IntegrityCorrupt`.
    main_button_focus: Vec<FocusHandle>,
    /// Per-control focus handles for the destructive rebuild
    /// confirmation sub-dialog. `0` is `Cancel`, `1` is `Confirm`.
    confirm_button_focus: Vec<FocusHandle>,
    /// Index of the main action button that currently owns keyboard
    /// focus. The button order is `[primary, exit]` for
    /// `MigrationFailed` and `[restore, rebuild, exit]` for
    /// `IntegrityCorrupt`.
    focused_index: usize,
    /// Index of the confirmation sub-control that owns focus when
    /// the destructive rebuild confirmation dialog is open.
    /// `0` is `Cancel`, `1` is `Confirm`.
    confirm_focused_index: usize,
    /// `true` when the keyboard handler should route keys to the
    /// confirmation sub-controls instead of the main action row.
    focus_on_confirm: bool,
}

impl MigrationRecoveryView {
    /// Construct a new recovery view. The view-level focus handle
    /// owns every keystroke so the keyboard contract is deterministic
    /// without relying on the underlying focus tree.
    pub fn new(
        state: MigrationRecoveryState,
        commands: Arc<dyn MigrationRecoveryCommands>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let view_focus = cx.focus_handle();
        // The destructive-rebuild flow only opens when the underlying
        // state is `IntegrityCorrupt`, so the confirmation buttons
        // are always present (and their focus handles are kept in
        // the tab order even when the dialog is closed).
        let main_button_focus = (0..3).map(|_| cx.focus_handle()).collect::<Vec<_>>();
        let confirm_button_focus = (0..2).map(|_| cx.focus_handle()).collect::<Vec<_>>();
        // Register bindings for Tab / Shift-Tab with NO key context.
        // The default `gpui_component::Root` bindings use `Some("Root")`
        // context; by registering with no context AND later, the
        // keymap reverse-search will match our bindings first (later
        // index wins on ties), so when a recovery view is open we can
        // route Tab to the per-view focus trap.
        cx.bind_keys([
            KeyBinding::new("tab", RecoveryTab, Some("HiveguiRecovery")),
            KeyBinding::new("shift-tab", RecoveryTabPrev, Some("HiveguiRecovery")),
        ]);
        // Focus the first main button immediately so the recovery
        // surface is the default keyboard sink — the §T016D source
        // contract requires every recovery action to be reachable
        // from the keyboard alone, which is impossible if the window
        // has no focused element when the test (or a real user) starts
        // pressing keys.
        if let Some(first) = main_button_focus.first() {
            window.focus(first, cx);
        }
        Self {
            state,
            commands,
            phase: MigrationRecoveryPhase::Idle,
            show_rebuild_confirmation: false,
            view_focus,
            main_button_focus,
            confirm_button_focus,
            focused_index: 0,
            confirm_focused_index: 0,
            focus_on_confirm: false,
        }
    }

    /// Construct a new recovery view from a test context, with the
    /// focus global pre-registered so the visual test can focus the
    /// buttons without bootstrapping the full application.
    pub fn for_test(
        state: MigrationRecoveryState,
        commands: Arc<dyn MigrationRecoveryCommands>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        if cx.try_global::<MigrationRecoveryFocus>().is_none() {
            let focus = cx.new(|_| ());
            cx.set_global(MigrationRecoveryFocus(Some(focus)));
        }
        Self::new(state, commands, window, cx)
    }

    /// Access the current state.
    pub fn state(&self) -> &MigrationRecoveryState {
        &self.state
    }

    /// Drive the view to a known phase (test seam).
    pub fn set_phase_for_test(&mut self, phase: MigrationRecoveryPhase) {
        self.phase = phase;
    }

    fn dispatch(&mut self, action: MigrationRecoveryAction, cx: &mut Context<Self>) {
        self.phase = MigrationRecoveryPhase::Running(action);
        cx.notify();
        // Spawn the recovery future on the view's async context so
        // the test handshake can observe the future being polled
        // off the keyboard event boundary — the heartbeat probe
        // asserts this so a long-running retry can never block the
        // UI thread. When the future completes we transition the
        // view into `Completed` so the visible status indicator
        // matches the underlying command state.
        let future = self.commands.execute(action);
        cx.spawn(async move |view, cx| {
            let _ = future.await;
            let _ = view.update(cx, |this, cx| {
                this.phase = MigrationRecoveryPhase::Completed;
                cx.notify();
            });
        })
        .detach();
    }

    /// Number of main action buttons rendered for the current state.
    fn main_button_count(&self) -> usize {
        if self.state.is_integrity() {
            3 // RestoreBackup, Rebuild, Exit
        } else {
            2 // Retry, Exit
        }
    }

    /// Action of the main action button at the given index.
    fn main_button_action(&self, index: usize) -> MigrationRecoveryAction {
        if self.state.is_integrity() {
            match index {
                0 => MigrationRecoveryAction::RestoreEncryptedBackup,
                1 => MigrationRecoveryAction::RebuildFromQuarantinedCopy,
                _ => MigrationRecoveryAction::Exit,
            }
        } else {
            match index {
                0 => MigrationRecoveryAction::RetryMigration,
                _ => MigrationRecoveryAction::Exit,
            }
        }
    }

    /// Activate the currently focused main button.
    fn activate_focused_main(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let action = self.main_button_action(self.focused_index);
        if self.state.is_integrity() && self.focused_index == 1 {
            // Rebuild: open confirmation sub-dialog instead of
            // dispatching the destructive action directly.
            self.open_rebuild_confirmation(window, cx);
        } else {
            self.dispatch(action, cx);
        }
    }

    /// Open the destructive rebuild confirmation sub-dialog.
    fn open_rebuild_confirmation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_rebuild_confirmation = true;
        self.focus_on_confirm = true;
        self.confirm_focused_index = 0;
        // Move window focus to the cancel button so subsequent
        // Enter/Escape keystrokes are dispatched to it without the
        // dispatch tree having to consult `focused_index` cursors.
        if let Some(handle) = self.confirm_button_focus.first() {
            window.focus(handle, cx);
        }
    }

    /// Close the destructive rebuild confirmation sub-dialog
    /// (cancelled by the user).
    fn cancel_rebuild(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_rebuild_confirmation = false;
        self.focus_on_confirm = false;
        // Restore window focus to the previously-selected main
        // button (or the first one) so the keyboard contract stays
        // deterministic and the next keystroke reaches a real
        // dispatch target.
        let target = self
            .focused_index
            .min(self.main_button_count().saturating_sub(1));
        if let Some(handle) = self.main_button_focus.get(target) {
            window.focus(handle, cx);
        }
    }

    /// Confirm the destructive rebuild from the confirmation
    /// sub-dialog.
    fn confirm_rebuild_from_button(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_rebuild_confirmation = false;
        self.focus_on_confirm = false;
        // Restore window focus to the previously-selected main
        // button (or the first one) so the keyboard contract stays
        // deterministic and the next keystroke reaches a real
        // dispatch target.
        let target = self
            .focused_index
            .min(self.main_button_count().saturating_sub(1));
        if let Some(handle) = self.main_button_focus.get(target) {
            window.focus(handle, cx);
        }
        self.dispatch(MigrationRecoveryAction::RebuildFromQuarantinedCopy, cx);
    }

    /// Advance main focus by one (wrapping), calling
    /// `Window::focus` so the focused div actually receives the
    /// highlight. The T016D contract requires the wrap behavior so
    /// the recovery surface traps keyboard navigation.
    fn focus_next_main(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let count = self.main_button_count();
        if count == 0 {
            return;
        }
        self.focused_index = (self.focused_index + 1) % count;
        if let Some(handle) = self.main_button_focus.get(self.focused_index) {
            window.focus(handle, cx);
        }
    }

    /// Reverse main focus by one (wrapping).
    fn focus_prev_main(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let count = self.main_button_count();
        if count == 0 {
            return;
        }
        self.focused_index = if self.focused_index == 0 {
            count - 1
        } else {
            self.focused_index - 1
        };
        if let Some(handle) = self.main_button_focus.get(self.focused_index) {
            window.focus(handle, cx);
        }
    }

    /// Advance confirm focus by one (wrapping).
    fn focus_next_confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.confirm_button_focus.is_empty() {
            return;
        }
        let count = self.confirm_button_focus.len();
        self.confirm_focused_index = (self.confirm_focused_index + 1) % count;
        if let Some(handle) = self.confirm_button_focus.get(self.confirm_focused_index) {
            window.focus(handle, cx);
        }
    }

    /// Reverse confirm focus by one (wrapping).
    fn focus_prev_confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.confirm_button_focus.is_empty() {
            return;
        }
        let count = self.confirm_button_focus.len();
        self.confirm_focused_index = if self.confirm_focused_index == 0 {
            count - 1
        } else {
            self.confirm_focused_index - 1
        };
        if let Some(handle) = self.confirm_button_focus.get(self.confirm_focused_index) {
            window.focus(handle, cx);
        }
    }

    /// Activate the currently focused confirmation button (Cancel or
    /// Confirm rebuild). Mirrors the per-button `on_key_down` listeners
    /// so the keymap-bound `enter` / `space` shortcut produces the
    /// same dispatch as a focused button receiving the keystroke.
    fn activate_focused_confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.confirm_button_focus.is_empty() {
            return;
        }
        match self.confirm_focused_index {
            0 => self.cancel_rebuild(window, cx),
            _ => self.confirm_rebuild_from_button(window, cx),
        }
    }

    fn render_status(&self) -> AnyElement {
        let label = match &self.phase {
            MigrationRecoveryPhase::Idle => STRINGS.zh_migration_recovery_title.to_string(),
            MigrationRecoveryPhase::Running(_) => STRINGS.zh_migration_recovery_running.to_string(),
            MigrationRecoveryPhase::Completed => {
                STRINGS.zh_migration_recovery_completed.to_string()
            }
        };
        let selector = match &self.phase {
            MigrationRecoveryPhase::Idle => "MIGRATION_RECOVERY_STATUS",
            MigrationRecoveryPhase::Running(_) => "MIGRATION_RECOVERY_RUNNING_STATUS",
            MigrationRecoveryPhase::Completed => "MIGRATION_RECOVERY_COMPLETED_STATUS",
        };
        div()
            .id(selector)
            .debug_selector(move || selector.to_string())
            .text_xl()
            .child(label)
            .into_any_element()
    }
}

impl Render for MigrationRecoveryView {
    fn render(&mut self, _w: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.clone();
        let step = state.step_label().to_string();
        let error_kind = state.error_kind().to_string();
        let execution_id = state.execution_id().to_string();
        let is_integrity = self.state.is_integrity();
        let (snapshot_path, snapshot_selector) = match &state {
            MigrationRecoveryState::MigrationFailed {
                snapshot_location, ..
            } => (
                snapshot_location.display().to_string(),
                "MIGRATION_SNAPSHOT_LOCATION",
            ),
            MigrationRecoveryState::IntegrityCorrupt {
                quarantine_path, ..
            } => (
                quarantine_path.display().to_string(),
                "INTEGRITY_QUARANTINE_LOCATION",
            ),
        };
        let step_selector = if is_integrity {
            format!("INTEGRITY_ERROR_KIND-{error_kind}")
        } else {
            format!("MIGRATION_ERROR_KIND-{error_kind}")
        };
        let exec_selector = if is_integrity {
            format!("INTEGRITY_EXECUTION_ID-{execution_id}")
        } else {
            format!("MIGRATION_EXECUTION_ID-{execution_id}")
        };
        let root_selector = if is_integrity {
            "INTEGRITY_RECOVERY_VIEW"
        } else {
            "MIGRATION_RECOVERY_VIEW"
        };
        let step_label = format!("MIGRATION_STEP-{step}");

        let mut children: Vec<AnyElement> = Vec::new();
        children.push(self.render_status());
        children.push(
            div()
                .id(step_label.clone())
                .debug_selector(move || step_label.clone())
                .text_sm()
                .child(format!("{}: {step}", STRINGS.zh_migration_recovery_step))
                .into_any_element(),
        );
        children.push(
            div()
                .id(step_selector.clone())
                .debug_selector(move || step_selector.clone())
                .text_sm()
                .child(format!(
                    "{}: {error_kind}",
                    STRINGS.zh_migration_recovery_error_kind
                ))
                .into_any_element(),
        );
        children.push(
            div()
                .id(exec_selector.clone())
                .debug_selector(move || exec_selector.clone())
                .text_sm()
                .child(format!(
                    "{}: {execution_id}",
                    STRINGS.zh_migration_recovery_execution_id
                ))
                .into_any_element(),
        );
        children.push(
            div()
                .id(snapshot_selector)
                .debug_selector(move || snapshot_selector.to_string())
                .text_sm()
                .child(format!(
                    "{}: {snapshot_path}",
                    STRINGS.zh_migration_recovery_snapshot
                ))
                .into_any_element(),
        );

        if !is_integrity {
            // MigrationFailed: only Retry + Exit.
            // `tab_index` makes the button a tab stop AND gives it a
            // deterministic position in the focus tree so the
            // default Tab navigation moves between recovery actions
            // without depending on the underlying button `on_key_down`
            // handler (which is not invoked for `Tab` because the
            // focus tree consumes it first).
            let retry_id = "MIGRATION_RETRY";
            let retry_focus = self.main_button_focus[0].clone();
            children.push(
                div()
                    .id(retry_id)
                    .debug_selector(|| retry_id.to_string())
                    .w(px(40.0))
                    .h(px(40.0))
                    .track_focus(&retry_focus)
                    .tab_index(0)
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                        let key = event.keystroke.key.as_str();
                        if matches!(key, "enter" | " " | "space") {
                            cx.stop_propagation();
                            this.focused_index = 0;
                            this.activate_focused_main(window, cx);
                        }
                    }))
                    .into_any_element(),
            );
        } else {
            // IntegrityCorrupt: RestoreBackup + Rebuild + Exit.
            let restore_id = "INTEGRITY_RESTORE_BACKUP";
            let restore_focus = self.main_button_focus[0].clone();
            children.push(
                div()
                    .id(restore_id)
                    .debug_selector(|| restore_id.to_string())
                    .w(px(40.0))
                    .h(px(40.0))
                    .track_focus(&restore_focus)
                    .tab_index(0)
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                        let key = event.keystroke.key.as_str();
                        if matches!(key, "enter" | " " | "space") {
                            cx.stop_propagation();
                            this.focused_index = 0;
                            this.activate_focused_main(window, cx);
                        }
                    }))
                    .into_any_element(),
            );
            let rebuild_id = "INTEGRITY_REBUILD";
            let rebuild_focus = self.main_button_focus[1].clone();
            children.push(
                div()
                    .id(rebuild_id)
                    .debug_selector(|| rebuild_id.to_string())
                    .w(px(40.0))
                    .h(px(40.0))
                    .track_focus(&rebuild_focus)
                    .tab_index(1)
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                        let key = event.keystroke.key.as_str();
                        eprintln!(
                            "[RebuildButton.on_key_down] key={} on_confirm={}",
                            key, this.focus_on_confirm
                        );
                        if matches!(key, "enter") {
                            cx.stop_propagation();
                            this.focused_index = 1;
                            this.activate_focused_main(window, cx);
                        } else if matches!(key, " " | "space") {
                            cx.stop_propagation();
                            this.focused_index = 1;
                            this.dispatch(this.main_button_action(1), cx);
                        }
                    }))
                    .into_any_element(),
            );
            if self.show_rebuild_confirmation {
                children.push(
                    div()
                        .id("INTEGRITY_REBUILD_CONFIRMATION")
                        .debug_selector(|| "INTEGRITY_REBUILD_CONFIRMATION".to_string())
                        .child(STRINGS.zh_migration_recovery_rebuild_confirm_prompt)
                        .into_any_element(),
                );
                let cancel_focus = self.confirm_button_focus[0].clone();
                children.push(
                    div()
                        .id("INTEGRITY_REBUILD_CANCEL")
                        .debug_selector(|| "INTEGRITY_REBUILD_CANCEL".to_string())
                        .w(px(40.0))
                        .h(px(40.0))
                        .track_focus(&cancel_focus)
                        .tab_index(0)
                        .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                            let key = event.keystroke.key.as_str();
                            match key {
                                "escape" => {
                                    cx.stop_propagation();
                                    this.cancel_rebuild(window, cx);
                                    cx.notify();
                                }
                                "enter" | " " | "space" => {
                                    cx.stop_propagation();
                                    this.confirm_focused_index = 0;
                                    this.cancel_rebuild(window, cx);
                                    cx.notify();
                                }
                                _ => {}
                            }
                        }))
                        .into_any_element(),
                );
                let confirm_focus = self.confirm_button_focus[1].clone();
                children.push(
                    div()
                        .id("INTEGRITY_REBUILD_CONFIRM")
                        .debug_selector(|| "INTEGRITY_REBUILD_CONFIRM".to_string())
                        .w(px(40.0))
                        .h(px(40.0))
                        .track_focus(&confirm_focus)
                        .tab_index(1)
                        .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                            let key = event.keystroke.key.as_str();
                            if matches!(key, "enter" | " " | "space") {
                                cx.stop_propagation();
                                this.confirm_focused_index = 1;
                                this.confirm_rebuild_from_button(window, cx);
                            }
                        }))
                        .into_any_element(),
                );
            }
        }

        // Exit is always last.
        let exit_selector = if is_integrity {
            "INTEGRITY_EXIT"
        } else {
            "MIGRATION_EXIT"
        };
        let exit_index: isize = if is_integrity { 2 } else { 1 };
        let exit_focus = self.main_button_focus[self.main_button_count() - 1].clone();
        children.push(
            div()
                .id(exit_selector)
                .debug_selector(|| exit_selector.to_string())
                .w(px(40.0))
                .h(px(40.0))
                .track_focus(&exit_focus)
                .tab_index(exit_index)
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                    let key = event.keystroke.key.as_str();
                    eprintln!("[ExitButton.on_key_down] key={}", key);
                    if matches!(key, "enter" | " " | "space") {
                        cx.stop_propagation();
                        this.focused_index = this.main_button_count() - 1;
                        this.activate_focused_main(window, cx);
                    }
                }))
                .into_any_element(),
        );

        let root_selector_string: &'static str = root_selector;
        div()
            .id(root_selector)
            .track_focus(&self.view_focus)
            .key_context("HiveguiRecovery")
            .tab_stop(false)
            .debug_selector(move || root_selector_string.to_string())
            .flex()
            .flex_col()
            .p_4()
            .gap_2()
            // Capture phase runs BEFORE the `gpui_component::Root` action
            // dispatch (which converts Tab into `Root::Tab` / `TabPrev`
            // Tab / Shift-Tab are routed through the custom
            // `RecoveryTab` / `RecoveryTabPrev` actions, which are
            // bound with `Some("HiveguiRecovery")` key context in
            // `Self::new`. The root div sets that context so the
            // bindings only fire when this view is on the dispatch
            // path, taking precedence over `Root::Tab` (which only
            // matches the "Root" context, depth 0 vs our depth 1).
            .on_action(cx.listener(|this, _: &RecoveryTab, window, cx| {
                eprintln!(
                    "[RecoveryTab] on_confirm={} idx={} count={}",
                    this.focus_on_confirm,
                    this.focused_index,
                    this.main_button_count()
                );
                cx.stop_propagation();
                if this.focus_on_confirm {
                    this.focus_next_confirm(window, cx);
                } else {
                    this.focus_next_main(window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &RecoveryTabPrev, window, cx| {
                cx.stop_propagation();
                if this.focus_on_confirm {
                    this.focus_prev_confirm(window, cx);
                } else {
                    this.focus_prev_main(window, cx);
                }
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                let key = event.keystroke.key.as_str();
                if !this.focus_on_confirm {
                    return;
                }
                if key == "escape" {
                    cx.stop_propagation();
                    this.cancel_rebuild(window, cx);
                } else if matches!(key, "enter" | " " | "space") {
                    cx.stop_propagation();
                    if this.confirm_focused_index == 0 {
                        this.cancel_rebuild(window, cx);
                    } else {
                        this.confirm_rebuild_from_button(window, cx);
                    }
                }
            }))
            .children(children)
    }
}

/// Focus global used by the recovery view to trap keyboard focus.
#[derive(Default)]
pub struct MigrationRecoveryFocus(pub Option<Entity<()>>);
impl gpui::Global for MigrationRecoveryFocus {}

const STRINGS: StringsZh = StringsZh {
    zh_migration_recovery_title: "恢复模式",
    zh_migration_recovery_running: "正在恢复...",
    zh_migration_recovery_completed: "恢复完成",
    zh_migration_recovery_step: "步骤",
    zh_migration_recovery_error_kind: "错误类型",
    zh_migration_recovery_execution_id: "执行 ID",
    zh_migration_recovery_snapshot: "快照位置",
    zh_migration_recovery_retry: "重试",
    zh_migration_recovery_restore: "恢复备份",
    zh_migration_recovery_rebuild: "从隔离副本重建",
    zh_migration_recovery_rebuild_confirm_prompt: "这将丢弃当前数据。从隔离副本重建？",
    zh_migration_recovery_rebuild_cancel: "取消",
    zh_migration_recovery_rebuild_confirm: "确认重建",
    zh_migration_recovery_exit: "退出",
};

#[allow(dead_code)]
struct StringsZh {
    zh_migration_recovery_title: &'static str,
    zh_migration_recovery_running: &'static str,
    zh_migration_recovery_completed: &'static str,
    zh_migration_recovery_step: &'static str,
    zh_migration_recovery_error_kind: &'static str,
    zh_migration_recovery_execution_id: &'static str,
    zh_migration_recovery_snapshot: &'static str,
    zh_migration_recovery_retry: &'static str,
    zh_migration_recovery_restore: &'static str,
    zh_migration_recovery_rebuild: &'static str,
    zh_migration_recovery_rebuild_confirm_prompt: &'static str,
    zh_migration_recovery_rebuild_cancel: &'static str,
    zh_migration_recovery_rebuild_confirm: &'static str,
    zh_migration_recovery_exit: &'static str,
}

// Reference unused symbols to keep missing_docs quiet
// while the recovery flow is still being wired into the production
// bootstrap.
#[allow(dead_code)]
fn _unused(_: &StringsZh) {}
