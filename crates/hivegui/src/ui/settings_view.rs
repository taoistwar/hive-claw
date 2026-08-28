//! 数据管理视图。
//!
//! Provides session retention entry points, backup restore precheck +
//! replacement confirmation, export/import controls, and diagnostic
//! bundle export with redaction.
//! scroll:agent_execution

use std::path::Path;
use std::sync::Arc;

use gpui::*;
use gpui_component::ActiveTheme as _;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::scroll::ScrollableElement;
use sqlx::query_scalar;

use crate::datasource::Store;
use crate::datasource::backup::{
    BackupCoordinator, PreparedRestore, RestoreCoordinator, RestoreSafetyBackupState,
};
use crate::datasource::conversation_store::ConversationStore;
use crate::runtime::diagnostics::{DiagnosticBundle, ExecutionEventCollector, RedactionConfig};
const RETENTION_KEY: &str = "conversation_retention_days";

actions!(hivegui_settings, [SettingsEscape]);

#[cfg(test)]
struct RestorePreviewDeliveryInterlock {
    ready: tokio::sync::oneshot::Sender<()>,
    release: tokio::sync::oneshot::Receiver<()>,
}

#[cfg(test)]
struct RestoreCancelDeliveryInterlock {
    ready: tokio::sync::oneshot::Sender<()>,
    release: tokio::sync::oneshot::Receiver<()>,
    applied: tokio::sync::oneshot::Sender<()>,
}

type RestorePreviewOutcome = Result<(RestoreCoordinator, PreparedRestore, String), String>;

pub struct SettingsView {
    store: Option<Store>,
    collector: Option<Arc<ExecutionEventCollector>>,
    export_path: SharedString,
    import_path: SharedString,
    diagnostic_path: SharedString,
    retention_days: SharedString,
    backup_passphrase: SharedString,

    export_path_input: Option<Entity<InputState>>,
    import_path_input: Option<Entity<InputState>>,
    backup_passphrase_input: Option<Entity<InputState>>,
    diagnostic_path_input: Option<Entity<InputState>>,
    retention_days_input: Option<Entity<InputState>>,

    restore_impact_preview: String,
    restore_coordinator: Option<RestoreCoordinator>,
    prepared_restore: Option<Arc<PreparedRestore>>,
    restore_restart_required: bool,
    restore_preview_generation: u64,
    restore_preview_busy: bool,
    #[cfg(test)]
    restore_preview_delivery_interlock: Option<RestorePreviewDeliveryInterlock>,
    #[cfg(test)]
    restore_cancel_delivery_interlock: Option<RestoreCancelDeliveryInterlock>,
    #[cfg(test)]
    restore_cancel_dispatch_count: usize,
    retention_loaded: bool,
    is_exporting: bool,
    is_importing: bool,
    is_restoring_prechecked: bool,
    is_replacement_confirmed: bool,
    is_exporting_diagnostic: bool,
    restore_precheck_focus: FocusHandle,
    settings_focus: FocusHandle,
    content_scroll: ScrollHandle,
    initial_focus_installed: bool,
    status_message: Option<SharedString>,
    is_error: bool,
}

impl SettingsView {
    pub fn new(cx: &mut Context<Self>, collector: Option<Arc<ExecutionEventCollector>>) -> Self {
        cx.bind_keys([KeyBinding::new(
            "escape",
            SettingsEscape,
            Some("HiveguiSettings"),
        )]);
        Self {
            store: None,
            collector,
            export_path: "./hivegui-export.age.tar".into(),
            import_path: SharedString::new(""),
            diagnostic_path: SharedString::from(format!(
                "./hivegui-diagnostic-{}.json",
                uuid::Uuid::new_v4()
            )),
            retention_days: "365".into(),
            backup_passphrase: SharedString::new(""),
            export_path_input: None,
            import_path_input: None,
            backup_passphrase_input: None,
            diagnostic_path_input: None,
            retention_days_input: None,
            restore_impact_preview: "恢复前请先预检".into(),
            restore_coordinator: None,
            prepared_restore: None,
            restore_restart_required: false,
            restore_preview_generation: 0,
            restore_preview_busy: false,
            #[cfg(test)]
            restore_preview_delivery_interlock: None,
            #[cfg(test)]
            restore_cancel_delivery_interlock: None,
            #[cfg(test)]
            restore_cancel_dispatch_count: 0,
            retention_loaded: false,
            is_exporting: false,
            is_importing: false,
            is_restoring_prechecked: false,
            is_replacement_confirmed: false,
            is_exporting_diagnostic: false,
            restore_precheck_focus: cx.focus_handle(),
            settings_focus: cx.focus_handle(),
            content_scroll: ScrollHandle::default(),
            initial_focus_installed: false,
            status_message: None,
            is_error: false,
        }
    }

    pub fn set_store(&mut self, store: Store) {
        if self.store.is_some() {
            return;
        }
        self.store = Some(store);
        self.retention_loaded = false;
        self.is_restoring_prechecked = false;
        self.is_replacement_confirmed = false;
        self.restore_coordinator = None;
        self.prepared_restore = None;
        self.restore_restart_required = false;
        self.restore_impact_preview = "恢复前请先预检".to_string();
    }

    fn clear_status(&mut self) {
        if self.restore_restart_required {
            return;
        }
        self.status_message = None;
        self.is_error = false;
    }

    fn cancel_completed_restore_preview_after_input_change(&mut self, cx: &mut Context<Self>) {
        if self.restore_restart_required {
            return;
        }

        self.restore_preview_generation = self.restore_preview_generation.wrapping_add(1);
        self.is_restoring_prechecked = false;
        self.is_replacement_confirmed = false;
        self.restore_impact_preview = "恢复前请先预检".to_string();

        if self.restore_preview_busy {
            return;
        }

        let Some(prepared) = self.prepared_restore.take() else {
            return;
        };
        let Some(coordinator) = self.restore_coordinator.take() else {
            self.prepared_restore = Some(prepared);
            return;
        };
        self.cancel_exact_restore_preview(coordinator, prepared, None, cx);
    }

    fn cancel_exact_restore_preview(
        &mut self,
        coordinator: RestoreCoordinator,
        prepared: Arc<PreparedRestore>,
        success_status: Option<&'static str>,
        cx: &mut Context<Self>,
    ) {
        self.restore_preview_busy = true;
        self.restore_coordinator = Some(coordinator.clone());
        self.prepared_restore = Some(prepared.clone());
        #[cfg(test)]
        {
            self.restore_cancel_dispatch_count += 1;
        }
        let entity = cx.entity();
        let cancellation_coordinator = coordinator.clone();
        let cancellation_prepared = prepared.clone();
        let cancellation = crate::ui::spawn_tokio(async move {
            cancellation_coordinator
                .cancel_preview(cancellation_prepared.as_ref())
                .await
        });
        #[cfg(test)]
        let delivery_interlock = self.restore_cancel_delivery_interlock.take();
        cx.spawn(async move |_this, cx| {
            let outcome = cancellation.await;
            #[cfg(test)]
            let applied = if let Some(interlock) = delivery_interlock {
                let _ = interlock.ready.send(());
                let _ = interlock.release.await;
                Some(interlock.applied)
            } else {
                None
            };
            entity.update(cx, move |view, cx| {
                let exact_pair = view.restore_coordinator.is_some()
                    && view
                        .prepared_restore
                        .as_ref()
                        .is_some_and(|current| Arc::ptr_eq(current, &prepared));
                match outcome {
                    Ok(Ok(())) => {
                        if view.restore_restart_required || !exact_pair {
                            return;
                        }
                        view.restore_coordinator = None;
                        view.prepared_restore = None;
                        view.restore_preview_busy = false;
                        if let Some(status) = success_status {
                            view.set_status(status, false);
                        }
                    }
                    Ok(Err(_)) | Err(_) => {
                        let _ = coordinator.enter_preview_cancel_failure_terminal();
                        if exact_pair {
                            view.restore_restart_required = true;
                            view.restore_preview_busy = true;
                            view.is_restoring_prechecked = false;
                            view.is_replacement_confirmed = false;
                            view.restore_impact_preview = "恢复前请先预检".to_string();
                            view.set_status(
                                "恢复预检清理未能安全完成；Store 已锁定，必须重启 HiveGUI",
                                true,
                            );
                        }
                    }
                }
                cx.notify();
            });
            #[cfg(test)]
            if let Some(applied) = applied {
                let _ = applied.send(());
            }
        })
        .detach();
    }

    fn apply_restore_preview_outcome(
        &mut self,
        generation: u64,
        outcome: RestorePreviewOutcome,
        cx: &mut Context<Self>,
    ) {
        if generation != self.restore_preview_generation {
            match outcome {
                Ok((coordinator, prepared, _impact)) => {
                    self.cancel_exact_restore_preview(coordinator, Arc::new(prepared), None, cx);
                }
                Err(_error) => self.restore_preview_busy = false,
            }
            return;
        }

        self.restore_preview_busy = false;
        match outcome {
            Ok((coordinator, prepared, impact)) => {
                self.restore_coordinator = Some(coordinator);
                self.prepared_restore = Some(Arc::new(prepared));
                self.restore_impact_preview = impact;
                self.is_restoring_prechecked = true;
                self.is_replacement_confirmed = false;
                self.set_status("恢复预检通过", false);
            }
            Err(error) => {
                self.restore_coordinator = None;
                self.prepared_restore = None;
                self.restore_impact_preview = "恢复前请先预检".to_string();
                self.is_restoring_prechecked = false;
                self.set_status(format!("恢复预检失败：{error}"), true);
            }
        }
    }

    fn set_status(&mut self, msg: impl Into<SharedString>, is_error: bool) {
        self.status_message = Some(msg.into());
        self.is_error = is_error;
    }

    #[cfg(test)]
    fn install_restore_preview_delivery_interlock_for_test(
        &mut self,
    ) -> (
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Sender<()>,
    ) {
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        self.restore_preview_delivery_interlock = Some(RestorePreviewDeliveryInterlock {
            ready: ready_tx,
            release: release_rx,
        });
        (ready_rx, release_tx)
    }

    #[cfg(test)]
    fn install_restore_cancel_delivery_interlock_for_test(
        &mut self,
    ) -> (
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Sender<()>,
        tokio::sync::oneshot::Receiver<()>,
    ) {
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let (applied_tx, applied_rx) = tokio::sync::oneshot::channel();
        self.restore_cancel_delivery_interlock = Some(RestoreCancelDeliveryInterlock {
            ready: ready_tx,
            release: release_rx,
            applied: applied_tx,
        });
        (ready_rx, release_tx, applied_rx)
    }

    fn ensure_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.export_path_input.is_none() {
            let initial = self.export_path.to_string();
            self.export_path_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("/path/to/export.age.tar")
                    .default_value(&initial)
            }));
            if let Some(state) = &self.export_path_input {
                cx.subscribe_in(state, window, |this, state, event, _w, _cx| {
                    if let InputEvent::Change = event {
                        this.export_path = state.read(_cx).value().to_string().into();
                        this.clear_status();
                    }
                })
                .detach();
            }
        }

        if self.backup_passphrase_input.is_none() {
            let initial = self.backup_passphrase.to_string();
            self.backup_passphrase_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("备份口令")
                    .default_value(&initial)
            }));
            if let Some(state) = &self.backup_passphrase_input {
                cx.subscribe_in(state, window, |this, state, event, _w, _cx| {
                    if let InputEvent::Change = event {
                        this.backup_passphrase = state.read(_cx).value().to_string().into();
                        this.cancel_completed_restore_preview_after_input_change(_cx);
                        this.clear_status();
                    }
                })
                .detach();
            }
        }

        if self.import_path_input.is_none() {
            let initial = self.import_path.to_string();
            self.import_path_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("/path/to/backup.age.tar")
                    .default_value(&initial)
            }));
            if let Some(state) = &self.import_path_input {
                cx.subscribe_in(state, window, |this, state, event, _w, _cx| {
                    if let InputEvent::Change = event {
                        this.import_path = state.read(_cx).value().to_string().into();
                        this.cancel_completed_restore_preview_after_input_change(_cx);
                        this.clear_status();
                    }
                })
                .detach();
            }
        }

        if self.diagnostic_path_input.is_none() {
            let initial = self.diagnostic_path.to_string();
            self.diagnostic_path_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("/path/to/diagnostic.json")
                    .default_value(&initial)
            }));
            if let Some(state) = &self.diagnostic_path_input {
                cx.subscribe_in(state, window, |this, state, event, _w, _cx| {
                    if let InputEvent::Change = event {
                        this.diagnostic_path = state.read(_cx).value().to_string().into();
                        this.clear_status();
                    }
                })
                .detach();
            }
        }

        if self.retention_days_input.is_none() {
            let initial = self.retention_days.to_string();
            self.retention_days_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("365")
                    .default_value(&initial)
            }));
            if let Some(state) = &self.retention_days_input {
                cx.subscribe_in(state, window, |this, state, event, _w, _cx| {
                    if let InputEvent::Change = event {
                        this.retention_days = state.read(_cx).value().to_string().into();
                        this.clear_status();
                    }
                })
                .detach();
            }
        }

        if let Some(state) = &self.export_path_input {
            self.export_path = state.read(cx).value().to_string().into();
        }
        if let Some(state) = &self.import_path_input {
            self.import_path = state.read(cx).value().to_string().into();
        }
        if let Some(state) = &self.diagnostic_path_input {
            self.diagnostic_path = state.read(cx).value().to_string().into();
        }
        if let Some(state) = &self.backup_passphrase_input {
            self.backup_passphrase = state.read(cx).value().to_string().into();
        }
        if let Some(state) = &self.retention_days_input {
            self.retention_days = state.read(cx).value().to_string().into();
        }

        if !self.initial_focus_installed {
            self.initial_focus_installed = true;
            let focus = self.settings_focus.clone();
            cx.on_next_frame(window, move |_view, window, cx| {
                focus.focus(window, cx);
            });
        }

        if let Some(store) = self.store.as_ref()
            && !self.retention_loaded
        {
            self.retention_loaded = true;
            let store = store.clone();
            let entity = cx.entity();
            cx.spawn(async move |_this, cx| {
                let value = store
                    .get_global_config(RETENTION_KEY)
                    .await
                    .ok()
                    .flatten()
                    .and_then(|value| value.parse::<u64>().ok())
                    .unwrap_or(365);
                entity.update(cx, move |view, _| {
                    view.retention_days = value.to_string().into();
                    view.retention_loaded = true;
                });
            })
            .detach();
        }
    }

    fn save_retention(&mut self, cx: &mut Context<Self>) {
        let parsed = self.retention_days.to_string().trim().parse::<u64>();
        match parsed {
            Ok(days) if days > 0 => {
                self.set_status("正在保存保留期...", false);
                if let Some(store) = self.store.clone() {
                    let entity = cx.entity();
                    cx.spawn(async move |_this, cx| {
                        let outcome = store
                            .upsert_global_config(
                                "conversation retention days",
                                RETENTION_KEY,
                                "number",
                                &days.to_string(),
                            )
                            .await;
                        entity.update(cx, move |view, cx| {
                            match outcome {
                                Ok(_) => view.set_status(
                                    format!("会话保留期已持久化为 {days} 天（已应用）"),
                                    false,
                                ),
                                Err(err) => {
                                    view.set_status(format!("保存会话保留期失败：{err}"), true)
                                }
                            }
                            cx.notify();
                        });
                    })
                    .detach();
                } else {
                    self.set_status("Store 未就绪，无法保存会话保留期", true);
                    cx.notify();
                }
            }
            _ => {
                self.set_status("保留期必须是大于 0 的整数", true);
            }
        }
        cx.notify();
    }

    fn preview_retention(&mut self, cx: &mut Context<Self>) {
        let Some(store) = self.store.clone() else {
            self.set_status("Store 未就绪，无法预览清理影响", true);
            cx.notify();
            return;
        };
        let retention_days = self.retention_days.to_string().parse::<i64>().ok();
        let entity = cx.entity();
        cx.spawn(async move |_this, cx| {
            let outcome = match ConversationStore::from_store(&store, retention_days) {
                Ok(conversations) => conversations
                    .preview_retention("expired")
                    .await
                    .map(|preview| preview.affected_count()),
                Err(error) => Err(error),
            };
            entity.update(cx, move |view, cx| {
                match outcome {
                    Ok(count) => view.set_status(format!("预计清理过期会话 {count} 条"), false),
                    Err(error) => view.set_status(format!("预览会话清理影响失败：{error}"), true),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.key == "escape" {
            self.is_replacement_confirmed = false;
            self.restore_precheck_focus.focus(window, cx);
            cx.notify();
        }
    }

    fn preview_restore(&mut self, cx: &mut Context<Self>) {
        if self.restore_restart_required {
            cx.notify();
            return;
        }
        if self.restore_coordinator.is_some() || self.prepared_restore.is_some() {
            self.set_status("已有恢复预检等待处理；请确认恢复或修改恢复输入后再试", true);
            cx.notify();
            return;
        }
        if self.restore_preview_busy {
            self.set_status("恢复预检仍在处理中，请等待当前预检安全收口", true);
            cx.notify();
            return;
        }
        if self.store.is_none() {
            self.set_status("Store 未就绪", true);
            cx.notify();
            return;
        }
        if self.import_path.trim().is_empty() {
            self.set_status("请输入恢复文件路径", true);
            cx.notify();
            return;
        }
        if self.backup_passphrase.trim().is_empty() {
            self.set_status("恢复前请先输入备份口令", true);
            cx.notify();
            return;
        }
        if !Path::new(&self.import_path.to_string()).exists() {
            self.set_status("恢复文件不存在", true);
            cx.notify();
            return;
        }

        let store = self.store.as_ref().expect("store checked").clone();
        let path = self.import_path.to_string();
        let passphrase = self.backup_passphrase.to_string();
        let preview_generation = self.restore_preview_generation;
        self.restore_preview_busy = true;
        self.set_status("正在预检恢复内容...", false);
        cx.notify();

        let entity = cx.entity();
        let task = crate::ui::spawn_tokio(async move {
            let chat_sessions = query_scalar::<_, i64>("SELECT COUNT(*) FROM chat_sessions")
                .fetch_one(store.pool())
                .await
                .map_err(|error| error.to_string())?;
            let chat_messages = query_scalar::<_, i64>("SELECT COUNT(*) FROM chat_messages")
                .fetch_one(store.pool())
                .await
                .map_err(|error| error.to_string())?;
            let agent_executions = query_scalar::<_, i64>("SELECT COUNT(*) FROM agent_executions")
                .fetch_one(store.pool())
                .await
                .map_err(|error| error.to_string())?;
            let coordinator =
                RestoreCoordinator::from_store(store).map_err(|error| error.to_string())?;
            let prepared = coordinator
                .preview_restore(Path::new(&path), &passphrase)
                .await
                .map_err(|error| error.to_string())?;
            let impact = format!(
                "恢复预检通过：归档已完整认证并物化为隔离的未激活恢复实例 {}。\
完整替换影响：现有数据库与托管 Plugin 制品将被完整替换，不会合并 identifier、ID 或 updated_at；\
当前实例包含会话 {chat_sessions} 条、消息 {chat_messages} 条、执行 {agent_executions} 条。\
确认后将在 Store 冻结周期生成并验证安全备份，计划位置：{}。",
                prepared.db_instance_operation_id(),
                prepared.safety_backup_path().display(),
            );
            Ok::<_, String>((coordinator, prepared, impact))
        });

        #[cfg(test)]
        let delivery_interlock = self.restore_preview_delivery_interlock.take();
        cx.spawn(async move |_this, cx| {
            let outcome = task
                .await
                .unwrap_or_else(|error| Err(format!("恢复预检后台任务失败：{error}")));
            #[cfg(test)]
            if let Some(interlock) = delivery_interlock {
                let _ = interlock.ready.send(());
                let _ = interlock.release.await;
            }
            entity.update(cx, move |view, cx| {
                view.apply_restore_preview_outcome(preview_generation, outcome, cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn do_export(&mut self, cx: &mut Context<Self>) {
        if self.store.is_none() {
            self.set_status("Store 未就绪", true);
            cx.notify();
            return;
        }
        if self.backup_passphrase.trim().is_empty() {
            self.set_status("请先输入备份口令", true);
            cx.notify();
            return;
        }
        let Some(path) = (!self.export_path.is_empty()).then(|| self.export_path.to_string())
        else {
            self.set_status("请输入导出路径", true);
            cx.notify();
            return;
        };

        let store = self.store.as_ref().expect("store checked").clone();
        let passphrase = self.backup_passphrase.to_string();
        let entity = cx.entity();
        self.is_exporting = true;
        self.set_status("正在导出...", false);
        cx.notify();

        let task = crate::ui::spawn_tokio(async move {
            let outcome = match BackupCoordinator::from_store(store) {
                Ok(coordinator) => match coordinator.preview_export(Path::new(&path)).await {
                    Ok(preview) => coordinator
                        .confirm_export(preview, &passphrase)
                        .await
                        .map_err(|error| format!("{error}；Store 已冻结，请重启后继续写入")),
                    Err(error) => Err(error.to_string()),
                },
                Err(error) => Err(error.to_string()),
            };
            (path, outcome)
        });
        cx.spawn(async move |_this, cx| {
            let (path, outcome) = match task.await {
                Ok(outcome) => outcome,
                Err(error) => {
                    entity.update(cx, move |view, cx| {
                        view.is_exporting = false;
                        view.set_status(format!("导出后台任务失败：{error}"), true);
                        cx.notify();
                    });
                    return;
                }
            };
            entity.update(cx, move |view, cx| {
                view.is_exporting = false;
                match outcome {
                    Ok(_) => view.set_status(
                        format!(
                            "导出完成：{path}（加密备份已验证；Store 已冻结，请重启后继续写入）"
                        ),
                        false,
                    ),
                    Err(err) => view.set_status(format!("导出失败：{err}"), true),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn do_import(&mut self, cx: &mut Context<Self>) {
        if self.restore_restart_required {
            cx.notify();
            return;
        }
        if self.store.is_none() {
            self.set_status("Store 未就绪", true);
            cx.notify();
            return;
        }
        if self.backup_passphrase.trim().is_empty() {
            self.set_status("恢复前请先输入备份口令", true);
            cx.notify();
            return;
        }
        if !self.is_restoring_prechecked {
            self.set_status("请先执行恢复预检", true);
            cx.notify();
            return;
        }
        if !self.is_replacement_confirmed {
            self.set_status("请先确认完整替换", true);
            cx.notify();
            return;
        }
        if self.import_path.is_empty() {
            self.set_status("请输入恢复路径", true);
            cx.notify();
            return;
        }

        let (safety_path, confirmation) = {
            let Some(coordinator) = self.restore_coordinator.as_ref() else {
                self.set_status("恢复预检状态已失效，请重新执行预检", true);
                cx.notify();
                return;
            };
            let Some(prepared) = self.prepared_restore.as_ref() else {
                self.set_status("恢复预检状态已失效，请重新执行预检", true);
                cx.notify();
                return;
            };
            (
                prepared.safety_backup_path().to_path_buf(),
                coordinator.begin_confirmation(prepared.as_ref()),
            )
        };
        let confirmation = match confirmation {
            Ok(confirmation) => confirmation,
            Err(_error) => {
                let preview_is_cancelable = self
                    .restore_coordinator
                    .as_ref()
                    .zip(self.prepared_restore.as_ref())
                    .is_some_and(|(coordinator, prepared)| {
                        coordinator.preview_is_cancelable(prepared.as_ref())
                    });
                self.is_restoring_prechecked = false;
                self.is_replacement_confirmed = false;
                self.restore_impact_preview = "恢复前请先预检".to_string();
                self.is_importing = false;
                if preview_is_cancelable {
                    let coordinator = self
                        .restore_coordinator
                        .take()
                        .expect("cancelable preview retains its exact coordinator");
                    let prepared = self
                        .prepared_restore
                        .take()
                        .expect("cancelable preview retains its exact PreparedRestore");
                    self.set_status(
                        "恢复确认未完成；正在安全取消本次预检，完成后请重新预检",
                        true,
                    );
                    self.cancel_exact_restore_preview(
                        coordinator,
                        prepared,
                        Some("本次预检已取消，请重新预检"),
                        cx,
                    );
                } else {
                    if let Some(coordinator) = self.restore_coordinator.as_ref() {
                        let _ = coordinator.enter_preview_cancel_failure_terminal();
                    }
                    self.restore_restart_required = true;
                    self.restore_preview_busy = true;
                    self.set_status(
                        "恢复确认未能安全完成；Store 已进入保护状态，必须重启 HiveGUI 进行恢复校验",
                        true,
                    );
                }
                cx.notify();
                return;
            }
        };

        let entity = cx.entity();
        self.restore_restart_required = true;
        self.is_importing = true;
        self.set_status("正在恢复；Store 已冻结，完成后必须重启 HiveGUI", false);
        cx.notify();

        let task = crate::ui::spawn_tokio(confirmation.finish());
        cx.spawn(async move |_this, cx| {
            let outcome = task.await;
            entity.update(cx, move |view, cx| {
                view.is_importing = false;
                match outcome {
                    Ok(Ok(_recovery)) => {
                        view.set_status(
                            format!(
                                "恢复完成：安全备份已验证：{}；Store 已冻结，必须重启 HiveGUI 后继续",
                                safety_path.display()
                            ),
                            false,
                        );
                        view.is_restoring_prechecked = false;
                        view.is_replacement_confirmed = false;
                        view.prepared_restore = None;
                    }
                    Ok(Err(error)) => match error.safety_backup_state() {
                        RestoreSafetyBackupState::NotVerified => {
                            view.set_status(
                                "恢复未完成：安全备份尚未验证，尚未进入数据切换；Store 已冻结，必须重启 HiveGUI 进行恢复校验",
                                true,
                            );
                        }
                        RestoreSafetyBackupState::Verified => {
                            view.set_status(
                                format!(
                                    "恢复未完成：安全备份已验证：{}；数据切换或收口未完成；Store 已冻结，必须重启 HiveGUI 进行恢复校验",
                                    safety_path.display()
                                ),
                                true,
                            );
                        }
                        RestoreSafetyBackupState::Invalidated => {
                            view.set_status(
                                "恢复未完成：安全备份绑定已失效，安全备份状态及数据切换阶段无法确认；Store 已冻结，必须重启 HiveGUI 进行恢复校验",
                                true,
                            );
                        }
                    },
                    Err(_join_error) => {
                        view.set_status(
                            "恢复后台任务异常结束：安全备份状态及数据切换阶段无法确认；Store 已冻结，必须重启 HiveGUI 进行恢复校验",
                            true,
                        );
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn export_diagnostic_bundle(&mut self, cx: &mut Context<Self>) {
        let Some(collector) = self.collector.clone() else {
            self.set_status("诊断采集器未接入", true);
            cx.notify();
            return;
        };
        let Some(path) =
            (!self.diagnostic_path.is_empty()).then(|| self.diagnostic_path.to_string())
        else {
            self.set_status("请输入诊断导出路径", true);
            cx.notify();
            return;
        };

        let entity = cx.entity();
        self.is_exporting_diagnostic = true;
        self.set_status("正在导出脱敏诊断样例...", false);
        cx.notify();

        let task = crate::ui::spawn_tokio_blocking(move || {
            let bundle = DiagnosticBundle::new(collector);
            bundle.export_redacted(Path::new(&path), &RedactionConfig::default())
        });
        cx.spawn(async move |_this, cx| {
            let result = match task.await {
                Ok(outcome) => outcome,
                Err(error) => {
                    entity.update(cx, move |view, cx| {
                        view.is_exporting_diagnostic = false;
                        view.set_status(format!("诊断导出后台任务失败：{error}"), true);
                        cx.notify();
                    });
                    return;
                }
            };
            entity.update(cx, move |view, cx| {
                view.is_exporting_diagnostic = false;
                match result {
                    Ok(redacted) => {
                        view.set_status(
                            format!(
                                "诊断导出成功：{} (events={})",
                                redacted.path().display(),
                                redacted.event_count(),
                            ),
                            false,
                        );
                    }
                    Err(err) => view.set_status(format!("诊断导出失败：{err}"), true),
                }
                cx.notify();
            });
        })
        .detach();
    }
}

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_inputs(window, cx);

        let theme = cx.theme();
        let bg = theme.background;
        let fg = theme.foreground;
        let border = theme.border;
        let panel = theme.group_box;
        let panel_fg = theme.group_box_foreground;
        let muted = theme.muted_foreground;

        let status = if let Some(msg) = &self.status_message {
            let color = if self.is_error {
                theme.danger
            } else {
                theme.success
            };
            div()
                .text_size(px(12.0))
                .text_color(color)
                .child(msg.clone())
        } else {
            div()
        };

        let view = cx.weak_entity();
        let export_input = self.export_path_input.clone().expect("export path input");
        let import_input = self.import_path_input.clone().expect("import path input");
        let passphrase_input = self
            .backup_passphrase_input
            .clone()
            .expect("backup passphrase input");
        let diagnostic_input = self
            .diagnostic_path_input
            .clone()
            .expect("diagnostic path input");
        let retention_input = self
            .retention_days_input
            .clone()
            .expect("retention days input");

        div()
            .id("settings-view")
            .track_focus(&self.settings_focus)
            .tab_index(0)
            .key_context("HiveguiSettings")
            .on_action(cx.listener(
                |view, _: &SettingsEscape, window, cx| {
                    view.is_replacement_confirmed = false;
                    view.restore_precheck_focus.focus(window, cx);
                    cx.notify();
                },
            ))
            .capture_key_down(cx.listener(|view, event: &KeyDownEvent, window, cx| {
                view.on_key_down(event, window, cx);
            }))
            .size_full()
            .flex()
            .flex_col()
            .bg(bg)
            .text_color(fg)
            .child(
                div()
                    .h(px(48.0))
                    .flex()
                    .items_center()
                    .px(px(16.0))
                    .child(div().text_size(px(18.0)).font_weight(FontWeight::BOLD).child("数据管理")),
            )
            .child(
                div()
                    .id("settings-native-scroll")
                    .debug_selector(|| "SETTINGS_SCROLL".to_string())
                    .flex_1()
                    .min_h_0()
                    .pr(px(12.0))
                    .overflow_y_scroll()
                    .track_scroll(&self.content_scroll)
                    .vertical_scrollbar(&self.content_scroll)
                    .p(px(16.0))
                    .flex()
                    .flex_col()
                    .gap(px(12.0))
                    .children([
                        div()
                            .bg(panel)
                            .text_color(panel_fg)
                            .rounded(px(8.0))
                            .p(px(12.0))
                            .flex()
                            .flex_col()
                            .gap(px(8.0))
                            .child(div().text_size(px(16.0)).font_weight(FontWeight::SEMIBOLD).child("会话保留期"))
                            .child(div().text_size(px(12.0)).text_color(muted).child("按天保存会话/消息保留策略。变更会直接持久化，影响新建会话与恢复窗口。"))
                            .child(
                                div().flex().flex_row().gap(px(8.0)).items_center()
                                    .child(div().w(px(96.0)).child("保留天数"))
                                    .child(
                                        div()
                                            .debug_selector(|| "SETTINGS_RETENTION_DAYS".to_string())
                                            .child(Input::new(&retention_input).w(px(120.0))),
                                    )
                                    .child(btn(
                                        "settings-retention-save",
                                        "保存保留期",
                                        theme.success,
                                        theme.success_hover,
                                        theme.success_foreground,
                                        theme.success_foreground,
                                        {
                                            let value = view.clone();
                                            move |_, _, cx| {
                                                let _ = value
                                                    .update(cx, |view, cx| view.save_retention(cx));
                                            }
                                        },
                                    ))
                                    .child(btn(
                                        "SETTINGS_RETENTION_PREVIEW",
                                        "预览清理影响",
                                        theme.secondary,
                                        theme.secondary_hover,
                                        theme.foreground,
                                        theme.foreground,
                                        {
                                            let value = view.clone();
                                            move |_, _, cx| {
                                                let _ = value.update(cx, |view, cx| {
                                                    view.preview_retention(cx)
                                                });
                                            }
                                        },
                                    )),
                            )
                            .into_any_element(),
                        div()
                            .bg(panel)
                            .text_color(panel_fg)
                            .rounded(px(8.0))
                            .p(px(12.0))
                            .flex()
                            .flex_col()
                            .gap(px(8.0))
                            .child(div().text_size(px(16.0)).font_weight(FontWeight::SEMIBOLD).child("备份口令"))
                            .child(div().text_size(px(12.0)).text_color(muted).child("导出与恢复使用同一口令，建议至少 12 个字符。"))
                            .child(div().flex().flex_row().gap(px(8.0)).items_center()
                                .child(div().w(px(96.0)).child("口令"))
                                .child(Input::new(&passphrase_input).w(px(420.0))))
                            .into_any_element(),
                        div()
                            .bg(panel)
                            .text_color(panel_fg)
                            .rounded(px(8.0))
                            .p(px(12.0))
                            .flex()
                            .flex_col()
                            .gap(px(8.0))
                            .child(div().text_size(px(16.0)).font_weight(FontWeight::SEMIBOLD).child("数据导出"))
                            .child(div().text_size(px(12.0)).text_color(muted).child("将数据库导出为加密 age 备份文件（导出文件不存在才可写入）。"))
                            .child(
                                div().flex().flex_row().gap(px(8.0)).items_center()
                                    .child(div().w(px(96.0)).child("导出路径"))
                                    .child(Input::new(&export_input).w(px(420.0))),
                            )
                            .child(
                                btn(
                                    "SETTINGS_BACKUP_EXPORT",
                                    if self.is_exporting { "导出中..." } else { "导出数据" },
                                    theme.primary,
                                    theme.primary_hover,
                                    theme.primary_foreground,
                                    theme.primary_foreground,
                                    {
                                        let view = view.clone();
                                        move |_, _, cx| {
                                                let _ = view.update(cx, |view, cx| view.do_export(cx));
                                        }
                                    },
                                ),
                            )
                            .into_any_element(),
                        div()
                            .bg(panel)
                            .text_color(panel_fg)
                            .rounded(px(8.0))
                            .p(px(12.0))
                            .flex()
                            .flex_col()
                            .gap(px(8.0))
                            .child(div().text_size(px(16.0)).font_weight(FontWeight::SEMIBOLD).child("恢复预检与完整替换"))
                            .child(div().text_size(px(12.0)).text_color(muted).child(format!("影响说明：{}", self.restore_impact_preview)))
                            .child(
                                div().flex().flex_row().gap(px(8.0)).items_center()
                                    .child(div().w(px(96.0)).child("恢复路径"))
                                    .child(Input::new(&import_input).w(px(420.0))),
                            )
                            .child(
                                div().flex().flex_row().gap(px(8.0)).items_center()
                                    .child(btn(
                                        "SETTINGS_RESTORE_PRECHECK",
                                        if self.is_restoring_prechecked { "已预检" } else { "预检恢复" },
                                        theme.warning,
                                        theme.warning_hover,
                                        theme.warning_foreground,
                                        theme.warning_foreground,
                                        {
                                            let view = view.clone();
                                            move |_, _, cx| {
                                                let _ = view.update(cx, |view, cx| view.preview_restore(cx));
                                            }
                                        },
                                    )
                                    .track_focus(&self.restore_precheck_focus)
                                    .child(focus_marker(
                                        "SETTINGS_RESTORE_PRECHECK_FOCUSED",
                                    )))
                                    .child(btn(
                                        "SETTINGS_RESTORE_CONFIRM",
                                        if self.is_replacement_confirmed {
                                            "取消确认"
                                        } else {
                                            "确认完整替换"
                                        },
                                        theme.danger,
                                        theme.danger_hover,
                                        theme.danger_foreground,
                                        theme.danger_foreground,
                                        {
                                            let view = view.clone();
                                            move |_, _, cx| {
                                                let _ = view.update(cx, |view, _| {
                                                    view.is_replacement_confirmed = !view.is_replacement_confirmed;
                                                });
                                            }
                                        },
                                    ))
                                    .child(btn(
                                        "settings-restore-execute",
                                        if self.is_importing { "恢复中..." } else { "执行恢复" },
                                        theme.primary,
                                        theme.primary_hover,
                                        theme.primary_foreground,
                                        theme.primary_foreground,
                                        {
                                            let view = view.clone();
                                            move |_, _, cx| {
                                                let _ = view.update(cx, |view, cx| view.do_import(cx));
                                            }
                                        },
                                    )),
                            )
                            .into_any_element(),
                        div()
                            .bg(panel)
                            .text_color(panel_fg)
                            .rounded(px(8.0))
                            .p(px(12.0))
                            .flex()
                            .flex_col()
                            .gap(px(8.0))
                            .child(div().text_size(px(16.0)).font_weight(FontWeight::SEMIBOLD).child("脱敏诊断导出"))
                            .child(div().text_size(px(12.0)).text_color(muted).child("按 execution_id 聚合事件并导出脱敏样例。"))
                            .child(
                                div().flex().flex_row().gap(px(8.0)).items_center()
                                    .child(div().w(px(96.0)).child("导出路径"))
                                    .child(Input::new(&diagnostic_input).w(px(420.0))),
                            )
                            .child(btn(
                                "SETTINGS_DIAGNOSTIC_EXPORT",
                                if self.is_exporting_diagnostic {
                                    "导出中..."
                                } else {
                                    "导出诊断样例"
                                },
                                theme.secondary,
                                theme.secondary_hover,
                                theme.foreground,
                                theme.foreground,
                                {
                                    let view = view.clone();
                                    move |_, _, cx| {
                                        let _ = view.update(cx, |view, cx| view.export_diagnostic_bundle(cx));
                                    }
                                },
                            ))
                            .into_any_element(),
                        status.into_any_element(),
                        div()
                            .text_size(px(11.0))
                            .text_color(muted)
                            .child("提示：恢复操作将覆盖现有数据，执行前请先导出当前数据。")
                            .child(div().mt(px(4.0)).text_color(border).child("路由：会话管理入口由 AI 管理 - 数据管理页签承载。"))
                            .into_any_element(),
                    ]),
            )
    }
}

fn btn(
    selector: &'static str,
    label: &'static str,
    bg: Hsla,
    hover: Hsla,
    fg: Hsla,
    hover_fg: Hsla,
    handler: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let debug_selector = selector.to_string();
    div()
        .id(selector)
        .debug_selector(move || debug_selector.clone())
        .tab_index(0)
        .role(Role::Button)
        .aria_label(label)
        .px(px(12.0))
        .py(px(8.0))
        .bg(bg)
        .rounded(px(6.0))
        .text_size(px(12.0))
        .text_color(fg)
        .cursor(CursorStyle::PointingHand)
        .hover(|s| s.bg(hover).text_color(hover_fg))
        .on_mouse_down(MouseButton::Left, handler)
        .child(label)
}

fn focus_marker(selector: impl Into<SharedString>) -> Stateful<Div> {
    let selector = selector.into();
    let debug_selector = selector.clone();
    div()
        .id(selector)
        .debug_selector(move || debug_selector.to_string())
        .w(px(0.0))
        .h(px(0.0))
        .overflow_hidden()
}

#[cfg(test)]
mod tests {
    use std::{
        env, fs,
        path::Path,
        process::{Command, Output},
        sync::Arc,
        time::Duration,
    };

    use gpui::{
        Context, IntoElement, Render, TestAppContext, VisualTestContext, Window, div, prelude::*,
        px, size,
    };
    use gpui_component::input::InputEvent;
    use sha2::{Digest as _, Sha256};

    use super::SettingsView;
    use crate::datasource::backup::BackupExporter;
    use crate::datasource::{Store, StoreOpenErrorKind, StoreOpenOptions};

    const SETTINGS_RESTORE_LOCK_CHILD_MODE: &str = "HIVEGUI_SETTINGS_RESTORE_LOCK_CHILD_MODE";
    const SETTINGS_RESTORE_LOCK_CHILD_ROOT: &str = "HIVEGUI_SETTINGS_RESTORE_LOCK_CHILD_ROOT";
    const RESTORE_PRE_SAFETY_FAILURE_STATUS: &str = "恢复未完成：安全备份尚未验证，尚未进入数据切换；Store 已冻结，必须重启 HiveGUI 进行恢复校验";
    const RESTORE_INVALIDATED_SAFETY_FAILURE_STATUS: &str = "恢复未完成：安全备份绑定已失效，安全备份状态及数据切换阶段无法确认；Store 已冻结，必须重启 HiveGUI 进行恢复校验";
    const RESTORE_JOIN_FAILURE_STATUS: &str = "恢复后台任务异常结束：安全备份状态及数据切换阶段无法确认；Store 已冻结，必须重启 HiveGUI 进行恢复校验";

    #[derive(Debug, PartialEq, Eq)]
    struct RestoreStoreBindingSnapshot {
        database_path: Option<std::path::PathBuf>,
        pool_closed: Option<bool>,
        retention_loaded: bool,
        prechecked: bool,
        replacement_confirmed: bool,
        coordinator_present: bool,
        prepared_present: bool,
        restart_required: bool,
        preview_generation: u64,
        preview_busy: bool,
        importing: bool,
        impact: String,
        status: Option<String>,
        is_error: bool,
    }

    fn restore_store_binding_snapshot(view: &SettingsView) -> RestoreStoreBindingSnapshot {
        RestoreStoreBindingSnapshot {
            database_path: view
                .store
                .as_ref()
                .map(|store| store.database_path().to_path_buf()),
            pool_closed: view.store.as_ref().map(|store| store.pool().is_closed()),
            retention_loaded: view.retention_loaded,
            prechecked: view.is_restoring_prechecked,
            replacement_confirmed: view.is_replacement_confirmed,
            coordinator_present: view.restore_coordinator.is_some(),
            prepared_present: view.prepared_restore.is_some(),
            restart_required: view.restore_restart_required,
            preview_generation: view.restore_preview_generation,
            preview_busy: view.restore_preview_busy,
            importing: view.is_importing,
            impact: view.restore_impact_preview.clone(),
            status: view.status_message.as_ref().map(ToString::to_string),
            is_error: view.is_error,
        }
    }

    fn data_source_names(runtime: &tokio::runtime::Runtime, database: &Path) -> Vec<String> {
        runtime.block_on(async {
            let options = sqlx::sqlite::SqliteConnectOptions::new()
                .filename(database)
                .create_if_missing(false)
                .read_only(true)
                .immutable(true)
                .foreign_keys(true);
            let pool = sqlx::sqlite::SqlitePoolOptions::new()
                .min_connections(1)
                .max_connections(1)
                .connect_with(options)
                .await
                .expect("open raw Settings restore database");
            let names =
                sqlx::query_scalar::<_, String>("SELECT name FROM data_sources ORDER BY name")
                    .fetch_all(&pool)
                    .await
                    .expect("read raw Settings restore DataSource names");
            pool.close().await;
            names
        })
    }

    fn data_source_names_with_sidecars(
        runtime: &tokio::runtime::Runtime,
        database: &Path,
    ) -> Vec<String> {
        runtime.block_on(async {
            let options = sqlx::sqlite::SqliteConnectOptions::new()
                .filename(database)
                .create_if_missing(false)
                .read_only(true)
                .foreign_keys(true);
            let pool = sqlx::sqlite::SqlitePoolOptions::new()
                .min_connections(1)
                .max_connections(1)
                .connect_with(options)
                .await
                .expect("open raw Settings database with committed sidecars");
            let names =
                sqlx::query_scalar::<_, String>("SELECT name FROM data_sources ORDER BY name")
                    .fetch_all(&pool)
                    .await
                    .expect("read Settings DataSource names with committed sidecars");
            pool.close().await;
            names
        })
    }

    fn export_settings_data_source_archive(
        runtime: &tokio::runtime::Runtime,
        source_root: &Path,
        archive: &Path,
        data_source_name: &str,
        passphrase: &str,
    ) {
        let source = runtime
            .block_on(Store::open_local(StoreOpenOptions::for_root(source_root)))
            .expect("open Settings lifecycle archive source Store");
        runtime
            .block_on(source.create(
                data_source_name,
                "127.0.0.1",
                3306,
                "archive-user",
                b"archive-password",
            ))
            .expect("seed Settings lifecycle archive state");
        runtime.block_on(source.pool().close());
        drop(source);
        runtime
            .block_on(
                BackupExporter::new(source_root.join("datasources.db"))
                    .export_age(archive, passphrase),
            )
            .expect("export Settings lifecycle archive");
    }

    fn settings_restore_live_instances(root: &Path) -> Vec<std::path::PathBuf> {
        let registry = root.join(".hivegui-db-staging-v1");
        match fs::read_dir(registry) {
            Ok(entries) => entries
                .map(|entry| entry.expect("read Settings restore registry entry").path())
                .collect(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => panic!("read Settings restore registry: {error}"),
        }
    }

    fn settings_restore_sha256(path: &Path) -> String {
        hex::encode(Sha256::digest(
            fs::read(path).expect("read Settings restore digest input"),
        ))
    }

    fn source_item_body<'a>(normalized: &'a str, signature: &str) -> &'a str {
        let start = normalized
            .find(signature)
            .unwrap_or_else(|| panic!("missing production source item: {signature}"));
        let body_start = normalized[start..]
            .find('{')
            .map(|offset| start + offset)
            .unwrap_or_else(|| panic!("missing body for production source item: {signature}"));
        let mut depth = 0_usize;
        for (offset, character) in normalized[body_start..].char_indices() {
            match character {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return &normalized[start..body_start + offset + character.len_utf8()];
                    }
                }
                _ => {}
            }
        }
        panic!("unterminated production source item: {signature}");
    }

    fn source_braced_body_at<'a>(normalized: &'a str, marker: &str) -> &'a str {
        let start = normalized
            .find(marker)
            .unwrap_or_else(|| panic!("missing production source marker: {marker}"));
        let body_start = normalized[start..]
            .find('{')
            .map(|offset| start + offset)
            .unwrap_or_else(|| panic!("missing body for production source marker: {marker}"));
        let mut depth = 0_usize;
        for (offset, character) in normalized[body_start..].char_indices() {
            match character {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return &normalized[start..body_start + offset + character.len_utf8()];
                    }
                }
                _ => {}
            }
        }
        panic!("unterminated production source marker: {marker}");
    }

    fn assert_restore_confirmation_safety_phase_source_contract() {
        let backup_source = include_str!("../datasource/backup.rs");
        let backup_normalized = backup_source
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let backup_enum_contract = backup_source
            .lines()
            .filter(|line| {
                let line = line.trim_start();
                !line.starts_with("///") && !line.starts_with("#[")
            })
            .collect::<Vec<_>>()
            .join("\n")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let state_start = backup_normalized
            .find("pub enum RestoreSafetyBackupState")
            .expect("public RestoreSafetyBackupState enum");
        let state_derive_start = backup_normalized[..state_start]
            .rfind("#[derive(")
            .expect("RestoreSafetyBackupState derive");
        let state_derive = &backup_normalized[state_derive_start..state_start];
        let derive_end = state_derive
            .find(")]")
            .map(|offset| offset + 2)
            .expect("complete RestoreSafetyBackupState derive");
        assert!(
            state_derive[..derive_end].contains("Copy")
                && state_derive[derive_end..].trim().is_empty(),
            "the adjacent RestoreSafetyBackupState derive must include Copy"
        );
        let state = source_item_body(&backup_enum_contract, "pub enum RestoreSafetyBackupState");
        assert_eq!(
            state, "pub enum RestoreSafetyBackupState { NotVerified, Verified, Invalidated, }",
            "the public Copy phase must expose exactly the three approved states"
        );
        let raw_state = source_item_body(backup_source, "pub enum RestoreSafetyBackupState");
        for variant in ["NotVerified", "Verified", "Invalidated"] {
            let lines = raw_state.lines().collect::<Vec<_>>();
            let variant_line = lines
                .iter()
                .position(|line| line.trim() == format!("{variant},"))
                .unwrap_or_else(|| panic!("missing exact safety state line {variant}"));
            let preceding_line = lines[..variant_line]
                .iter()
                .rev()
                .find(|line| !line.trim().is_empty())
                .copied()
                .unwrap_or_default()
                .trim_start();
            assert!(
                preceding_line.starts_with("///"),
                "public safety state {variant} requires its own rustdoc"
            );
        }
        assert!(
            backup_normalized.contains(
                "#[error(\"restore_confirmation_failed\")] pub struct RestoreConfirmationError"
            ),
            "RestoreConfirmationError Display must be fixed and must not interpolate its internal cause"
        );
        let confirmation_error =
            source_item_body(&backup_normalized, "pub struct RestoreConfirmationError");
        assert!(
            confirmation_error.contains("safety_backup_state: RestoreSafetyBackupState")
                && confirmation_error.contains("cause: ImportError")
                && !confirmation_error.contains("pub safety_backup_state")
                && !confirmation_error.contains("pub cause"),
            "RestoreConfirmationError must privately retain both its stored phase and internal cause"
        );
        let confirmation_error_impl =
            source_item_body(&backup_normalized, "impl RestoreConfirmationError");
        assert!(
            confirmation_error_impl
                .contains("pub fn safety_backup_state(&self) -> RestoreSafetyBackupState"),
            "RestoreConfirmationError must expose the public Copy phase accessor"
        );
        let phase_accessor = source_item_body(
            &backup_normalized,
            "pub fn safety_backup_state(&self) -> RestoreSafetyBackupState",
        );
        assert_eq!(
            phase_accessor,
            "pub fn safety_backup_state(&self) -> RestoreSafetyBackupState { self.safety_backup_state }",
            "the public phase accessor must return only its stored field without inspecting the cause"
        );
        let error_constructor = source_item_body(
            &backup_normalized,
            "fn new(safety_backup_state: RestoreSafetyBackupState, cause: ImportError) -> Self",
        );
        assert_eq!(
            error_constructor,
            "fn new(safety_backup_state: RestoreSafetyBackupState, cause: ImportError) -> Self { Self { safety_backup_state, cause, } }",
            "the private constructor must store the caller-proven phase and cause without classifying either"
        );
        assert!(
            !backup_normalized.contains("impl From<ImportError> for RestoreConfirmationError"),
            "a generic From<ImportError> would guess a phase without the finish state machine"
        );
        let confirmation_impl = source_item_body(&backup_normalized, "impl RestoreConfirmation {");
        assert_eq!(
            confirmation_impl
                .matches(
                    "pub async fn finish(self) -> Result<RestoreRecovery, RestoreConfirmationError>",
                )
                .count(),
            1,
            "the production RestoreConfirmation::finish boundary must return the structured error"
        );

        let verified_proof =
            source_item_body(&backup_normalized, "struct VerifiedRestoreSafetySnapshot");
        assert_eq!(
            verified_proof,
            "struct VerifiedRestoreSafetySnapshot { binding: RestoreSafetySnapshotBinding, evidence: ControlledTreeEvidence, }",
            "the proof token must own only its held binding and descriptor-derived evidence"
        );
        let verified_proof_new = source_item_body(
            &backup_normalized,
            "fn new(binding: RestoreSafetySnapshotBinding, relative_path: &Path) -> Result<Self, ImportError>",
        );
        let proof_new_first_binding = verified_proof_new
            .find("verify_snapshot_directory_binding(&binding)?;")
            .expect("proof constructor initial binding check");
        let proof_new_evidence = verified_proof_new
            .find(
                "let evidence = controlled_tree_evidence_at(binding.directory(), relative_path)?;",
            )
            .expect("proof constructor held-dirfd evidence");
        let proof_new_final_binding = verified_proof_new
            .rfind("verify_snapshot_directory_binding(&binding)?;")
            .expect("proof constructor final binding check");
        assert!(
            proof_new_first_binding < proof_new_evidence
                && proof_new_evidence < proof_new_final_binding
                && verified_proof_new.contains("Ok(Self { binding, evidence, })")
                && !verified_proof_new.contains("PathBuf")
                && !verified_proof_new.contains("canonical_path"),
            "proof construction must bind, derive evidence through the held dirfd, bind again, and never downgrade to a path"
        );
        assert_eq!(
            verified_proof_new
                .matches("verify_snapshot_directory_binding(&binding)?;")
                .count(),
            2
        );
        let proof_revalidate = source_item_body(
            &backup_normalized,
            "fn revalidate(&self) -> Result<(), ImportError>",
        );
        let revalidate_first_binding = proof_revalidate
            .find("verify_snapshot_directory_binding(&self.binding)?;")
            .expect("return-time initial binding check");
        let revalidate_evidence = proof_revalidate
            .find("let fresh_evidence = controlled_tree_evidence_at(self.binding.directory(),")
            .expect("return-time held-dirfd evidence recheck");
        let revalidate_final_binding = proof_revalidate
            .rfind("verify_snapshot_directory_binding(&self.binding)?;")
            .expect("return-time final binding check");
        assert!(
            revalidate_first_binding < revalidate_evidence
                && revalidate_evidence < revalidate_final_binding
                && proof_revalidate[revalidate_evidence..revalidate_final_binding].contains(")?;")
                && proof_revalidate.contains("fresh_evidence != self.evidence")
                && !proof_revalidate.contains("canonical_path")
                && !proof_revalidate.contains("PathBuf"),
            "return-time validation must repeat binding-evidence-binding and compare the stored evidence"
        );
        assert_eq!(
            proof_revalidate
                .matches("verify_snapshot_directory_binding(&self.binding)?;")
                .count(),
            2
        );
        let evidence_mismatch =
            source_braced_body_at(proof_revalidate, "if fresh_evidence != self.evidence");
        assert_eq!(
            evidence_mismatch.matches("return Err(").count(),
            1,
            "descriptor evidence mismatch must take the unique error return"
        );
        assert!(
            !evidence_mismatch.contains("Ok(")
                && proof_revalidate[revalidate_final_binding..]
                    .matches("Ok(())")
                    .count()
                    == 1,
            "only a matched fresh evidence plus final binding may return Ok"
        );

        let finish_internal = source_item_body(&backup_normalized, "async fn finish_internal(");
        let verified_snapshot = finish_internal
            .find("verify_restore_safety_snapshot(&snapshot_binding)")
            .expect("finish must fully verify the safety snapshot");
        let current_file_match = finish_internal
            .rfind("verify_current_database_for_snapshot(")
            .expect("finish must recheck the bound current database");
        let current_absent_match = finish_internal
            .rfind("verify_current_database_absent()")
            .expect("finish must recheck an absent current database");
        let final_current_match = current_file_match.max(current_absent_match);
        let cleanup_handoff = finish_internal
            .find("finish_or_cleanup_restore_safety_snapshot(")
            .expect("finish must complete the safety cleanup handoff");
        let final_snapshot_binding = finish_internal
            .rfind("verify_snapshot_directory_binding(&snapshot_binding)")
            .expect("finish must perform the final canonical snapshot-directory binding check");
        let proof_creation = finish_internal
            .find("let verified_safety_snapshot = VerifiedRestoreSafetySnapshot::new(")
            .expect("finish must convert the exact held binding into its proof token");
        let not_verified_mapping = finish_internal
            .find("RestoreConfirmationError::new( RestoreSafetyBackupState::NotVerified,")
            .or_else(|| {
                finish_internal
                    .find("RestoreConfirmationError::new(RestoreSafetyBackupState::NotVerified,")
            })
            .expect("all pre-proof errors must map to NotVerified");
        let apply_outcome = finish_internal
            .find("let apply_outcome =")
            .expect("finish must save the post-safety apply outcome");
        let apply = finish_internal[apply_outcome..]
            .find(".apply_restore_after_safety(")
            .map(|offset| apply_outcome + offset)
            .expect("finish must enter the existing post-safety apply path");
        let apply_statement_end = finish_internal[apply_outcome..]
            .find(';')
            .map(|offset| apply_outcome + offset + 1)
            .expect("complete apply_outcome statement");
        let apply_statement = &finish_internal[apply_outcome..apply_statement_end];
        assert!(
            apply_statement.starts_with("let apply_outcome =")
                && apply_statement.contains(".apply_restore_after_safety(")
                && apply_statement.contains("&verified_safety_snapshot")
                && apply_statement.ends_with(".await;")
                && !apply_statement.contains("Ok(")
                && !apply_statement.contains("Err("),
            "apply_outcome RHS must be exactly the awaited real apply call: {apply_statement}"
        );
        assert_eq!(
            finish_internal
                .matches(".apply_restore_after_safety(")
                .count(),
            1,
            "finish may execute only the one apply call saved in apply_outcome"
        );
        let pre_apply = &finish_internal[..apply_outcome];
        assert_eq!(
            pre_apply
                .matches("RestoreSafetyBackupState::NotVerified")
                .count(),
            1,
            "every error before the verified proof enters apply must share one NotVerified mapper"
        );
        assert!(
            !pre_apply.contains("RestoreSafetyBackupState::Verified")
                && !pre_apply.contains("RestoreSafetyBackupState::Invalidated"),
            "Verified and Invalidated may not be selected before post-safety apply"
        );
        let return_revalidation = finish_internal[apply..]
            .find("let safety_binding = verified_safety_snapshot.revalidate();")
            .map(|offset| apply + offset)
            .expect("finish must save held-proof revalidation after every apply outcome");
        let outcome_classification = finish_internal[return_revalidation..]
            .find("classify_restore_confirmation_outcome(apply_outcome, safety_binding)")
            .or_else(|| {
                finish_internal[return_revalidation..]
                    .find("classify_restore_confirmation_outcome( apply_outcome, safety_binding, )")
            })
            .map(|offset| return_revalidation + offset)
            .expect(
                "finish must pass the exact saved apply and binding outcomes to classification",
            );
        assert!(
            verified_snapshot < final_current_match
                && final_current_match < cleanup_handoff
                && cleanup_handoff < final_snapshot_binding
                && final_snapshot_binding < proof_creation
                && proof_creation <= not_verified_mapping
                && not_verified_mapping < apply_outcome
                && apply_outcome < apply
                && apply < return_revalidation
                && return_revalidation < outcome_classification,
            "the proof must follow every safety check; apply must be saved, then the same proof unconditionally revalidated before classification"
        );
        assert!(
            finish_internal[apply..return_revalidation].contains("&verified_safety_snapshot")
                && !finish_internal[apply..return_revalidation].contains(".await?"),
            "apply must borrow the held proof and cannot early-return before revalidation"
        );
        for forbidden in [
            "ImportError::",
            ".to_string(",
            ".contains(",
            ".exists(",
            "fs::metadata(",
            "fs::symlink_metadata(",
            "match error",
            "match cause",
        ] {
            assert!(
                !finish_internal.contains(forbidden),
                "finish must derive phase from control flow, never an ImportError variant/string or a path probe: {forbidden}"
            );
        }

        let outcome_classifier = source_item_body(
            &backup_normalized,
            "fn classify_restore_confirmation_outcome(",
        );
        for (label, alternatives) in [
            (
                "success plus valid proof returns success",
                &[
                    "(Ok(recovery), Ok(())) => Ok(recovery)",
                    "(Ok(recovery), Ok(())) => { Ok(recovery) }",
                ][..],
            ),
            (
                "apply error plus valid proof returns Verified",
                &[
                    "(Err(cause), Ok(())) => Err(RestoreConfirmationError::new( RestoreSafetyBackupState::Verified, cause, ))",
                    "(Err(cause), Ok(())) => Err(RestoreConfirmationError::new(RestoreSafetyBackupState::Verified, cause))",
                ][..],
            ),
            (
                "success plus invalid proof returns Invalidated",
                &[
                    "(Ok(_), Err(binding_cause)) => Err(RestoreConfirmationError::new( RestoreSafetyBackupState::Invalidated, binding_cause, ))",
                    "(Ok(_), Err(binding_cause)) => Err(RestoreConfirmationError::new(RestoreSafetyBackupState::Invalidated, binding_cause))",
                ][..],
            ),
            (
                "apply error plus invalid proof returns Invalidated",
                &[
                    "(Err(_), Err(binding_cause)) => Err(RestoreConfirmationError::new( RestoreSafetyBackupState::Invalidated, binding_cause, ))",
                    "(Err(_), Err(binding_cause)) => Err(RestoreConfirmationError::new(RestoreSafetyBackupState::Invalidated, binding_cause))",
                ][..],
            ),
        ] {
            assert!(
                alternatives
                    .iter()
                    .any(|pattern| outcome_classifier.contains(pattern)),
                "the pure outcome mapper is missing {label}: {outcome_classifier}"
            );
        }
        assert!(
            outcome_classifier.contains("match (apply_outcome, safety_binding)")
                && !outcome_classifier.contains("RestoreSafetyBackupState::NotVerified")
                && !outcome_classifier.contains("ImportError::")
                && !outcome_classifier.contains(".to_string(")
                && !outcome_classifier.contains(".contains(")
                && !outcome_classifier.contains(".exists(")
                && !outcome_classifier.contains("Path::"),
            "post-apply classification may use only apply outcome and held-proof validity"
        );

        let pre_safety =
            source_item_body(&backup_normalized, "async fn validate_restore_database(");
        for exact_invalid_manifest_mapping in [
            ".connect_with(options) .await .map_err(|_| ImportError::InvalidManifest(\"open restore database\".into()))?;",
            "verify_sqlite_health(&pool) .await .map_err(|_| ImportError::InvalidManifest(\"restore SQLite health\".into()))?;",
            "verify_schema(&pool) .await .map_err(|_| ImportError::InvalidManifest(\"restore schema drift\".into()))?;",
        ] {
            assert!(
                pre_safety.contains(exact_invalid_manifest_mapping),
                "the non-SQLite fixture must stay in InvalidManifest: {exact_invalid_manifest_mapping}"
            );
        }
        let post_safety =
            source_item_body(&backup_normalized, "async fn apply_restore_after_safety(");
        assert!(
            post_safety.contains(
                "return Err(ImportError::InvalidManifest( \"restore manifest cannot be armed\".into(), ));"
            ) || post_safety.contains(
                "return Err(ImportError::InvalidManifest(\"restore manifest cannot be armed\".into()));"
            ),
            "the armed-manifest post-safety fixture must use the same InvalidManifest category"
        );
        assert!(
            post_safety.contains("verified_safety_snapshot: &VerifiedRestoreSafetySnapshot")
                && !post_safety.contains("safety_snapshot: PathBuf")
                && post_safety
                    .contains("safety_snapshot: verified_safety_snapshot.evidence.clone()"),
            "apply and owner publication must borrow the proof and clone its held evidence"
        );
        assert_eq!(
            post_safety.matches("verified_safety_snapshot").count(),
            2,
            "apply may use the proof only in its parameter and exact owner evidence field"
        );
        for forbidden_safety_reopen in [
            "verified_safety_snapshot.binding.canonical_path",
            "controlled_tree_evidence(&safety_snapshot",
            "controlled_tree_evidence( &safety_snapshot",
            "File::open(&safety_snapshot",
            "fs::read(&safety_snapshot",
        ] {
            assert!(
                !post_safety.contains(forbidden_safety_reopen),
                "apply must not reopen the verified safety tree through a Path: {forbidden_safety_reopen}"
            );
        }

        let settings_source = include_str!("settings_view.rs");
        let settings_normalized = settings_source
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let do_import = source_item_body(
            &settings_normalized,
            "fn do_import(&mut self, cx: &mut Context<Self>)",
        );
        assert_eq!(
            do_import.matches("error.safety_backup_state()").count(),
            1,
            "Settings must classify the confirmation failure exactly once through its structured phase accessor"
        );
        assert!(
            do_import.contains("RestoreSafetyBackupState::NotVerified")
                && do_import.contains("RestoreSafetyBackupState::Verified")
                && do_import.contains("RestoreSafetyBackupState::Invalidated"),
            "Settings must explicitly render all three structured safety phases"
        );
        let failure_start = do_import
            .find("Ok(Err(error)) =>")
            .expect("structured RestoreConfirmationError outcome arm");
        let join_start = do_import[failure_start..]
            .find("Err(_join_error) =>")
            .map(|offset| failure_start + offset)
            .expect("separate conservative JoinError arm");
        let structured_failure_arm = source_braced_body_at(do_import, "Ok(Err(error)) =>");
        assert_eq!(
            structured_failure_arm.matches("error").count(),
            2,
            "the structured error may occur only in its binding and safety_backup_state accessor; string/debug classification is forbidden: {structured_failure_arm}"
        );
        for forbidden in ["error.to_string", "error.contains", "{error", "error:?}"] {
            assert!(
                !structured_failure_arm.contains(forbidden),
                "Settings must not classify or expose RestoreConfirmationError text: {forbidden}"
            );
        }
        for forbidden_probe in [
            "safety_path.exists(",
            "safety_path.try_exists(",
            ".is_file(",
            ".is_dir(",
            "fs::read(",
            "fs::read_dir(",
            "fs::metadata(",
            "fs::symlink_metadata(",
            "File::open(",
            ".canonicalize(",
            "prepared_restore",
            "restore_coordinator",
        ] {
            assert!(
                !structured_failure_arm.contains(forbidden_probe),
                "Settings must not infer the structured phase by probing safety artifacts: {forbidden_probe}"
            );
        }
        assert_eq!(
            structured_failure_arm.matches("safety_path").count(),
            1,
            "the exact safety path may appear only once, as Verified display data"
        );
        assert!(
            structured_failure_arm.contains("safety_path.display()"),
            "only Verified may display the exact safety path"
        );
        let not_verified_arm = source_braced_body_at(
            structured_failure_arm,
            "RestoreSafetyBackupState::NotVerified =>",
        );
        let verified_arm = source_braced_body_at(
            structured_failure_arm,
            "RestoreSafetyBackupState::Verified =>",
        );
        let invalidated_arm = source_braced_body_at(
            structured_failure_arm,
            "RestoreSafetyBackupState::Invalidated =>",
        );
        assert!(
            not_verified_arm.contains(RESTORE_PRE_SAFETY_FAILURE_STATUS)
                && !not_verified_arm.contains("safety_path"),
            "NotVerified must use only its exact no-switch status"
        );
        assert!(
            verified_arm.contains("恢复未完成：安全备份已验证：{}；数据切换或收口未完成；Store 已冻结，必须重启 HiveGUI 进行恢复校验")
                && verified_arm.contains("safety_path.display()"),
            "Verified must use only the exact displayed-path status"
        );
        assert!(
            invalidated_arm.contains(RESTORE_INVALIDATED_SAFETY_FAILURE_STATUS)
                && !invalidated_arm.contains("safety_path"),
            "Invalidated must use only its exact conservative status"
        );
        assert_eq!(
            invalidated_arm.matches("view.set_status(").count(),
            1,
            "Invalidated must execute exactly one status update"
        );
        assert_eq!(
            invalidated_arm
                .matches(RESTORE_INVALIDATED_SAFETY_FAILURE_STATUS)
                .count(),
            1,
            "Invalidated must execute its fixed text exactly once"
        );
        assert!(
            invalidated_arm.matches('"').count() == 2
                && !invalidated_arm.contains("format!(")
                && invalidated_arm.contains("true"),
            "Invalidated may contain no dead-code, formatted, or secondary status text"
        );
        let invalidated_exact = format!(
            "RestoreSafetyBackupState::Invalidated => {{ view.set_status( \"{}\", true, ); }}",
            RESTORE_INVALIDATED_SAFETY_FAILURE_STATUS
        );
        assert_eq!(
            invalidated_arm, invalidated_exact,
            "Invalidated arm must directly return its one fixed status update"
        );
        let join_arm = source_braced_body_at(&do_import[join_start..], "Err(_join_error) =>");
        assert_eq!(
            join_arm.matches("_join_error").count(),
            1,
            "JoinError may appear only in its ignored binding"
        );
        assert!(
            join_arm.contains(RESTORE_JOIN_FAILURE_STATUS) && join_arm.contains("true"),
            "JoinError must use the exact conservative status as an error"
        );
        assert_eq!(
            join_arm.matches("view.set_status(").count(),
            1,
            "JoinError must execute exactly one status update"
        );
        assert_eq!(
            join_arm.matches(RESTORE_JOIN_FAILURE_STATUS).count(),
            1,
            "JoinError must execute its fixed text exactly once"
        );
        assert!(
            join_arm.matches('"').count() == 2 && !join_arm.contains("format!("),
            "JoinError may contain no dead-code, formatted, or secondary status text"
        );
        let join_exact = format!(
            "Err(_join_error) => {{ view.set_status( \"{}\", true, ); }}",
            RESTORE_JOIN_FAILURE_STATUS
        );
        assert_eq!(
            join_arm, join_exact,
            "JoinError arm must directly return its one fixed status update"
        );
    }

    fn settings_restore_failure_fixture(
        cx: &mut TestAppContext,
        runtime: &tokio::runtime::Runtime,
        source_root: &Path,
        target_root: &Path,
        archive: &Path,
        restored_name: &str,
        current_name: &str,
        passphrase: &str,
    ) -> (Store, gpui::Entity<SettingsView>) {
        export_settings_data_source_archive(
            runtime,
            source_root,
            archive,
            restored_name,
            passphrase,
        );
        let store = runtime
            .block_on(Store::open_local(StoreOpenOptions::for_root(target_root)))
            .expect("open structured safety-phase production Store");
        runtime
            .block_on(store.create(
                current_name,
                "127.0.0.1",
                3307,
                "current-user",
                b"current-password",
            ))
            .expect("seed structured safety-phase current state");
        let live_store = store.clone();
        let view = cx.new(|cx| {
            let mut view = SettingsView::new(cx, None);
            view.set_store(store);
            view.import_path = archive.to_string_lossy().to_string().into();
            view.backup_passphrase = passphrase.to_string().into();
            view
        });
        (live_store, view)
    }

    fn wait_for_settings_restore_ready(
        cx: &mut TestAppContext,
        runtime: &tokio::runtime::Runtime,
        view: &gpui::Entity<SettingsView>,
    ) {
        cx.dispatcher.allow_parking();
        {
            let runtime_guard = runtime.enter();
            cx.update(|cx| view.update(cx, |view, cx| view.preview_restore(cx)));
            drop(runtime_guard);
        }
        let mut ready = false;
        let mut status = String::new();
        for _ in 0..3_000 {
            runtime.block_on(async { tokio::time::sleep(Duration::from_millis(10)).await });
            cx.run_until_parked();
            let (is_ready, is_error, current_status) = cx.update(|cx| {
                let view = view.read(cx);
                (
                    view.is_restoring_prechecked && !view.restore_preview_busy,
                    view.is_error,
                    view.status_message
                        .as_ref()
                        .map(ToString::to_string)
                        .unwrap_or_default(),
                )
            });
            ready = is_ready;
            status = current_status;
            if is_ready || is_error {
                break;
            }
        }
        assert!(ready, "structured safety-phase preview failed: {status}");
    }

    fn run_settings_restore_confirmation(
        cx: &mut TestAppContext,
        runtime: &tokio::runtime::Runtime,
        view: &gpui::Entity<SettingsView>,
        live_store: &Store,
    ) -> String {
        {
            let runtime_guard = runtime.enter();
            cx.update(|cx| {
                view.update(cx, |view, cx| {
                    view.is_replacement_confirmed = true;
                    view.do_import(cx);
                });
            });
            drop(runtime_guard);
        }
        let (restart_required, importing, pool_closed) = cx.update(|cx| {
            let view = view.read(cx);
            (
                view.restore_restart_required,
                view.is_importing,
                view.store
                    .as_ref()
                    .expect("structured safety-phase Store remains bound")
                    .pool()
                    .is_closed(),
            )
        });
        assert!(restart_required && importing);
        assert!(
            pool_closed && live_store.pool().is_closed(),
            "do_import must synchronously close the exact Store before asynchronous safety work"
        );

        let mut settled = false;
        for _ in 0..3_000 {
            runtime.block_on(async { tokio::time::sleep(Duration::from_millis(10)).await });
            cx.run_until_parked();
            if !cx.update(|cx| view.read(cx).is_importing) {
                settled = true;
                break;
            }
        }
        assert!(
            settled,
            "structured safety-phase confirmation did not settle"
        );
        cx.update(|cx| {
            view.read(cx)
                .status_message
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_default()
        })
    }

    fn run_settings_restore_lock_child(root: &Path) -> Output {
        Command::new(env::current_exe().expect("resolve Settings unit-test binary"))
            .arg("--exact")
            .arg("ui::settings_view::tests::child_settings_restore_lock_probe")
            .arg("--nocapture")
            .env(SETTINGS_RESTORE_LOCK_CHILD_MODE, "expect-locked")
            .env(SETTINGS_RESTORE_LOCK_CHILD_ROOT, root)
            .output()
            .expect("run independent Settings Store-lock child")
    }

    fn assert_settings_restore_lock_child(output: &Output, context: &str) {
        assert!(
            output.status.success(),
            "{context}\nchild stdout:\n{}\nchild stderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    #[test]
    fn child_settings_restore_lock_probe() {
        let Some(mode) = env::var_os(SETTINGS_RESTORE_LOCK_CHILD_MODE) else {
            return;
        };
        assert_eq!(mode, "expect-locked");
        let root = env::var_os(SETTINGS_RESTORE_LOCK_CHILD_ROOT)
            .map(std::path::PathBuf::from)
            .expect("Settings lock-probe root is configured");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("create Settings child Tokio runtime");
        let result = runtime.block_on(Store::open_local(StoreOpenOptions::for_root(root)));
        let error = match result {
            Ok(store) => {
                drop(store);
                panic!("child opened a Store whose production restore lock must remain held")
            }
            Err(error) => error,
        };
        assert_eq!(error.kind(), StoreOpenErrorKind::AlreadyLocked);
    }

    struct SettingsLayoutTestView;

    impl Render for SettingsLayoutTestView {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .debug_selector(|| "SETTINGS_CONTENT".to_owned())
        }
    }

    #[gpui::test]
    fn settings_content_is_constrained_to_the_available_height(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::theme::init(cx);
            gpui_component::init(cx);
        });
        let window = cx.open_window(size(px(800.0), px(520.0)), |_, _| SettingsLayoutTestView);
        cx.run_until_parked();

        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let bounds = cx
            .debug_bounds("SETTINGS_CONTENT")
            .expect("settings content exists");
        assert_eq!(bounds.top(), px(0.0));
        assert_eq!(bounds.bottom(), px(520.0));
    }

    #[gpui::test]
    fn production_export_freezes_the_live_store_and_reports_restart_requirement(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_component::theme::init(cx);
            gpui_component::init(cx);
        });
        let temp_dir = tempfile::tempdir().expect("create Settings backup fixture");
        let runtime = tokio::runtime::Runtime::new().expect("create Tokio runtime");
        let store = runtime
            .block_on(Store::new(temp_dir.path()))
            .expect("create canonical Settings Store");
        runtime
            .block_on(store.create(
                "last-legal-write",
                "127.0.0.1",
                3306,
                "settings-test",
                b"settings-production-boundary",
            ))
            .expect("persist the final legal write before confirmation");
        let store_after_export = store.clone();
        let export_path = temp_dir.path().join("settings-production.age");
        let view = cx.new(|cx| {
            let mut view = SettingsView::new(cx, None);
            view.set_store(store);
            view.export_path = export_path.to_string_lossy().to_string().into();
            view.backup_passphrase = "T119-settings-production-passphrase".into();
            view
        });

        let runtime_guard = runtime.enter();
        cx.dispatcher.allow_parking();
        cx.update(|cx| {
            view.update(cx, |view, cx| view.do_export(cx));
        });
        let mut exporting = true;
        for _ in 0..300 {
            cx.run_until_parked();
            exporting = cx.update(|cx| view.read(cx).is_exporting);
            if !exporting {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        drop(runtime_guard);

        assert!(!exporting, "timed out waiting for the production export");
        assert!(
            export_path.is_file(),
            "production export must publish the archive"
        );
        let blocked = runtime
            .block_on(store_after_export.create(
                "after-confirmation",
                "127.0.0.1",
                3307,
                "settings-test",
                b"must-not-write",
            ))
            .expect_err("production export must leave the confirmed Store frozen");
        assert!(blocked.to_string().contains("write_gate_closed"));
        let status = cx.update(|cx| {
            view.read(cx)
                .status_message
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_default()
        });
        assert!(
            status.contains("Store 已冻结") && status.contains("重启后继续写入"),
            "successful production export must explain the frozen Store lifecycle: {status}"
        );
    }

    #[gpui::test]
    fn production_restore_preview_and_confirmation_use_the_store_bound_coordinator(
        cx: &mut TestAppContext,
    ) {
        const PASSPHRASE: &str = "T130-settings-store-bound-restore-passphrase";

        cx.update(|cx| {
            gpui_component::theme::init(cx);
            gpui_component::init(cx);
        });
        let temp_dir = tempfile::tempdir().expect("create Settings restore fixture");
        let source_root = temp_dir.path().join("archive-source");
        let target_root = temp_dir.path().join("production-target");
        let archive = temp_dir.path().join("settings-restore.age.tar");
        // A current-thread runtime cannot poll a spawned Tokio task until this
        // test explicitly calls `block_on`. This makes the handler-return
        // boundary below observable without a worker racing ahead of it.
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("create deterministic Settings Tokio runtime");

        let source = runtime
            .block_on(Store::open_local(StoreOpenOptions::for_root(&source_root)))
            .expect("open owner-aware archive source Store");
        runtime
            .block_on(source.create(
                "settings-restored-new",
                "127.0.0.1",
                3306,
                "archive-user",
                b"archive-password",
            ))
            .expect("seed new archive state");
        runtime.block_on(source.pool().close());
        drop(source);
        runtime
            .block_on(
                BackupExporter::new(source_root.join("datasources.db"))
                    .export_age(&archive, PASSPHRASE),
            )
            .expect("export Settings restore fixture");

        let store = runtime
            .block_on(Store::open_local(StoreOpenOptions::for_root(&target_root)))
            .expect("open owner-aware production target Store");
        runtime
            .block_on(store.create(
                "settings-old-current",
                "127.0.0.1",
                3307,
                "old-user",
                b"old-password",
            ))
            .expect("seed old production state");
        let live_store = store.clone();
        let view = cx.new(|cx| {
            let mut view = SettingsView::new(cx, None);
            view.set_store(store);
            view.import_path = archive.to_string_lossy().to_string().into();
            view.backup_passphrase = PASSPHRASE.into();
            view
        });

        cx.dispatcher.allow_parking();
        {
            let preview_runtime_guard = runtime.enter();
            cx.update(|cx| {
                view.update(cx, |view, cx| view.preview_restore(cx));
            });
            drop(preview_runtime_guard);
        }
        let mut preview_complete = false;
        let mut preview_status = String::new();
        for _ in 0..3_000 {
            runtime.block_on(async { tokio::time::sleep(Duration::from_millis(10)).await });
            cx.run_until_parked();
            let (complete, is_error, status) = cx.update(|cx| {
                let view = view.read(cx);
                (
                    view.is_restoring_prechecked,
                    view.is_error,
                    view.status_message
                        .as_ref()
                        .map(ToString::to_string)
                        .unwrap_or_default(),
                )
            });
            preview_complete = complete;
            preview_status = status;
            if complete || is_error {
                break;
            }
        }
        assert!(
            preview_complete,
            "production restore preview did not complete successfully: {preview_status}"
        );

        let registry = target_root.join(".hivegui-db-staging-v1");
        assert!(
            registry.is_dir(),
            "production Settings preview_restore must materialize and retain one authenticated unarmed restore instance"
        );
        let live_instances = fs::read_dir(&registry)
            .expect("read Settings restore registry")
            .map(|entry| entry.expect("read restore registry entry").path())
            .collect::<Vec<_>>();
        assert_eq!(
            live_instances.len(),
            1,
            "one successful UI preview must retain exactly one prepared restore"
        );
        let live_instance = &live_instances[0];
        let live_basename = live_instance
            .file_name()
            .and_then(|name| name.to_str())
            .expect("restore instance has an ASCII basename");
        let operation_id = live_basename
            .strip_prefix("restore-")
            .expect("preview retains a restore-{UUID} instance");
        let safety_snapshot = target_root
            .join("backups")
            .join(format!("restore-safety-{operation_id}"));
        let instance_manifest: serde_json::Value = serde_json::from_slice(
            &fs::read(live_instance.join(".hivegui-db-instance-v1.json"))
                .expect("read UI preview instance manifest"),
        )
        .expect("parse UI preview instance manifest");
        assert_eq!(instance_manifest["ownership_state"], "unarmed");
        assert_eq!(instance_manifest["role"], "restore");
        assert_eq!(instance_manifest["db_instance_operation_id"], operation_id);
        assert!(
            !live_instance.join(".hivegui-db-recovery-v1.json").exists(),
            "preview must not publish an armed restore owner"
        );
        assert!(
            !safety_snapshot.exists(),
            "preview may display only the exact future safety snapshot path"
        );
        assert_eq!(
            data_source_names(&runtime, &live_instance.join("datasources.db")),
            vec!["settings-restored-new"]
        );
        let impact = cx.update(|cx| view.read(cx).restore_impact_preview.clone());
        let exact_safety_path = safety_snapshot.to_string_lossy();
        for required in [
            "现有数据库",
            "托管 Plugin",
            "完整替换",
            "不会合并",
            "安全备份",
            exact_safety_path.as_ref(),
        ] {
            assert!(
                impact.contains(required),
                "restore preview must explain the complete replacement and exact future safety path; missing {required:?}: {impact}"
            );
        }
        assert!(
            !live_store.pool().is_closed(),
            "preview must leave the production Store writable"
        );
        runtime
            .block_on(live_store.create(
                "settings-last-legal",
                "127.0.0.1",
                3308,
                "last-user",
                b"last-password",
            ))
            .expect("preview must permit the final legal write before confirmation");
        assert!(
            live_instance.is_dir(),
            "the UI must retain its prepared restore across the final legal write"
        );
        let held_connection = runtime
            .block_on(live_store.pool().acquire())
            .expect("hold one connection across the confirmation boundary");

        // Entering a current-thread runtime installs the spawn handle but does
        // not poll its tasks. Therefore no asynchronous confirmation can race
        // this production handler or the immediate post-return observation.
        {
            let confirm_runtime_guard = runtime.enter();
            cx.update(|cx| {
                view.update(cx, |view, cx| {
                    view.is_replacement_confirmed = true;
                    view.do_import(cx);
                });
            });
            drop(confirm_runtime_guard);
        }
        let (is_importing, pool_closed) = cx.update(|cx| {
            let view = view.read(cx);
            (
                view.is_importing,
                view.store
                    .as_ref()
                    .expect("Settings production Store remains bound")
                    .pool()
                    .is_closed(),
            )
        });
        assert!(is_importing, "confirmation must start asynchronous finish");
        assert!(
            pool_closed,
            "do_import must synchronously freeze and close the shared Store pool before returning"
        );
        let blocked = runtime
            .block_on(live_store.create(
                "settings-must-not-write-after-confirmation",
                "127.0.0.1",
                3309,
                "blocked-user",
                b"blocked-password",
            ))
            .expect_err("confirmation must synchronously close the production write gate");
        assert!(blocked.to_string().contains("write_gate_closed"));
        for pump_round in 1..=5 {
            runtime.block_on(async { tokio::time::sleep(Duration::from_millis(20)).await });
            cx.run_until_parked();
            assert!(
                cx.update(|cx| view.read(cx).is_importing),
                "asynchronous finish escaped the held connection drain during pump round {pump_round}"
            );
            assert!(
                live_store.pool().is_closed(),
                "the shared Pool reopened while confirmation was draining"
            );
            assert!(
                !safety_snapshot.exists(),
                "the safety snapshot was published before the held connection drained during pump round {pump_round}"
            );
            assert!(
                live_instance.is_dir(),
                "the prepared live instance disappeared before the held connection drained during pump round {pump_round}"
            );
            let draining_manifest: serde_json::Value = serde_json::from_slice(
                &fs::read(live_instance.join(".hivegui-db-instance-v1.json"))
                    .expect("read the draining restore instance manifest"),
            )
            .expect("parse the draining restore instance manifest");
            assert_eq!(
                draining_manifest["ownership_state"], "unarmed",
                "the restore instance armed before the held connection drained during pump round {pump_round}"
            );
            assert!(
                !live_instance.join(".hivegui-db-recovery-v1.json").exists(),
                "the recovery owner was published before the held connection drained during pump round {pump_round}"
            );
        }
        {
            let connection_drop_runtime_guard = runtime.enter();
            drop(held_connection);
            drop(connection_drop_runtime_guard);
        }

        let mut importing = true;
        for _ in 0..1_000 {
            runtime.block_on(async { tokio::time::sleep(Duration::from_millis(10)).await });
            cx.run_until_parked();
            importing = cx.update(|cx| view.read(cx).is_importing);
            if !importing {
                break;
            }
        }
        assert!(!importing, "timed out waiting for UI restore finish");
        assert!(live_store.pool().is_closed());
        let terminal_write = runtime
            .block_on(live_store.create(
                "settings-must-not-write-after-finish",
                "127.0.0.1",
                3310,
                "blocked-user",
                b"blocked-password",
            ))
            .expect_err("successful in-process restore stays frozen until restart recovery");
        assert!(terminal_write.to_string().contains("write_gate_closed"));
        assert_eq!(
            data_source_names(&runtime, &target_root.join("datasources.db")),
            vec!["settings-restored-new"]
        );
        let terminal_lock = run_settings_restore_lock_child(&target_root);
        assert_settings_restore_lock_child(
            &terminal_lock,
            "after a recognizable new current is published, Settings must retain the OS Store lock until restart recovery",
        );
        assert!(safety_snapshot.join("manifest.json").is_file());
        assert_eq!(
            data_source_names(&runtime, &safety_snapshot.join("datasources.db")),
            vec!["settings-last-legal", "settings-old-current"]
        );
        assert_eq!(
            fs::read_dir(&registry)
                .expect("read retired Settings restore registry")
                .count(),
            0,
            "successful restore must retire the prepared live instance"
        );
        let status = cx.update(|cx| {
            view.read(cx)
                .status_message
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_default()
        });
        assert!(
            status.contains("安全备份已验证")
                && status.contains(exact_safety_path.as_ref())
                && status.contains("Store 已冻结")
                && status.contains("重启"),
            "successful restore must report the verified safety path and restart-required frozen lifecycle: {status}"
        );
    }

    #[gpui::test]
    fn changing_restore_input_cancels_the_exact_preview_before_a_second_preview(
        cx: &mut TestAppContext,
    ) {
        const PASSPHRASE: &str = "T130-settings-preview-lifecycle-passphrase";

        cx.update(|cx| {
            gpui_component::theme::init(cx);
            gpui_component::init(cx);
        });
        let temp_dir = tempfile::tempdir().expect("create Settings lifecycle fixture");
        let source_a = temp_dir.path().join("archive-source-a");
        let source_b = temp_dir.path().join("archive-source-b");
        let target_root = temp_dir.path().join("production-target");
        let archive_a = temp_dir.path().join("settings-preview-a.age.tar");
        let archive_b = temp_dir.path().join("settings-preview-b.age.tar");
        let runtime = tokio::runtime::Runtime::new().expect("create lifecycle Tokio runtime");
        export_settings_data_source_archive(
            &runtime,
            &source_a,
            &archive_a,
            "settings-preview-a",
            PASSPHRASE,
        );
        export_settings_data_source_archive(
            &runtime,
            &source_b,
            &archive_b,
            "settings-preview-b",
            PASSPHRASE,
        );

        let store = runtime
            .block_on(Store::open_local(StoreOpenOptions::for_root(&target_root)))
            .expect("open lifecycle production Store");
        runtime
            .block_on(store.create(
                "settings-lifecycle-current",
                "127.0.0.1",
                3307,
                "current-user",
                b"current-password",
            ))
            .expect("seed lifecycle current state");
        let live_store = store.clone();
        let archive_a_value = archive_a.to_string_lossy().into_owned();
        let archive_b_value = archive_b.to_string_lossy().into_owned();

        cx.dispatcher.allow_parking();
        let first_preview_runtime_guard = runtime.enter();
        let window = cx.open_window(size(px(900.0), px(720.0)), move |_, cx| {
            let mut view = SettingsView::new(cx, None);
            view.set_store(store);
            view.import_path = archive_a_value.into();
            view.backup_passphrase = PASSPHRASE.into();
            view
        });
        cx.run_until_parked();
        window
            .update(cx, |view, _, cx| view.preview_restore(cx))
            .expect("start first production preview");
        let mut first_preview_complete = false;
        let mut first_preview_status = String::new();
        for _ in 0..3_000 {
            cx.run_until_parked();
            let (complete, is_error, status) = window
                .update(cx, |view, _, _| {
                    (
                        view.is_restoring_prechecked,
                        view.is_error,
                        view.status_message
                            .as_ref()
                            .map(ToString::to_string)
                            .unwrap_or_default(),
                    )
                })
                .expect("read first preview state");
            first_preview_complete = complete;
            first_preview_status = status;
            if complete || is_error {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            first_preview_complete,
            "first production preview did not complete: {first_preview_status}"
        );
        drop(first_preview_runtime_guard);
        let first_instances = settings_restore_live_instances(&target_root);
        assert_eq!(first_instances.len(), 1);
        let first_live = first_instances[0].clone();
        assert_eq!(
            data_source_names(&runtime, &first_live.join("datasources.db")),
            vec!["settings-preview-a"]
        );
        let first_manifest: serde_json::Value = serde_json::from_slice(
            &fs::read(first_live.join(".hivegui-db-instance-v1.json"))
                .expect("read first preview manifest"),
        )
        .expect("parse first preview manifest");
        assert_eq!(first_manifest["ownership_state"], "unarmed");
        let first_operation_id = first_manifest["db_instance_operation_id"]
            .as_str()
            .expect("first preview operation id");
        let first_safety = target_root
            .join("backups")
            .join(format!("restore-safety-{first_operation_id}"));
        assert!(!first_safety.exists());

        let change_runtime_guard = runtime.enter();
        window
            .update(cx, |view, window, cx| {
                view.is_replacement_confirmed = true;
                let input = view
                    .import_path_input
                    .clone()
                    .expect("render created the production restore-path InputState");
                input.update(cx, |input, cx| {
                    input.set_value(archive_b_value.clone(), window, cx);
                    cx.emit(InputEvent::Change);
                });
            })
            .expect("change the real production restore-path input");
        cx.run_until_parked();
        let (prechecked, replacement_confirmed, selected_path) = window
            .update(cx, |view, _, _| {
                (
                    view.is_restoring_prechecked,
                    view.is_replacement_confirmed,
                    view.import_path.to_string(),
                )
            })
            .expect("read post-Change production state");
        assert!(!prechecked, "InputState Change must clear precheck state");
        assert!(
            !replacement_confirmed,
            "InputState Change must clear replacement confirmation"
        );
        assert_eq!(selected_path, archive_b_value);

        let mut prepared_cleared = false;
        let mut remaining_instances = first_instances;
        for _ in 0..500 {
            cx.run_until_parked();
            prepared_cleared = window
                .update(cx, |view, _, _| {
                    view.prepared_restore.is_none() && !view.restore_preview_busy
                })
                .expect("read cancelled PreparedRestore state");
            remaining_instances = settings_restore_live_instances(&target_root);
            if prepared_cleared && remaining_instances.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            prepared_cleared && remaining_instances.is_empty(),
            "a real restore-path InputState Change must cancel the exact PreparedRestore and retire its unarmed registry instance; prepared_cleared={prepared_cleared}, remaining={remaining_instances:?}"
        );
        assert!(
            !first_safety.exists(),
            "canceling a preview must not create its planned safety snapshot"
        );

        for stale_result_round in 1..=5 {
            std::thread::sleep(Duration::from_millis(20));
            cx.run_until_parked();
            let (no_prepared, still_prechecked, still_confirmed) = window
                .update(cx, |view, _, _| {
                    (
                        view.prepared_restore.is_none(),
                        view.is_restoring_prechecked,
                        view.is_replacement_confirmed,
                    )
                })
                .expect("read stable cancelled preview state");
            assert!(
                no_prepared
                    && !still_prechecked
                    && !still_confirmed
                    && settings_restore_live_instances(&target_root).is_empty(),
                "an old asynchronous preview result reinstalled cancelled state during round {stale_result_round}"
            );
        }
        drop(change_runtime_guard);
        assert!(!live_store.pool().is_closed());
        runtime
            .block_on(live_store.create(
                "settings-write-after-preview-cancel",
                "127.0.0.1",
                3308,
                "after-cancel-user",
                b"after-cancel-password",
            ))
            .expect("canceling a preview must leave current Store writable");

        let second_preview_runtime_guard = runtime.enter();
        window
            .update(cx, |view, _, cx| view.preview_restore(cx))
            .expect("start second production preview");
        let mut second_preview_complete = false;
        let mut second_preview_status = String::new();
        for _ in 0..3_000 {
            cx.run_until_parked();
            let (complete, is_error, status) = window
                .update(cx, |view, _, _| {
                    (
                        view.is_restoring_prechecked,
                        view.is_error,
                        view.status_message
                            .as_ref()
                            .map(ToString::to_string)
                            .unwrap_or_default(),
                    )
                })
                .expect("read second preview state");
            second_preview_complete = complete;
            second_preview_status = status;
            if complete || is_error {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            second_preview_complete,
            "second production preview did not complete: {second_preview_status}"
        );
        for _ in 0..5 {
            std::thread::sleep(Duration::from_millis(20));
            cx.run_until_parked();
        }
        let (second_prechecked, second_confirmed) = window
            .update(cx, |view, _, _| {
                (view.is_restoring_prechecked, view.is_replacement_confirmed)
            })
            .expect("read stable second preview state");
        assert!(second_prechecked);
        assert!(!second_confirmed);
        let second_instances = settings_restore_live_instances(&target_root);
        assert_eq!(
            second_instances.len(),
            1,
            "a second preview must retain only its one new unarmed instance"
        );
        let second_live = &second_instances[0];
        assert_ne!(second_live, &first_live);
        let second_manifest: serde_json::Value = serde_json::from_slice(
            &fs::read(second_live.join(".hivegui-db-instance-v1.json"))
                .expect("read second preview manifest"),
        )
        .expect("parse second preview manifest");
        assert_eq!(second_manifest["ownership_state"], "unarmed");
        assert!(
            !second_live.join(".hivegui-db-recovery-v1.json").exists(),
            "the second preview must remain owner-free"
        );
        let second_operation_id = second_manifest["db_instance_operation_id"]
            .as_str()
            .expect("second preview operation id");
        let second_safety = target_root
            .join("backups")
            .join(format!("restore-safety-{second_operation_id}"));
        assert!(!first_safety.exists());
        assert!(!second_safety.exists());
        drop(second_preview_runtime_guard);
        assert_eq!(
            data_source_names(&runtime, &second_live.join("datasources.db")),
            vec!["settings-preview-b"]
        );

        let changed_passphrase = "T130-settings-preview-lifecycle-changed-passphrase";
        let passphrase_change_runtime_guard = runtime.enter();
        window
            .update(cx, |view, window, cx| {
                view.is_replacement_confirmed = true;
                let input = view
                    .backup_passphrase_input
                    .clone()
                    .expect("render created the production backup-passphrase InputState");
                input.update(cx, |input, cx| {
                    input.set_value(changed_passphrase, window, cx);
                    cx.emit(InputEvent::Change);
                });
            })
            .expect("change the real production backup-passphrase input");
        cx.run_until_parked();
        let (prechecked, replacement_confirmed, selected_passphrase) = window
            .update(cx, |view, _, _| {
                (
                    view.is_restoring_prechecked,
                    view.is_replacement_confirmed,
                    view.backup_passphrase.to_string(),
                )
            })
            .expect("read post-passphrase-Change production state");
        assert!(
            !prechecked,
            "backup-passphrase InputState Change must clear precheck state"
        );
        assert!(
            !replacement_confirmed,
            "backup-passphrase InputState Change must clear replacement confirmation"
        );
        assert_eq!(selected_passphrase, changed_passphrase);

        let mut second_prepared_cleared = false;
        let mut second_remaining_instances = second_instances;
        for _ in 0..500 {
            cx.run_until_parked();
            second_prepared_cleared = window
                .update(cx, |view, _, _| {
                    view.prepared_restore.is_none() && !view.restore_preview_busy
                })
                .expect("read passphrase-cancelled PreparedRestore state");
            second_remaining_instances = settings_restore_live_instances(&target_root);
            if second_prepared_cleared && second_remaining_instances.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            second_prepared_cleared && second_remaining_instances.is_empty(),
            "a real backup-passphrase InputState Change must cancel the exact B PreparedRestore and retire its unarmed registry instance; prepared_cleared={second_prepared_cleared}, remaining={second_remaining_instances:?}"
        );
        assert!(
            !second_safety.exists(),
            "canceling B through a passphrase Change must not create its planned safety snapshot"
        );
        drop(passphrase_change_runtime_guard);
        assert!(!live_store.pool().is_closed());
        runtime
            .block_on(live_store.create(
                "settings-write-after-passphrase-cancel",
                "127.0.0.1",
                3309,
                "after-passphrase-user",
                b"after-passphrase-password",
            ))
            .expect("passphrase cancellation must leave current Store writable");
    }

    #[gpui::test]
    fn restore_preview_cancel_failure_enters_restart_only_terminal_state(cx: &mut TestAppContext) {
        const PASSPHRASE: &str = "T130-settings-cancel-failure-passphrase-secret";
        const STAGING_CANARY: &[u8] = b"T130-settings-cancel-failure-staging-canary-secret";

        cx.update(|cx| {
            gpui_component::theme::init(cx);
            gpui_component::init(cx);
        });
        let temp_dir = tempfile::tempdir().expect("create Settings cancel-failure fixture");
        let source_root = temp_dir.path().join("cancel-failure-source");
        let target_root = temp_dir.path().join("cancel-failure-production-target");
        let archive = temp_dir
            .path()
            .join("cancel-failure-archive-secret.age.tar");
        let changed_archive = temp_dir
            .path()
            .join("cancel-failure-next-archive-secret.age.tar");
        let runtime = tokio::runtime::Runtime::new().expect("create cancel-failure Tokio runtime");
        export_settings_data_source_archive(
            &runtime,
            &source_root,
            &archive,
            "settings-cancel-failure-archive",
            PASSPHRASE,
        );

        let store = runtime
            .block_on(Store::open_local(StoreOpenOptions::for_root(&target_root)))
            .expect("open cancel-failure production Store");
        runtime
            .block_on(store.create(
                "settings-cancel-failure-current",
                "127.0.0.1",
                3307,
                "current-user",
                b"current-password",
            ))
            .expect("seed cancel-failure current state");
        let live_store = store.clone();
        let archive_value = archive.to_string_lossy().into_owned();
        let changed_archive_value = changed_archive.to_string_lossy().into_owned();

        cx.dispatcher.allow_parking();
        let preview_runtime_guard = runtime.enter();
        let window = cx.open_window(size(px(900.0), px(720.0)), move |_, cx| {
            let mut view = SettingsView::new(cx, None);
            view.set_store(store);
            view.import_path = archive_value.into();
            view.backup_passphrase = PASSPHRASE.into();
            view
        });
        cx.run_until_parked();
        window
            .update(cx, |view, _, cx| view.preview_restore(cx))
            .expect("start cancel-failure production preview");
        let mut preview_complete = false;
        let mut preview_status = String::new();
        for _ in 0..3_000 {
            cx.run_until_parked();
            let (complete, is_error, status) = window
                .update(cx, |view, _, _| {
                    (
                        view.is_restoring_prechecked,
                        view.is_error,
                        view.status_message
                            .as_ref()
                            .map(ToString::to_string)
                            .unwrap_or_default(),
                    )
                })
                .expect("read cancel-failure preview state");
            preview_complete = complete;
            preview_status = status;
            if complete || is_error {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            preview_complete,
            "cancel-failure production preview did not complete: {preview_status}"
        );
        drop(preview_runtime_guard);

        let live_instances = settings_restore_live_instances(&target_root);
        assert_eq!(live_instances.len(), 1);
        let live_instance = live_instances[0].clone();
        let manifest_path = live_instance.join(".hivegui-db-instance-v1.json");
        let manifest_before = fs::read(&manifest_path).expect("read cancel-failure manifest");
        let manifest: serde_json::Value =
            serde_json::from_slice(&manifest_before).expect("parse cancel-failure manifest");
        assert_eq!(manifest["ownership_state"], "unarmed");
        let operation_id = manifest["db_instance_operation_id"]
            .as_str()
            .expect("cancel-failure operation id");
        let safety_snapshot = target_root
            .join("backups")
            .join(format!("restore-safety-{operation_id}"));
        assert!(!safety_snapshot.exists());
        assert!(
            !live_instance.join(".hivegui-db-recovery-v1.json").exists()
                && !live_instance
                    .join(".hivegui-db-recovery-v1.json.staging")
                    .exists(),
            "Ready preview must remain owner-free before cancellation"
        );
        assert_eq!(
            data_source_names(&runtime, &live_instance.join("datasources.db")),
            vec!["settings-cancel-failure-archive"]
        );

        let staging_canary_path = live_instance.join(".hivegui-db-instance-v1.json.staging");
        fs::write(&staging_canary_path, STAGING_CANARY)
            .expect("plant deterministic cancellation-retirement canary");
        assert_eq!(
            fs::read(&staging_canary_path).expect("read planted staging canary"),
            STAGING_CANARY
        );
        assert!(!live_store.pool().is_closed());

        let cancel_runtime_guard = runtime.enter();
        window
            .update(cx, |view, window, cx| {
                view.is_replacement_confirmed = true;
                let input = view
                    .import_path_input
                    .clone()
                    .expect("render created cancel-failure restore-path InputState");
                input.update(cx, |input, cx| {
                    input.set_value(changed_archive_value.clone(), window, cx);
                    cx.emit(InputEvent::Change);
                });
            })
            .expect("trigger real restore-path Change for failed cancellation");
        cx.run_until_parked();
        let (changed_path, prechecked, confirmed, busy) = window
            .update(cx, |view, _, _| {
                (
                    view.import_path.to_string(),
                    view.is_restoring_prechecked,
                    view.is_replacement_confirmed,
                    view.restore_preview_busy,
                )
            })
            .expect("read synchronous cancel-failure Change state");
        assert_eq!(changed_path, changed_archive_value);
        assert!(!prechecked);
        assert!(!confirmed);
        assert!(busy, "real cancellation must own the preview lifecycle");

        let mut cancellation_finished = false;
        let mut terminal_status = String::new();
        for _ in 0..500 {
            cx.run_until_parked();
            let (is_error, status) = window
                .update(cx, |view, _, _| {
                    (
                        view.is_error,
                        view.status_message
                            .as_ref()
                            .map(ToString::to_string)
                            .unwrap_or_default(),
                    )
                })
                .expect("read cancel-failure outcome state");
            terminal_status = status;
            if is_error && terminal_status.contains("清理") {
                cancellation_finished = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            cancellation_finished,
            "timed out waiting for deterministic preview cancellation failure: {terminal_status}"
        );

        let (restart_required, busy, importing, prechecked, confirmed, coordinator_retained) =
            window
                .update(cx, |view, _, _| {
                    (
                        view.restore_restart_required,
                        view.restore_preview_busy,
                        view.is_importing,
                        view.is_restoring_prechecked,
                        view.is_replacement_confirmed,
                        view.restore_coordinator.is_some(),
                    )
                })
                .expect("read cancel-failure terminal state");
        assert!(
            restart_required,
            "failed preview retirement must make Settings restart-only"
        );
        assert!(busy, "failed preview retirement must remain lifecycle-busy");
        assert!(!importing);
        assert!(!prechecked);
        assert!(!confirmed);
        assert!(
            coordinator_retained,
            "restart-only Settings must retain the exact Store-bound coordinator and OS owner"
        );
        assert!(
            live_store.pool().is_closed(),
            "failed preview retirement must close the shared Store pool before publishing terminal UI state"
        );
        let blocked_write = runtime
            .block_on(live_store.create(
                "settings-must-not-write-after-cancel-failure",
                "127.0.0.1",
                3308,
                "blocked-user",
                b"blocked-password",
            ))
            .expect_err("failed preview retirement must block every business write until restart");
        assert!(blocked_write.to_string().contains("write_gate_closed"));

        assert_eq!(
            settings_restore_live_instances(&target_root),
            live_instances
        );
        assert!(live_instance.is_dir());
        assert_eq!(
            fs::read(&manifest_path).expect("reread cancel-failure manifest"),
            manifest_before,
            "failed cancellation must not rewrite the unarmed live manifest"
        );
        assert_eq!(
            fs::read(&staging_canary_path).expect("reread staging canary"),
            STAGING_CANARY,
            "failed cancellation must leave the conflicting staging canary byte-for-byte intact"
        );
        assert!(
            !live_instance.join(".hivegui-db-recovery-v1.json").exists()
                && !live_instance
                    .join(".hivegui-db-recovery-v1.json.staging")
                    .exists(),
            "failed cancellation must not publish a recovery owner"
        );
        assert!(!safety_snapshot.exists());
        assert_eq!(
            data_source_names(&runtime, &live_instance.join("datasources.db")),
            vec!["settings-cancel-failure-archive"]
        );
        assert_eq!(
            data_source_names_with_sidecars(&runtime, &target_root.join("datasources.db")),
            vec!["settings-cancel-failure-current"]
        );

        assert!(
            terminal_status.contains("清理")
                && terminal_status.contains("未能安全完成")
                && terminal_status.contains("Store")
                && terminal_status.contains("锁定")
                && terminal_status.contains("必须重启"),
            "terminal status must only explain incomplete cleanup, locked Store, and mandatory restart: {terminal_status}"
        );
        let staging_canary_text =
            std::str::from_utf8(STAGING_CANARY).expect("staging canary is UTF-8");
        for secret in [
            PASSPHRASE,
            archive.to_string_lossy().as_ref(),
            changed_archive.to_string_lossy().as_ref(),
            staging_canary_text,
            live_instance.to_string_lossy().as_ref(),
            staging_canary_path.to_string_lossy().as_ref(),
            ".hivegui-db-instance-v1.json.staging",
            "ambiguous unarmed restore ownership",
        ] {
            assert!(
                !terminal_status.contains(secret),
                "terminal cleanup status leaked sensitive/internal detail {secret:?}: {terminal_status}"
            );
        }

        let terminal_lock = run_settings_restore_lock_child(&target_root);
        assert_settings_restore_lock_child(
            &terminal_lock,
            "failed preview retirement must retain the independent OS Store lock until restart",
        );

        let (_should_not_be_ready, _should_not_release) = window
            .update(cx, |view, _, _| {
                view.install_restore_preview_delivery_interlock_for_test()
            })
            .expect("install no-progress preview interlock");
        window
            .update(cx, |view, _, cx| view.preview_restore(cx))
            .expect("attempt preview after cancellation failure");
        window
            .update(cx, |view, _, cx| view.do_import(cx))
            .expect("attempt import after cancellation failure");
        for _ in 0..5 {
            std::thread::sleep(Duration::from_millis(20));
            cx.run_until_parked();
        }
        let (
            still_restart_required,
            still_busy,
            still_importing,
            seam_unconsumed,
            coordinator_still_retained,
        ) = window
            .update(cx, |view, _, _| {
                (
                    view.restore_restart_required,
                    view.restore_preview_busy,
                    view.is_importing,
                    view.restore_preview_delivery_interlock.is_some(),
                    view.restore_coordinator.is_some(),
                )
            })
            .expect("read stable restart-only state");
        assert!(
            still_restart_required
                && still_busy
                && !still_importing
                && seam_unconsumed
                && coordinator_still_retained
        );
        assert!(live_store.pool().is_closed());
        assert_eq!(
            settings_restore_live_instances(&target_root),
            live_instances
        );
        assert!(!safety_snapshot.exists());
        drop(cancel_runtime_guard);
    }

    #[gpui::test]
    fn stale_preview_outcome_is_cancelled_after_path_change_before_delivery(
        cx: &mut TestAppContext,
    ) {
        const PASSPHRASE: &str = "T130-settings-stale-preview-passphrase";

        cx.update(|cx| {
            gpui_component::theme::init(cx);
            gpui_component::init(cx);
        });
        let temp_dir = tempfile::tempdir().expect("create stale-preview fixture");
        let source_a = temp_dir.path().join("stale-source-a");
        let source_b = temp_dir.path().join("stale-source-b");
        let target_root = temp_dir.path().join("stale-production-target");
        let archive_a = temp_dir.path().join("stale-preview-a.age.tar");
        let archive_b = temp_dir.path().join("stale-preview-b.age.tar");
        let runtime = tokio::runtime::Runtime::new().expect("create stale-preview Tokio runtime");
        export_settings_data_source_archive(
            &runtime,
            &source_a,
            &archive_a,
            "settings-stale-preview-a",
            PASSPHRASE,
        );
        export_settings_data_source_archive(
            &runtime,
            &source_b,
            &archive_b,
            "settings-stale-preview-b",
            PASSPHRASE,
        );
        let store = runtime
            .block_on(Store::open_local(StoreOpenOptions::for_root(&target_root)))
            .expect("open stale-preview production Store");
        runtime
            .block_on(store.create(
                "settings-stale-current",
                "127.0.0.1",
                3307,
                "current-user",
                b"current-password",
            ))
            .expect("seed stale-preview current state");
        let live_store = store.clone();
        let archive_a_value = archive_a.to_string_lossy().into_owned();
        let archive_b_value = archive_b.to_string_lossy().into_owned();

        cx.dispatcher.allow_parking();
        let first_runtime_guard = runtime.enter();
        let window = cx.open_window(size(px(900.0), px(720.0)), move |_, cx| {
            let mut view = SettingsView::new(cx, None);
            view.set_store(store);
            view.import_path = archive_a_value.into();
            view.backup_passphrase = PASSPHRASE.into();
            view
        });
        cx.run_until_parked();
        let (mut outcome_ready, outcome_release) = window
            .update(cx, |view, _, _| {
                view.install_restore_preview_delivery_interlock_for_test()
            })
            .expect("install thin preview delivery interlock");
        window
            .update(cx, |view, _, cx| view.preview_restore(cx))
            .expect("start interlocked A preview");

        let mut a_outcome_ready = false;
        for _ in 0..3_000 {
            cx.run_until_parked();
            match outcome_ready.try_recv() {
                Ok(()) => {
                    a_outcome_ready = true;
                    break;
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {}
                Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                    panic!("preview delivery interlock closed before reporting ready")
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            a_outcome_ready,
            "timed out after the real A core preview outcome"
        );
        let (a_installed, a_prechecked) = window
            .update(cx, |view, _, _| {
                (
                    view.prepared_restore.is_some(),
                    view.is_restoring_prechecked,
                )
            })
            .expect("read interlocked A delivery state");
        assert!(
            !a_installed && !a_prechecked,
            "the thin interlock must pause before the unique production outcome install path"
        );
        let a_instances = settings_restore_live_instances(&target_root);
        assert_eq!(a_instances.len(), 1);
        let a_live = a_instances[0].clone();
        let a_manifest: serde_json::Value = serde_json::from_slice(
            &fs::read(a_live.join(".hivegui-db-instance-v1.json"))
                .expect("read interlocked A manifest"),
        )
        .expect("parse interlocked A manifest");
        assert_eq!(a_manifest["ownership_state"], "unarmed");
        let a_operation_id = a_manifest["db_instance_operation_id"]
            .as_str()
            .expect("interlocked A operation id");
        let a_safety = target_root
            .join("backups")
            .join(format!("restore-safety-{a_operation_id}"));
        assert!(!a_safety.exists());
        drop(first_runtime_guard);
        assert_eq!(
            data_source_names(&runtime, &a_live.join("datasources.db")),
            vec!["settings-stale-preview-a"]
        );

        let busy_runtime_guard = runtime.enter();
        window
            .update(cx, |view, window, cx| {
                let input = view
                    .import_path_input
                    .clone()
                    .expect("render created stale-preview path InputState");
                input.update(cx, |input, cx| {
                    input.set_value(archive_b_value.clone(), window, cx);
                    cx.emit(InputEvent::Change);
                });
            })
            .expect("change A path to B before A delivery");
        cx.run_until_parked();
        let changed_path = window
            .update(cx, |view, _, _| view.import_path.to_string())
            .expect("read B path after real InputState Change");
        assert_eq!(changed_path, archive_b_value);

        window
            .update(cx, |view, _, cx| view.preview_restore(cx))
            .expect("attempt duplicate preview while A delivery remains busy");
        let (duplicate_rejected, duplicate_prechecked, duplicate_status) = window
            .update(cx, |view, _, _| {
                (
                    view.is_error,
                    view.is_restoring_prechecked,
                    view.status_message
                        .as_ref()
                        .map(ToString::to_string)
                        .unwrap_or_default(),
                )
            })
            .expect("read duplicate-preview rejection state");
        let mut outcome_release = Some(outcome_release);
        if !duplicate_rejected || duplicate_prechecked {
            let _ = outcome_release
                .take()
                .expect("A outcome release remains available")
                .send(());
            panic!(
                "a duplicate preview must be synchronously rejected while A outcome delivery is busy; is_error={duplicate_rejected}, prechecked={duplicate_prechecked}, status={duplicate_status:?}"
            );
        }
        assert_eq!(
            settings_restore_live_instances(&target_root),
            vec![a_live.clone()],
            "busy duplicate rejection must leave only the original A unarmed instance"
        );

        outcome_release
            .take()
            .expect("release A outcome exactly once")
            .send(())
            .expect("release interlocked A outcome");
        let mut stale_a_cancelled = false;
        for _ in 0..500 {
            cx.run_until_parked();
            let no_installed_preview = window
                .update(cx, |view, _, _| {
                    view.prepared_restore.is_none()
                        && !view.is_restoring_prechecked
                        && !view.restore_preview_busy
                })
                .expect("read released stale A state");
            if no_installed_preview && settings_restore_live_instances(&target_root).is_empty() {
                stale_a_cancelled = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            stale_a_cancelled,
            "released A outcome must not install after path B wins and must retire through real cancel_preview"
        );
        assert!(!a_safety.exists());
        drop(busy_runtime_guard);
        assert!(!live_store.pool().is_closed());
        runtime
            .block_on(live_store.create(
                "settings-write-after-stale-preview",
                "127.0.0.1",
                3308,
                "after-stale-user",
                b"after-stale-password",
            ))
            .expect("stale preview cancellation must leave current Store writable");

        let b_runtime_guard = runtime.enter();
        window
            .update(cx, |view, _, cx| view.preview_restore(cx))
            .expect("start B preview after stale A retirement");
        let mut b_complete = false;
        let mut b_status = String::new();
        for _ in 0..3_000 {
            cx.run_until_parked();
            let (complete, is_error, status) = window
                .update(cx, |view, _, _| {
                    (
                        view.is_restoring_prechecked,
                        view.is_error,
                        view.status_message
                            .as_ref()
                            .map(ToString::to_string)
                            .unwrap_or_default(),
                    )
                })
                .expect("read B preview state");
            b_complete = complete;
            b_status = status;
            if complete || is_error {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(b_complete, "B preview did not complete: {b_status}");
        let b_instances = settings_restore_live_instances(&target_root);
        assert_eq!(
            b_instances.len(),
            1,
            "B preview must retain exactly one new unarmed instance"
        );
        let b_live = &b_instances[0];
        assert_ne!(b_live, &a_live);
        let b_manifest: serde_json::Value = serde_json::from_slice(
            &fs::read(b_live.join(".hivegui-db-instance-v1.json"))
                .expect("read B preview manifest"),
        )
        .expect("parse B preview manifest");
        assert_eq!(b_manifest["ownership_state"], "unarmed");
        let b_operation_id = b_manifest["db_instance_operation_id"]
            .as_str()
            .expect("B preview operation id");
        let b_safety = target_root
            .join("backups")
            .join(format!("restore-safety-{b_operation_id}"));
        assert!(!a_safety.exists());
        assert!(!b_safety.exists());
        drop(b_runtime_guard);
        assert_eq!(
            data_source_names(&runtime, &b_live.join("datasources.db")),
            vec!["settings-stale-preview-b"]
        );
    }

    #[gpui::test]
    fn repeating_a_ready_preview_is_rejected_without_replacing_its_exact_pair(
        cx: &mut TestAppContext,
    ) {
        const PASSPHRASE: &str = "T130-settings-ready-repeat-passphrase";

        cx.update(|cx| {
            gpui_component::theme::init(cx);
            gpui_component::init(cx);
        });
        let temp_dir = tempfile::tempdir().expect("create Ready-repeat fixture");
        let source_root = temp_dir.path().join("ready-repeat-source");
        let target_root = temp_dir.path().join("ready-repeat-target");
        let archive = temp_dir.path().join("ready-repeat.age.tar");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("create deterministic Ready-repeat Tokio runtime");
        export_settings_data_source_archive(
            &runtime,
            &source_root,
            &archive,
            "settings-ready-repeat-new",
            PASSPHRASE,
        );
        let store = runtime
            .block_on(Store::open_local(StoreOpenOptions::for_root(&target_root)))
            .expect("open Ready-repeat production Store");
        runtime
            .block_on(store.create(
                "settings-ready-repeat-current",
                "127.0.0.1",
                3307,
                "current-user",
                b"current-password",
            ))
            .expect("seed Ready-repeat current state");
        let live_store = store.clone();
        let view = cx.new(|cx| {
            let mut view = SettingsView::new(cx, None);
            view.set_store(store);
            view.import_path = archive.to_string_lossy().to_string().into();
            view.backup_passphrase = PASSPHRASE.into();
            view
        });

        cx.dispatcher.allow_parking();
        {
            let first_preview_guard = runtime.enter();
            cx.update(|cx| view.update(cx, |view, cx| view.preview_restore(cx)));
            drop(first_preview_guard);
        }
        let mut first_ready = false;
        let mut first_status = String::new();
        for _ in 0..3_000 {
            runtime.block_on(async { tokio::time::sleep(Duration::from_millis(10)).await });
            cx.run_until_parked();
            let (ready, is_error, status) = cx.update(|cx| {
                let view = view.read(cx);
                (
                    view.is_restoring_prechecked && !view.restore_preview_busy,
                    view.is_error,
                    view.status_message
                        .as_ref()
                        .map(ToString::to_string)
                        .unwrap_or_default(),
                )
            });
            first_ready = ready;
            first_status = status;
            if ready || is_error {
                break;
            }
        }
        assert!(
            first_ready,
            "first preview did not become Ready: {first_status}"
        );
        let (operation_id, safety_snapshot, impact) = cx.update(|cx| {
            let view = view.read(cx);
            assert!(view.restore_coordinator.is_some());
            let prepared = view
                .prepared_restore
                .as_ref()
                .expect("Ready preview retains exact PreparedRestore");
            (
                prepared.db_instance_operation_id().to_owned(),
                prepared.safety_backup_path().to_path_buf(),
                view.restore_impact_preview.clone(),
            )
        });
        let live_instances = settings_restore_live_instances(&target_root);
        assert_eq!(live_instances.len(), 1);
        let live_instance = live_instances[0].clone();
        assert_eq!(
            live_instance.file_name().and_then(|name| name.to_str()),
            Some(format!("restore-{operation_id}").as_str())
        );
        let manifest: serde_json::Value = serde_json::from_slice(
            &fs::read(live_instance.join(".hivegui-db-instance-v1.json"))
                .expect("read Ready-repeat manifest"),
        )
        .expect("parse Ready-repeat manifest");
        assert_eq!(manifest["ownership_state"], "unarmed");
        assert!(!safety_snapshot.exists());
        assert_eq!(
            data_source_names(&runtime, &live_instance.join("datasources.db")),
            vec!["settings-ready-repeat-new"]
        );

        let (_unused_ready, unused_release) = cx.update(|cx| {
            view.update(cx, |view, _| {
                view.install_restore_preview_delivery_interlock_for_test()
            })
        });
        {
            let repeated_preview_guard = runtime.enter();
            cx.update(|cx| view.update(cx, |view, cx| view.preview_restore(cx)));
            drop(repeated_preview_guard);
        }
        let (
            repeat_rejected,
            still_ready,
            still_not_busy,
            same_operation,
            same_safety,
            same_impact,
            same_coordinator_present,
            interlock_unconsumed,
            repeat_status,
        ) = cx.update(|cx| {
            let view = view.read(cx);
            let prepared = view
                .prepared_restore
                .as_ref()
                .expect("repeated Ready preview must preserve PreparedRestore");
            (
                view.is_error,
                view.is_restoring_prechecked,
                !view.restore_preview_busy,
                prepared.db_instance_operation_id() == operation_id,
                prepared.safety_backup_path() == safety_snapshot,
                view.restore_impact_preview == impact,
                view.restore_coordinator.is_some(),
                view.restore_preview_delivery_interlock.is_some(),
                view.status_message
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_default(),
            )
        });
        assert!(
            repeat_rejected,
            "a repeated production preview must synchronously reject an existing Ready pair: {repeat_status}"
        );
        assert!(
            still_ready
                && still_not_busy
                && same_operation
                && same_safety
                && same_impact
                && same_coordinator_present
                && interlock_unconsumed,
            "Ready rejection must preserve the exact coordinator/prepared pair and must not start a second core task"
        );

        for _ in 0..5 {
            runtime.block_on(async { tokio::time::sleep(Duration::from_millis(20)).await });
            cx.run_until_parked();
        }
        let stable = cx.update(|cx| {
            let view = view.read(cx);
            let prepared = view
                .prepared_restore
                .as_ref()
                .expect("Ready pair remains installed after rejection");
            !view.restore_preview_busy
                && prepared.db_instance_operation_id() == operation_id
                && prepared.safety_backup_path() == safety_snapshot
                && view.restore_impact_preview == impact
                && view.restore_coordinator.is_some()
                && view.restore_preview_delivery_interlock.is_some()
        });
        assert!(
            stable,
            "rejected repeat must remain stable after runtime pumps"
        );
        assert_eq!(
            settings_restore_live_instances(&target_root),
            vec![live_instance]
        );
        assert!(!safety_snapshot.exists());
        assert!(!live_store.pool().is_closed());
        runtime
            .block_on(live_store.create(
                "settings-write-after-ready-repeat",
                "127.0.0.1",
                3308,
                "after-repeat-user",
                b"after-repeat-password",
            ))
            .expect("Ready repeat rejection must leave current Store writable");
        drop(unused_release);
    }

    #[gpui::test]
    fn archive_replacement_before_confirmation_cancels_ready_preview_without_freezing_store(
        cx: &mut TestAppContext,
    ) {
        const PASSPHRASE: &str = "T130-settings-confirm-preflight-passphrase";

        cx.update(|cx| {
            gpui_component::theme::init(cx);
            gpui_component::init(cx);
        });
        let temp_dir = tempfile::tempdir().expect("create confirmation-preflight fixture");
        let source_a_root = temp_dir.path().join("confirm-preflight-source-a");
        let source_b_root = temp_dir.path().join("confirm-preflight-source-b");
        let target_root = temp_dir.path().join("confirm-preflight-target");
        let archive = temp_dir.path().join("confirm-preflight.age.tar");
        let replacement = temp_dir.path().join("confirm-preflight-b.age.tar");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("create deterministic confirmation-preflight Tokio runtime");
        export_settings_data_source_archive(
            &runtime,
            &source_a_root,
            &archive,
            "settings-confirm-preflight-a",
            PASSPHRASE,
        );
        export_settings_data_source_archive(
            &runtime,
            &source_b_root,
            &replacement,
            "settings-confirm-preflight-b",
            PASSPHRASE,
        );
        let store = runtime
            .block_on(Store::open_local(StoreOpenOptions::for_root(&target_root)))
            .expect("open confirmation-preflight production Store");
        runtime
            .block_on(store.create(
                "settings-confirm-preflight-current",
                "127.0.0.1",
                3307,
                "current-user",
                b"current-password",
            ))
            .expect("seed confirmation-preflight current state");
        let live_store = store.clone();
        let view = cx.new(|cx| {
            let mut view = SettingsView::new(cx, None);
            view.set_store(store);
            view.import_path = archive.to_string_lossy().to_string().into();
            view.backup_passphrase = PASSPHRASE.into();
            view
        });

        cx.dispatcher.allow_parking();
        {
            let preview_guard = runtime.enter();
            cx.update(|cx| view.update(cx, |view, cx| view.preview_restore(cx)));
            drop(preview_guard);
        }
        let mut first_ready = false;
        let mut first_status = String::new();
        for _ in 0..3_000 {
            runtime.block_on(async { tokio::time::sleep(Duration::from_millis(10)).await });
            cx.run_until_parked();
            let (ready, is_error, status) = cx.update(|cx| {
                let view = view.read(cx);
                (
                    view.is_restoring_prechecked && !view.restore_preview_busy,
                    view.is_error,
                    view.status_message
                        .as_ref()
                        .map(ToString::to_string)
                        .unwrap_or_default(),
                )
            });
            first_ready = ready;
            first_status = status;
            if ready || is_error {
                break;
            }
        }
        assert!(
            first_ready,
            "archive A preview did not become Ready: {first_status}"
        );
        let (operation_a, safety_a) = cx.update(|cx| {
            let view = view.read(cx);
            let prepared = view
                .prepared_restore
                .as_ref()
                .expect("Ready archive A preview retains its PreparedRestore");
            (
                prepared.db_instance_operation_id().to_owned(),
                prepared.safety_backup_path().to_path_buf(),
            )
        });
        let live_a = target_root
            .join(".hivegui-db-staging-v1")
            .join(format!("restore-{operation_a}"));
        assert_eq!(
            data_source_names(&runtime, &live_a.join("datasources.db")),
            vec!["settings-confirm-preflight-a"]
        );
        assert!(!safety_a.exists());

        // Keep the canonical pathname continuously bound to legal archive B
        // before invoking the real production confirmation handler. The
        // PreparedRestore must remain bound to the descriptor for archive A.
        fs::remove_file(&archive).expect("unlink canonical archive A leaf");
        fs::rename(&replacement, &archive).expect("publish archive B at the canonical path");

        // A current-thread runtime installs the spawn handle without polling
        // cancellation. This makes the synchronous begin-preflight result
        // observable before the exact Ready pair is retired asynchronously.
        {
            let confirmation_guard = runtime.enter();
            cx.update(|cx| {
                view.update(cx, |view, cx| {
                    view.is_replacement_confirmed = true;
                    view.do_import(cx);
                });
            });
            drop(confirmation_guard);
        }
        let (
            restart_required,
            importing,
            cancelling,
            pool_closed,
            prechecked,
            replacement_confirmed,
            impact,
            status,
        ) = cx.update(|cx| {
            let view = view.read(cx);
            (
                view.restore_restart_required,
                view.is_importing,
                view.restore_preview_busy,
                view.store
                    .as_ref()
                    .expect("Settings keeps the production Store bound")
                    .pool()
                    .is_closed(),
                view.is_restoring_prechecked,
                view.is_replacement_confirmed,
                view.restore_impact_preview.clone(),
                view.status_message
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_default(),
            )
        });
        assert!(
            !restart_required,
            "archive identity failure happens before confirmation CAS and must cancel the exact Ready pair instead of entering restart-only: {status}"
        );
        assert!(!importing, "archive identity failure must not start apply");
        assert!(
            cancelling,
            "the production handler must synchronously enter exact-preview retirement"
        );
        assert!(
            !pool_closed && !live_store.pool().is_closed(),
            "pre-CAS archive identity failure must leave the shared Store pool open"
        );
        assert!(
            !prechecked && !replacement_confirmed && impact == "恢复前请先预检",
            "rejected confirmation must synchronously invalidate the stale preview flags and impact"
        );
        assert!(!safety_a.exists());
        assert_eq!(
            status,
            "恢复确认未完成；正在安全取消本次预检，完成后请重新预检"
        );
        for secret_or_internal in [
            archive.to_string_lossy().as_ref(),
            PASSPHRASE,
            "UnsafeArchiveEntry",
            "archive contains unsafe entry",
            "io error:",
            "storage_recovery_blocked",
            "prepared_restore_not_active",
            "restore interrupted at",
        ] {
            assert!(
                !status.contains(secret_or_internal),
                "confirmation-preflight status leaked raw input or internal error: {status}"
            );
        }

        let mut retired = false;
        let mut settled_status = String::new();
        for _ in 0..3_000 {
            runtime.block_on(async { tokio::time::sleep(Duration::from_millis(10)).await });
            cx.run_until_parked();
            let (settled, terminal, prechecked, confirmed, impact, status) = cx.update(|cx| {
                let view = view.read(cx);
                (
                    !view.restore_preview_busy
                        && view.restore_coordinator.is_none()
                        && view.prepared_restore.is_none(),
                    view.restore_restart_required,
                    view.is_restoring_prechecked,
                    view.is_replacement_confirmed,
                    view.restore_impact_preview.clone(),
                    view.status_message
                        .as_ref()
                        .map(ToString::to_string)
                        .unwrap_or_default(),
                )
            });
            assert!(
                !terminal,
                "successful pre-CAS preview retirement must not become restart-only"
            );
            if settled {
                assert!(
                    !prechecked && !confirmed && impact == "恢复前请先预检",
                    "settled cancellation must retain cleared preview flags and impact"
                );
                settled_status = status;
                retired = true;
                break;
            }
        }
        assert!(
            retired,
            "archive A Ready pair did not retire after rejection"
        );
        assert_eq!(settled_status, "本次预检已取消，请重新预检");
        assert!(
            settings_restore_live_instances(&target_root).is_empty(),
            "the exact archive A unarmed instance must be retired before another preview"
        );
        assert!(!safety_a.exists());
        assert!(!live_store.pool().is_closed());
        runtime
            .block_on(live_store.create(
                "settings-write-after-confirm-preflight-rejection",
                "127.0.0.1",
                3308,
                "after-rejection-user",
                b"after-rejection-password",
            ))
            .expect("pre-CAS archive replacement must leave the Store write gate open");

        {
            let second_preview_guard = runtime.enter();
            cx.update(|cx| view.update(cx, |view, cx| view.preview_restore(cx)));
            drop(second_preview_guard);
        }
        let mut second_ready = false;
        let mut second_status = String::new();
        for _ in 0..3_000 {
            runtime.block_on(async { tokio::time::sleep(Duration::from_millis(10)).await });
            cx.run_until_parked();
            let (ready, is_error, status) = cx.update(|cx| {
                let view = view.read(cx);
                (
                    view.is_restoring_prechecked && !view.restore_preview_busy,
                    view.is_error,
                    view.status_message
                        .as_ref()
                        .map(ToString::to_string)
                        .unwrap_or_default(),
                )
            });
            second_ready = ready;
            second_status = status;
            if ready || is_error {
                break;
            }
        }
        assert!(
            second_ready,
            "archive B could not be previewed after archive A retirement: {second_status}"
        );
        let operation_b = cx.update(|cx| {
            view.read(cx)
                .prepared_restore
                .as_ref()
                .expect("archive B preview retains a new PreparedRestore")
                .db_instance_operation_id()
                .to_owned()
        });
        assert_ne!(operation_b, operation_a);
        let live_instances = settings_restore_live_instances(&target_root);
        assert_eq!(live_instances.len(), 1);
        assert_eq!(
            data_source_names(&runtime, &live_instances[0].join("datasources.db")),
            vec!["settings-confirm-preflight-b"]
        );
        assert!(!live_store.pool().is_closed());
    }

    #[gpui::test]
    fn post_cas_pre_freeze_confirmation_failure_enters_fail_closed_restart_terminal(
        cx: &mut TestAppContext,
    ) {
        const PASSPHRASE: &str = "T130-settings-post-cas-terminal-passphrase";
        const TERMINAL_STATUS: &str =
            "恢复确认未能安全完成；Store 已进入保护状态，必须重启 HiveGUI 进行恢复校验";

        cx.update(|cx| {
            gpui_component::theme::init(cx);
            gpui_component::init(cx);
        });
        let temp_dir = tempfile::tempdir().expect("create post-CAS terminal fixture");
        let source_root = temp_dir.path().join("post-cas-terminal-source");
        let target_root = temp_dir.path().join("post-cas-terminal-target");
        let archive = temp_dir.path().join("post-cas-terminal.age.tar");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("create deterministic post-CAS terminal Tokio runtime");
        export_settings_data_source_archive(
            &runtime,
            &source_root,
            &archive,
            "settings-post-cas-restored",
            PASSPHRASE,
        );
        let store = runtime
            .block_on(Store::open_local(StoreOpenOptions::for_root(&target_root)))
            .expect("open post-CAS terminal production Store");
        runtime
            .block_on(store.create(
                "settings-post-cas-current",
                "127.0.0.1",
                3307,
                "current-user",
                b"current-password",
            ))
            .expect("seed post-CAS current state");
        let live_store = store.clone();
        let view = cx.new(|cx| {
            let mut view = SettingsView::new(cx, None);
            view.set_store(store);
            view.import_path = archive.to_string_lossy().to_string().into();
            view.backup_passphrase = PASSPHRASE.into();
            view
        });

        cx.dispatcher.allow_parking();
        {
            let preview_guard = runtime.enter();
            cx.update(|cx| view.update(cx, |view, cx| view.preview_restore(cx)));
            drop(preview_guard);
        }
        let mut ready = false;
        let mut preview_status = String::new();
        for _ in 0..3_000 {
            runtime.block_on(async { tokio::time::sleep(Duration::from_millis(10)).await });
            cx.run_until_parked();
            let (is_ready, is_error, status) = cx.update(|cx| {
                let view = view.read(cx);
                (
                    view.is_restoring_prechecked && !view.restore_preview_busy,
                    view.is_error,
                    view.status_message
                        .as_ref()
                        .map(ToString::to_string)
                        .unwrap_or_default(),
                )
            });
            ready = is_ready;
            preview_status = status;
            if is_ready || is_error {
                break;
            }
        }
        assert!(
            ready,
            "post-CAS preview did not become Ready: {preview_status}"
        );

        let (operation_id, safety_snapshot) = cx.update(|cx| {
            view.update(cx, |view, _| {
                let coordinator = view
                    .restore_coordinator
                    .as_ref()
                    .expect("Ready preview retains its exact coordinator");
                let prepared = view
                    .prepared_restore
                    .as_ref()
                    .expect("Ready preview retains its exact PreparedRestore");
                coordinator
                    .install_begin_confirmation_post_cas_pre_freeze_failure_for_test(prepared)
                    .expect("install exact one-shot post-CAS/pre-freeze fault");
                (
                    prepared.db_instance_operation_id().to_owned(),
                    prepared.safety_backup_path().to_path_buf(),
                )
            })
        });
        let live = target_root
            .join(".hivegui-db-staging-v1")
            .join(format!("restore-{operation_id}"));
        let manifest_path = live.join(".hivegui-db-instance-v1.json");
        let manifest_before = fs::read(&manifest_path).expect("read unarmed pre-CAS manifest");
        let manifest: serde_json::Value =
            serde_json::from_slice(&manifest_before).expect("parse unarmed pre-CAS manifest");
        assert_eq!(manifest["ownership_state"], "unarmed");
        assert!(!live.join(".hivegui-db-recovery-v1.json").exists());
        assert!(!live.join(".hivegui-db-recovery-v1.json.staging").exists());
        assert!(!safety_snapshot.exists());

        {
            let confirmation_guard = runtime.enter();
            cx.update(|cx| {
                view.update(cx, |view, cx| {
                    view.is_replacement_confirmed = true;
                    view.do_import(cx);
                });
            });
            drop(confirmation_guard);
        }
        let (
            restart_required,
            importing,
            terminal_busy,
            pool_closed,
            coordinator_retained,
            prepared_retained,
            prechecked,
            replacement_confirmed,
            impact,
            status,
        ) = cx.update(|cx| {
            let view = view.read(cx);
            (
                view.restore_restart_required,
                view.is_importing,
                view.restore_preview_busy,
                view.store
                    .as_ref()
                    .expect("terminal Settings keeps its exact Store")
                    .pool()
                    .is_closed(),
                view.restore_coordinator.is_some(),
                view.prepared_restore
                    .as_ref()
                    .is_some_and(|prepared| prepared.db_instance_operation_id() == operation_id),
                view.is_restoring_prechecked,
                view.is_replacement_confirmed,
                view.restore_impact_preview.clone(),
                view.status_message
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_default(),
            )
        });
        assert!(restart_required, "post-CAS ambiguity must require restart");
        assert!(!importing, "failed begin must not spawn restore finish");
        assert!(
            pool_closed && live_store.pool().is_closed(),
            "post-CAS failure must synchronously mark the exact shared Store pool closed"
        );
        assert!(
            terminal_busy,
            "post-CAS ambiguity must remain terminal-busy and reject new restore work"
        );
        assert!(
            coordinator_retained && prepared_retained,
            "terminal Settings must retain the exact coordinator/PreparedRestore and its OS owner"
        );
        assert!(
            !prechecked && !replacement_confirmed && impact == "恢复前请先预检",
            "post-CAS terminal must synchronously clear preview flags and replacement impact"
        );
        assert_eq!(status, TERMINAL_STATUS);
        for secret_or_internal in [
            archive.to_string_lossy().as_ref(),
            PASSPHRASE,
            operation_id.as_str(),
            "confirmation_post_cas_pre_freeze",
            "restore interrupted at",
            "io error:",
            "storage_recovery_blocked",
            "prepared_restore_not_active",
        ] {
            assert!(
                !status.contains(secret_or_internal),
                "post-CAS terminal status leaked raw input or internal error: {status}"
            );
        }

        let blocked_write = runtime
            .block_on(live_store.create(
                "settings-must-not-write-after-post-cas-failure",
                "127.0.0.1",
                3308,
                "blocked-user",
                b"blocked-password",
            ))
            .expect_err("post-CAS failure must close the canonical Store write gate");
        assert!(blocked_write.to_string().contains("write_gate_closed"));
        let terminal_lock = run_settings_restore_lock_child(&target_root);
        assert_settings_restore_lock_child(
            &terminal_lock,
            "post-CAS failure must retain the exact production Store OS lock until restart",
        );
        assert_eq!(
            fs::read(&manifest_path).expect("reread unarmed manifest"),
            manifest_before
        );
        assert!(live.is_dir());
        assert!(!live.join(".hivegui-db-recovery-v1.json").exists());
        assert!(!live.join(".hivegui-db-recovery-v1.json.staging").exists());
        assert!(!safety_snapshot.exists());
        assert_eq!(
            data_source_names_with_sidecars(&runtime, &target_root.join("datasources.db")),
            vec!["settings-post-cas-current"],
            "post-CAS terminal must preserve the exact old current database"
        );
        assert_eq!(
            data_source_names(&runtime, &live.join("datasources.db")),
            vec!["settings-post-cas-restored"],
            "post-CAS terminal must preserve the exact unarmed new staging database"
        );

        for _ in 0..3 {
            let terminal_guard = runtime.enter();
            cx.update(|cx| {
                view.update(cx, |view, cx| {
                    view.preview_restore(cx);
                    view.do_import(cx);
                });
            });
            drop(terminal_guard);
            cx.run_until_parked();
        }
        let (terminal_stable, prechecked, confirmed, impact, stable_status) = cx.update(|cx| {
            let view = view.read(cx);
            (
                view.restore_restart_required
                    && view.restore_preview_busy
                    && view.restore_coordinator.is_some()
                    && view.prepared_restore.as_ref().is_some_and(|prepared| {
                        prepared.db_instance_operation_id() == operation_id
                    })
                    && view
                        .store
                        .as_ref()
                        .expect("terminal Store remains bound")
                        .pool()
                        .is_closed(),
                view.is_restoring_prechecked,
                view.is_replacement_confirmed,
                view.restore_impact_preview.clone(),
                view.status_message
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_default(),
            )
        });
        assert!(
            terminal_stable,
            "preview/do_import attempts must not escape the post-CAS terminal"
        );
        assert!(
            !prechecked && !confirmed && impact == "恢复前请先预检",
            "terminal rejection rounds must preserve cleared preview flags and impact"
        );
        assert_eq!(stable_status, TERMINAL_STATUS);
        assert_eq!(settings_restore_live_instances(&target_root), vec![live]);
        assert!(!safety_snapshot.exists());
        assert_eq!(
            data_source_names_with_sidecars(&runtime, &target_root.join("datasources.db")),
            vec!["settings-post-cas-current"]
        );
        assert_eq!(
            data_source_names(
                &runtime,
                &settings_restore_live_instances(&target_root)[0].join("datasources.db"),
            ),
            vec!["settings-post-cas-restored"]
        );
    }

    #[gpui::test]
    fn consecutive_restore_input_changes_cancel_one_ready_preview_exactly_once(
        cx: &mut TestAppContext,
    ) {
        const PASSPHRASE: &str = "T130-settings-consecutive-input-cancel-passphrase";

        cx.update(|cx| {
            gpui_component::theme::init(cx);
            gpui_component::init(cx);
        });
        let temp_dir = tempfile::tempdir().expect("create consecutive input-change fixture");
        let source_root = temp_dir.path().join("consecutive-input-source");
        let target_root = temp_dir.path().join("consecutive-input-target");
        let archive = temp_dir.path().join("consecutive-input.age.tar");
        let replacement_path = temp_dir.path().join("replacement-selection.age.tar");
        let runtime =
            tokio::runtime::Runtime::new().expect("create consecutive input-change Tokio runtime");
        export_settings_data_source_archive(
            &runtime,
            &source_root,
            &archive,
            "settings-consecutive-preview",
            PASSPHRASE,
        );
        let store = runtime
            .block_on(Store::open_local(StoreOpenOptions::for_root(&target_root)))
            .expect("open consecutive input-change production Store");
        runtime
            .block_on(store.create(
                "settings-consecutive-current",
                "127.0.0.1",
                3307,
                "current-user",
                b"current-password",
            ))
            .expect("seed consecutive input-change current state");
        let live_store = store.clone();
        let archive_value = archive.to_string_lossy().into_owned();
        let replacement_value = replacement_path.to_string_lossy().into_owned();

        cx.dispatcher.allow_parking();
        let open_guard = runtime.enter();
        let window = cx.open_window(size(px(900.0), px(720.0)), move |_, cx| {
            let mut view = SettingsView::new(cx, None);
            view.set_store(store);
            view.import_path = archive_value.into();
            view.backup_passphrase = PASSPHRASE.into();
            view
        });
        cx.run_until_parked();
        window
            .update(cx, |view, _, cx| view.preview_restore(cx))
            .expect("start production preview before consecutive input changes");

        let mut ready = false;
        let mut preview_status = String::new();
        for _ in 0..3_000 {
            cx.run_until_parked();
            let (is_ready, is_error, status) = window
                .update(cx, |view, _, _| {
                    (
                        view.is_restoring_prechecked && !view.restore_preview_busy,
                        view.is_error,
                        view.status_message
                            .as_ref()
                            .map(ToString::to_string)
                            .unwrap_or_default(),
                    )
                })
                .expect("read consecutive input-change preview state");
            ready = is_ready;
            preview_status = status;
            if is_ready || is_error {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            ready,
            "consecutive input-change preview did not become Ready: {preview_status}"
        );
        drop(open_guard);

        let (operation_id, safety_snapshot) = window
            .update(cx, |view, _, _| {
                let prepared = view
                    .prepared_restore
                    .as_ref()
                    .expect("Ready preview retains its exact PreparedRestore");
                (
                    prepared.db_instance_operation_id().to_owned(),
                    prepared.safety_backup_path().to_path_buf(),
                )
            })
            .expect("capture exact Ready preview identity");
        let (mut cancel_ready, cancel_release, mut cancel_applied) = window
            .update(cx, |view, _, _| {
                view.install_restore_cancel_delivery_interlock_for_test()
            })
            .expect("install exact cancel-completion interlock");

        {
            let first_change_guard = runtime.enter();
            window
                .update(cx, |view, window, cx| {
                    view.is_replacement_confirmed = true;
                    let path_input = view
                        .import_path_input
                        .clone()
                        .expect("render created the production restore-path InputState");
                    path_input.update(cx, |input, cx| {
                        input.set_value(replacement_value.clone(), window, cx);
                        cx.emit(InputEvent::Change);
                    });
                })
                .expect("emit first real restore-path Change");
            drop(first_change_guard);
        }

        let first_cancel_delivery_guard = runtime.enter();
        let mut first_cancel_ready = false;
        for _ in 0..3_000 {
            cx.run_until_parked();
            match cancel_ready.try_recv() {
                Ok(()) => {
                    first_cancel_ready = true;
                    break;
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                    panic!("first cancel completion interlock closed before becoming ready")
                }
            }
        }
        assert!(
            first_cancel_ready,
            "first exact preview cancellation never reached its held delivery boundary"
        );
        let (busy_before_second_change, dispatches_before_second_change, exact_pair_held) = window
            .update(cx, |view, _, _| {
                (
                    view.restore_preview_busy,
                    view.restore_cancel_dispatch_count,
                    view.restore_coordinator.is_some()
                        && view.prepared_restore.as_ref().is_some_and(|prepared| {
                            prepared.db_instance_operation_id() == operation_id
                        }),
                )
            })
            .expect("read first held cancel state");
        assert!(busy_before_second_change && exact_pair_held);
        assert_eq!(dispatches_before_second_change, 1);
        assert!(
            settings_restore_live_instances(&target_root).is_empty(),
            "the first real cancel must retire the exact unarmed registry instance before delivery"
        );
        assert!(!safety_snapshot.exists());
        drop(first_cancel_delivery_guard);

        {
            let second_change_guard = runtime.enter();
            window
                .update(cx, |view, window, cx| {
                    view.is_restoring_prechecked = true;
                    view.is_replacement_confirmed = true;
                    let passphrase_input = view
                        .backup_passphrase_input
                        .clone()
                        .expect("render created the production passphrase InputState");
                    passphrase_input.update(cx, |input, cx| {
                        input.set_value("changed-passphrase", window, cx);
                        cx.emit(InputEvent::Change);
                    });
                })
                .expect("emit second real passphrase Change while cancellation is held");
            drop(second_change_guard);
        }

        let duplicate_cancel_delivery_guard = runtime.enter();
        let mut terminal_before_release = false;
        for _ in 0..300 {
            cx.run_until_parked();
            terminal_before_release = window
                .update(cx, |view, _, _| view.restore_restart_required)
                .expect("observe consecutive Change terminal state");
            if terminal_before_release {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let (
            dispatches_after_second_change,
            busy_after_second_change,
            exact_pair_after_second_change,
            prechecked_after_second_change,
            confirmed_after_second_change,
            impact_after_second_change,
            status_before_release,
        ) = window
            .update(cx, |view, _, _| {
                (
                    view.restore_cancel_dispatch_count,
                    view.restore_preview_busy,
                    view.restore_coordinator.is_some()
                        && view.prepared_restore.as_ref().is_some_and(|prepared| {
                            prepared.db_instance_operation_id() == operation_id
                        }),
                    view.is_restoring_prechecked,
                    view.is_replacement_confirmed,
                    view.restore_impact_preview.clone(),
                    view.status_message
                        .as_ref()
                        .map(ToString::to_string)
                        .unwrap_or_default(),
                )
            })
            .expect("capture state after the second real Change");
        drop(duplicate_cancel_delivery_guard);
        let write_while_cancel_delivery_is_held = runtime.block_on(live_store.create(
            "settings-write-during-single-cancel",
            "127.0.0.1",
            3308,
            "during-cancel-user",
            b"during-cancel-password",
        ));

        let late_cancel_delivery_guard = runtime.enter();
        cancel_release
            .send(())
            .expect("release the first exact cancel outcome");
        let mut first_cancel_applied = false;
        for _ in 0..300 {
            cx.run_until_parked();
            match cancel_applied.try_recv() {
                Ok(()) => {
                    first_cancel_applied = true;
                    break;
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                    panic!("first cancel applied interlock closed without delivery")
                }
            }
        }
        assert!(
            first_cancel_applied,
            "the released first exact cancel outcome was not applied"
        );
        for _ in 0..10 {
            cx.run_until_parked();
            std::thread::sleep(Duration::from_millis(10));
        }
        let (
            final_restart_required,
            final_busy,
            final_pair_cleared,
            final_prechecked,
            final_confirmed,
            final_impact,
        ) = window
            .update(cx, |view, _, _| {
                (
                    view.restore_restart_required,
                    view.restore_preview_busy,
                    view.restore_coordinator.is_none() && view.prepared_restore.is_none(),
                    view.is_restoring_prechecked,
                    view.is_replacement_confirmed,
                    view.restore_impact_preview.clone(),
                )
            })
            .expect("capture settled consecutive input-change state");
        drop(late_cancel_delivery_guard);
        let final_pool_was_open = !live_store.pool().is_closed();
        runtime.block_on(live_store.pool().close());

        assert_eq!(
            dispatches_after_second_change, 1,
            "a second Input Change while exact cancellation is busy must only invalidate the input generation, not dispatch duplicate cancel"
        );
        assert!(
            !terminal_before_release,
            "duplicate Input Change must not turn an in-flight successful cancellation terminal: {status_before_release}"
        );
        assert!(
            busy_after_second_change && exact_pair_after_second_change,
            "the first exact pair must remain owned until its one cancel outcome is delivered"
        );
        assert!(
            !prechecked_after_second_change
                && !confirmed_after_second_change
                && impact_after_second_change == "恢复前请先预检",
            "the second Change must still clear preview flags and replacement impact"
        );
        write_while_cancel_delivery_is_held
            .expect("one successful preview cancellation must leave the Store writable");
        assert!(
            !final_restart_required && !final_busy && final_pair_cleared,
            "the single successful cancel must settle without a restart-only downgrade"
        );
        assert!(!final_prechecked && !final_confirmed && final_impact == "恢复前请先预检");
        assert!(final_pool_was_open);
        assert!(settings_restore_live_instances(&target_root).is_empty());
        assert!(!safety_snapshot.exists());
    }

    #[gpui::test]
    fn late_cancel_success_cannot_clear_an_exact_pair_cancel_error_terminal(
        cx: &mut TestAppContext,
    ) {
        const PASSPHRASE: &str = "T130-settings-cancel-callback-order-passphrase";
        const TERMINAL_STATUS: &str = "恢复预检清理未能安全完成；Store 已锁定，必须重启 HiveGUI";

        cx.update(|cx| {
            gpui_component::theme::init(cx);
            gpui_component::init(cx);
        });
        let temp_dir = tempfile::tempdir().expect("create cancel callback-order fixture");
        let source_root = temp_dir.path().join("cancel-callback-source");
        let target_root = temp_dir.path().join("cancel-callback-target");
        let archive = temp_dir.path().join("cancel-callback.age.tar");
        let runtime =
            tokio::runtime::Runtime::new().expect("create cancel callback-order Tokio runtime");
        export_settings_data_source_archive(
            &runtime,
            &source_root,
            &archive,
            "settings-cancel-callback-preview",
            PASSPHRASE,
        );
        let store = runtime
            .block_on(Store::open_local(StoreOpenOptions::for_root(&target_root)))
            .expect("open cancel callback-order production Store");
        runtime
            .block_on(store.create(
                "settings-cancel-callback-current",
                "127.0.0.1",
                3307,
                "current-user",
                b"current-password",
            ))
            .expect("seed cancel callback-order current state");
        let live_store = store.clone();
        let view = cx.new(|cx| {
            let mut view = SettingsView::new(cx, None);
            view.set_store(store);
            view.import_path = archive.to_string_lossy().to_string().into();
            view.backup_passphrase = PASSPHRASE.into();
            view
        });

        cx.dispatcher.allow_parking();
        {
            let preview_guard = runtime.enter();
            cx.update(|cx| view.update(cx, |view, cx| view.preview_restore(cx)));
            drop(preview_guard);
        }
        let mut ready = false;
        let mut preview_status = String::new();
        for _ in 0..3_000 {
            runtime.block_on(async { tokio::time::sleep(Duration::from_millis(10)).await });
            cx.run_until_parked();
            let (is_ready, is_error, status) = cx.update(|cx| {
                let view = view.read(cx);
                (
                    view.is_restoring_prechecked && !view.restore_preview_busy,
                    view.is_error,
                    view.status_message
                        .as_ref()
                        .map(ToString::to_string)
                        .unwrap_or_default(),
                )
            });
            ready = is_ready;
            preview_status = status;
            if is_ready || is_error {
                break;
            }
        }
        assert!(
            ready,
            "cancel callback-order preview did not become Ready: {preview_status}"
        );

        let (coordinator, prepared, safety_snapshot) = cx.update(|cx| {
            let view = view.read(cx);
            let prepared = view
                .prepared_restore
                .as_ref()
                .expect("Ready callback-order preview retains PreparedRestore")
                .clone();
            (
                view.restore_coordinator
                    .as_ref()
                    .expect("Ready callback-order preview retains coordinator")
                    .clone(),
                prepared.clone(),
                prepared.safety_backup_path().to_path_buf(),
            )
        });
        let operation_id = prepared.db_instance_operation_id().to_owned();
        let (mut first_ready, first_release, mut first_applied) = cx.update(|cx| {
            view.update(cx, |view, _| {
                view.install_restore_cancel_delivery_interlock_for_test()
            })
        });
        {
            let first_cancel_guard = runtime.enter();
            cx.update(|cx| {
                view.update(cx, |view, cx| {
                    view.cancel_exact_restore_preview(
                        coordinator.clone(),
                        prepared.clone(),
                        None,
                        cx,
                    );
                });
            });
            drop(first_cancel_guard);
        }

        let mut held_success_ready = false;
        for _ in 0..3_000 {
            cx.run_until_parked();
            match first_ready.try_recv() {
                Ok(()) => {
                    held_success_ready = true;
                    break;
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                    panic!("held success interlock closed before becoming ready")
                }
            }
        }
        assert!(
            held_success_ready,
            "first cancel success did not reach its held delivery boundary"
        );
        assert!(settings_restore_live_instances(&target_root).is_empty());
        assert!(!safety_snapshot.exists());

        {
            let duplicate_cancel_guard = runtime.enter();
            cx.update(|cx| {
                view.update(cx, |view, cx| {
                    view.cancel_exact_restore_preview(
                        coordinator.clone(),
                        prepared.clone(),
                        None,
                        cx,
                    );
                });
            });
            drop(duplicate_cancel_guard);
        }
        let mut terminal_observed = false;
        for _ in 0..3_000 {
            cx.run_until_parked();
            let terminal = cx.update(|cx| {
                let view = view.read(cx);
                view.restore_restart_required
                    && view.restore_preview_busy
                    && view
                        .status_message
                        .as_ref()
                        .is_some_and(|status| status.as_ref() == TERMINAL_STATUS)
            });
            if terminal {
                terminal_observed = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            terminal_observed,
            "the captured exact-pair cancel error did not enter the terminal state"
        );
        let (terminal_pair_retained, terminal_dispatches, terminal_status) = cx.update(|cx| {
            let view = view.read(cx);
            (
                view.restore_coordinator.is_some()
                    && view
                        .prepared_restore
                        .as_ref()
                        .is_some_and(|stored| Arc::ptr_eq(stored, &prepared)),
                view.restore_cancel_dispatch_count,
                view.status_message
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_default(),
            )
        });
        assert!(terminal_pair_retained);
        assert_eq!(terminal_dispatches, 2);
        assert_eq!(terminal_status, TERMINAL_STATUS);
        assert!(live_store.pool().is_closed());

        first_release
            .send(())
            .expect("release the earlier successful cancel callback");
        let mut late_success_applied = false;
        for _ in 0..300 {
            cx.run_until_parked();
            match first_applied.try_recv() {
                Ok(()) => {
                    late_success_applied = true;
                    break;
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                    panic!("late success applied interlock closed without delivery")
                }
            }
        }
        assert!(
            late_success_applied,
            "the earlier successful cancel callback was not delivered after terminalization"
        );
        for _ in 0..10 {
            cx.run_until_parked();
            std::thread::sleep(Duration::from_millis(10));
        }

        let (
            terminal_still_required,
            terminal_still_busy,
            exact_pair_still_retained,
            stable_status,
            prechecked,
            confirmed,
            impact,
        ) = cx.update(|cx| {
            let view = view.read(cx);
            (
                view.restore_restart_required,
                view.restore_preview_busy,
                view.restore_coordinator.is_some()
                    && view
                        .prepared_restore
                        .as_ref()
                        .is_some_and(|stored| Arc::ptr_eq(stored, &prepared)),
                view.status_message
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_default(),
                view.is_restoring_prechecked,
                view.is_replacement_confirmed,
                view.restore_impact_preview.clone(),
            )
        });
        assert!(terminal_still_required);
        assert!(
            terminal_still_busy && exact_pair_still_retained,
            "a late success for the captured exact pair must not clear a newer fail-closed terminal"
        );
        assert_eq!(stable_status, TERMINAL_STATUS);
        assert!(!prechecked && !confirmed && impact == "恢复前请先预检");
        assert!(live_store.pool().is_closed());
        let blocked = runtime
            .block_on(live_store.create(
                "settings-must-not-write-after-cancel-error-terminal",
                "127.0.0.1",
                3308,
                "blocked-user",
                b"blocked-password",
            ))
            .expect_err("cancel-error terminal must keep the canonical Store gate closed");
        assert!(blocked.to_string().contains("write_gate_closed"));
        assert!(settings_restore_live_instances(&target_root).is_empty());
        assert!(!safety_snapshot.exists());
        assert_eq!(prepared.db_instance_operation_id(), operation_id);
    }

    #[gpui::test]
    fn set_store_rejects_cross_root_rebind_while_a_preview_outcome_is_in_flight(
        cx: &mut TestAppContext,
    ) {
        const PASSPHRASE: &str = "T130-settings-in-flight-store-rebind-passphrase";

        cx.update(|cx| {
            gpui_component::theme::init(cx);
            gpui_component::init(cx);
        });
        let temp_dir = tempfile::tempdir().expect("create in-flight Store-rebind fixture");
        let source_root = temp_dir.path().join("in-flight-rebind-source");
        let root_a = temp_dir.path().join("in-flight-rebind-root-a");
        let root_b = temp_dir.path().join("in-flight-rebind-root-b");
        let archive = temp_dir.path().join("in-flight-rebind.age.tar");
        let runtime =
            tokio::runtime::Runtime::new().expect("create in-flight Store-rebind Tokio runtime");
        export_settings_data_source_archive(
            &runtime,
            &source_root,
            &archive,
            "settings-in-flight-restored-a",
            PASSPHRASE,
        );
        let store_a = runtime
            .block_on(Store::open_local(StoreOpenOptions::for_root(&root_a)))
            .expect("open root A production Store");
        runtime
            .block_on(store_a.create(
                "settings-in-flight-current-a",
                "127.0.0.1",
                3307,
                "root-a-user",
                b"root-a-password",
            ))
            .expect("seed root A current state");
        let live_store_a = store_a.clone();
        let store_b = runtime
            .block_on(Store::open_local(StoreOpenOptions::for_root(&root_b)))
            .expect("open independent root B Store");
        runtime
            .block_on(store_b.create(
                "settings-in-flight-current-b",
                "127.0.0.1",
                3308,
                "root-b-user",
                b"root-b-password",
            ))
            .expect("seed root B current state");
        let live_store_b = store_b.clone();
        let view = cx.new(|cx| {
            let mut view = SettingsView::new(cx, None);
            view.set_store(store_a);
            view.import_path = archive.to_string_lossy().to_string().into();
            view.backup_passphrase = PASSPHRASE.into();
            view
        });

        cx.dispatcher.allow_parking();
        let runtime_guard = runtime.enter();
        let (mut outcome_ready, outcome_release) = cx.update(|cx| {
            view.update(cx, |view, _| {
                view.install_restore_preview_delivery_interlock_for_test()
            })
        });
        cx.update(|cx| view.update(cx, |view, cx| view.preview_restore(cx)));
        let mut core_outcome_ready = false;
        for _ in 0..3_000 {
            cx.run_until_parked();
            match outcome_ready.try_recv() {
                Ok(()) => {
                    core_outcome_ready = true;
                    break;
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                    panic!("root A preview interlock closed before its outcome was ready")
                }
            }
        }
        assert!(
            core_outcome_ready,
            "root A preview did not reach its held production delivery boundary"
        );
        let a_instances = settings_restore_live_instances(&root_a);
        assert_eq!(a_instances.len(), 1);
        let a_live = a_instances[0].clone();
        let a_manifest: serde_json::Value = serde_json::from_slice(
            &fs::read(a_live.join(".hivegui-db-instance-v1.json"))
                .expect("read held root A preview manifest"),
        )
        .expect("parse held root A preview manifest");
        assert_eq!(a_manifest["ownership_state"], "unarmed");
        let operation_a = a_manifest["db_instance_operation_id"]
            .as_str()
            .expect("held root A preview operation id")
            .to_owned();
        let safety_a = root_a
            .join("backups")
            .join(format!("restore-safety-{operation_a}"));
        assert!(!safety_a.exists());
        let (generation_before_rebind, busy_before_rebind, no_pair_before_delivery) =
            cx.update(|cx| {
                let view = view.read(cx);
                (
                    view.restore_preview_generation,
                    view.restore_preview_busy,
                    view.restore_coordinator.is_none() && view.prepared_restore.is_none(),
                )
            });
        assert!(busy_before_rebind && no_pair_before_delivery);

        cx.update(|cx| {
            view.update(cx, |view, _| view.set_store(store_b.clone()));
        });
        let (
            store_path_after_rebind_attempt,
            generation_after_rebind_attempt,
            busy_after_rebind_attempt,
            no_pair_after_rebind_attempt,
        ) = cx.update(|cx| {
            let view = view.read(cx);
            (
                view.store
                    .as_ref()
                    .expect("Settings remains Store-bound")
                    .database_path()
                    .to_path_buf(),
                view.restore_preview_generation,
                view.restore_preview_busy,
                view.restore_coordinator.is_none() && view.prepared_restore.is_none(),
            )
        });

        outcome_release
            .send(())
            .expect("release the held root A preview outcome");
        let mut a_pair_installed = false;
        for _ in 0..3_000 {
            cx.run_until_parked();
            let installed = cx.update(|cx| {
                let view = view.read(cx);
                view.is_restoring_prechecked
                    && !view.restore_preview_busy
                    && view.restore_coordinator.is_some()
                    && view
                        .prepared_restore
                        .as_ref()
                        .is_some_and(|prepared| prepared.db_instance_operation_id() == operation_a)
            });
            if installed {
                a_pair_installed = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            a_pair_installed,
            "released root A outcome did not install its exact pair"
        );
        let store_path_after_a_install = cx.update(|cx| {
            view.read(cx)
                .store
                .as_ref()
                .expect("Settings remains Store-bound after A delivery")
                .database_path()
                .to_path_buf()
        });
        assert_eq!(settings_restore_live_instances(&root_a), vec![a_live]);
        assert!(settings_restore_live_instances(&root_b).is_empty());
        assert!(!root_b.join("backups").exists());

        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.cancel_completed_restore_preview_after_input_change(cx);
            });
        });
        let mut a_retired = false;
        for _ in 0..3_000 {
            cx.run_until_parked();
            let settled = cx.update(|cx| {
                let view = view.read(cx);
                !view.restore_preview_busy
                    && view.restore_coordinator.is_none()
                    && view.prepared_restore.is_none()
                    && !view.restore_restart_required
            });
            if settled && settings_restore_live_instances(&root_a).is_empty() {
                a_retired = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let final_store_path = cx.update(|cx| {
            view.read(cx)
                .store
                .as_ref()
                .expect("Settings remains bound after A retirement")
                .database_path()
                .to_path_buf()
        });
        drop(runtime_guard);

        let write_a = runtime.block_on(live_store_a.create(
            "settings-write-after-in-flight-rebind-rejection",
            "127.0.0.1",
            3309,
            "after-rejection-user",
            b"after-rejection-password",
        ));
        let root_b_names =
            data_source_names_with_sidecars(&runtime, &root_b.join("datasources.db"));
        let root_a_pool_open = !live_store_a.pool().is_closed();
        let root_b_pool_open = !live_store_b.pool().is_closed();
        runtime.block_on(live_store_a.pool().close());
        runtime.block_on(live_store_b.pool().close());

        let expected_a_database = root_a.join("datasources.db");
        assert_eq!(
            store_path_after_rebind_attempt, expected_a_database,
            "set_store must synchronously reject cross-root replacement while A outcome delivery is in flight"
        );
        assert_eq!(generation_after_rebind_attempt, generation_before_rebind);
        assert!(busy_after_rebind_attempt && no_pair_after_rebind_attempt);
        assert_eq!(
            store_path_after_a_install, expected_a_database,
            "the released A pair must never be installed over a root B Settings binding"
        );
        assert!(
            a_retired,
            "the exact root A pair must retire through real cancel_preview"
        );
        assert_eq!(final_store_path, expected_a_database);
        assert!(!safety_a.exists());
        assert!(settings_restore_live_instances(&root_a).is_empty());
        assert!(settings_restore_live_instances(&root_b).is_empty());
        assert!(!root_b.join("backups").exists());
        assert_eq!(root_b_names, vec!["settings-in-flight-current-b"]);
        assert!(root_a_pool_open && root_b_pool_open);
        write_a.expect("rejected cross-root rebind must leave root A writable after cancellation");
    }

    #[gpui::test]
    fn set_store_is_a_one_time_binding_even_while_restore_is_idle(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::theme::init(cx);
            gpui_component::init(cx);
        });
        let temp_dir = tempfile::tempdir().expect("create idle Store-binding fixture");
        let root_a = temp_dir.path().join("idle-store-root-a");
        let root_b = temp_dir.path().join("idle-store-root-b");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("create idle Store-binding Tokio runtime");
        let store_a = runtime
            .block_on(Store::open_local(StoreOpenOptions::for_root(&root_a)))
            .expect("open idle root A Store");
        runtime
            .block_on(store_a.create(
                "settings-idle-current-a",
                "127.0.0.1",
                3307,
                "root-a-user",
                b"root-a-password",
            ))
            .expect("seed idle root A current state");
        let live_store_a = store_a.clone();
        let store_b = runtime
            .block_on(Store::open_local(StoreOpenOptions::for_root(&root_b)))
            .expect("open idle root B Store");
        runtime
            .block_on(store_b.create(
                "settings-idle-current-b",
                "127.0.0.1",
                3308,
                "root-b-user",
                b"root-b-password",
            ))
            .expect("seed idle root B current state");
        let live_store_b = store_b.clone();
        let view = cx.new(|cx| {
            let mut view = SettingsView::new(cx, None);
            view.set_store(store_a);
            view
        });
        let first_binding = cx.update(|cx| restore_store_binding_snapshot(view.read(cx)));
        assert_eq!(
            first_binding.database_path,
            Some(root_a.join("datasources.db")),
            "the first idle set_store call must bind root A"
        );

        cx.update(|cx| {
            view.update(cx, |view, _| view.set_store(store_b.clone()));
        });
        let after_second_binding = cx.update(|cx| restore_store_binding_snapshot(view.read(cx)));
        let write_a = runtime.block_on(live_store_a.create(
            "settings-idle-write-a",
            "127.0.0.1",
            3309,
            "root-a-write-user",
            b"root-a-write-password",
        ));
        let write_b = runtime.block_on(live_store_b.create(
            "settings-idle-write-b",
            "127.0.0.1",
            3310,
            "root-b-write-user",
            b"root-b-write-password",
        ));
        let root_a_names =
            data_source_names_with_sidecars(&runtime, &root_a.join("datasources.db"));
        let root_b_names =
            data_source_names_with_sidecars(&runtime, &root_b.join("datasources.db"));
        runtime.block_on(live_store_a.pool().close());
        runtime.block_on(live_store_b.pool().close());

        assert_eq!(
            after_second_binding, first_binding,
            "every second set_store call must be synchronously rejected even while restore is completely idle"
        );
        write_a.expect("idle binding rejection must leave root A writable");
        write_b.expect("idle binding rejection must not mutate independent root B");
        assert_eq!(
            root_a_names,
            vec!["settings-idle-current-a", "settings-idle-write-a"]
        );
        assert_eq!(
            root_b_names,
            vec!["settings-idle-current-b", "settings-idle-write-b"]
        );
        assert!(settings_restore_live_instances(&root_a).is_empty());
        assert!(settings_restore_live_instances(&root_b).is_empty());
    }

    #[gpui::test]
    fn set_store_rejects_cross_root_rebind_after_confirmation_terminal(cx: &mut TestAppContext) {
        const PASSPHRASE: &str = "T130-settings-terminal-store-rebind-passphrase";

        cx.update(|cx| {
            gpui_component::theme::init(cx);
            gpui_component::init(cx);
        });
        let temp_dir = tempfile::tempdir().expect("create terminal Store-rebind fixture");
        let source_root = temp_dir.path().join("terminal-rebind-source");
        let root_a = temp_dir.path().join("terminal-rebind-root-a");
        let root_b = temp_dir.path().join("terminal-rebind-root-b");
        let archive = temp_dir.path().join("terminal-rebind.age.tar");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("create terminal Store-rebind Tokio runtime");
        export_settings_data_source_archive(
            &runtime,
            &source_root,
            &archive,
            "settings-terminal-restored-a",
            PASSPHRASE,
        );
        let store_a = runtime
            .block_on(Store::open_local(StoreOpenOptions::for_root(&root_a)))
            .expect("open terminal root A production Store");
        runtime
            .block_on(store_a.create(
                "settings-terminal-old-a",
                "127.0.0.1",
                3307,
                "root-a-user",
                b"root-a-password",
            ))
            .expect("seed terminal root A old current");
        let live_store_a = store_a.clone();
        let store_b = runtime
            .block_on(Store::open_local(StoreOpenOptions::for_root(&root_b)))
            .expect("open independent terminal root B Store");
        runtime
            .block_on(store_b.create(
                "settings-terminal-current-b",
                "127.0.0.1",
                3308,
                "root-b-user",
                b"root-b-password",
            ))
            .expect("seed terminal root B current state");
        let live_store_b = store_b.clone();
        let view = cx.new(|cx| {
            let mut view = SettingsView::new(cx, None);
            view.set_store(store_a);
            view.import_path = archive.to_string_lossy().to_string().into();
            view.backup_passphrase = PASSPHRASE.into();
            view
        });

        cx.dispatcher.allow_parking();
        {
            let preview_guard = runtime.enter();
            cx.update(|cx| view.update(cx, |view, cx| view.preview_restore(cx)));
            drop(preview_guard);
        }
        let mut ready = false;
        let mut preview_status = String::new();
        for _ in 0..3_000 {
            runtime.block_on(async { tokio::time::sleep(Duration::from_millis(10)).await });
            cx.run_until_parked();
            let (is_ready, is_error, status) = cx.update(|cx| {
                let view = view.read(cx);
                (
                    view.is_restoring_prechecked && !view.restore_preview_busy,
                    view.is_error,
                    view.status_message
                        .as_ref()
                        .map(ToString::to_string)
                        .unwrap_or_default(),
                )
            });
            ready = is_ready;
            preview_status = status;
            if is_ready || is_error {
                break;
            }
        }
        assert!(
            ready,
            "terminal Store-rebind preview did not become Ready: {preview_status}"
        );
        let safety_snapshot = cx.update(|cx| {
            view.read(cx)
                .prepared_restore
                .as_ref()
                .expect("Ready terminal fixture retains PreparedRestore")
                .safety_backup_path()
                .to_path_buf()
        });

        {
            let confirmation_guard = runtime.enter();
            cx.update(|cx| {
                view.update(cx, |view, cx| {
                    view.is_replacement_confirmed = true;
                    view.do_import(cx);
                });
            });
            drop(confirmation_guard);
        }
        let mut terminal_complete = false;
        for _ in 0..3_000 {
            runtime.block_on(async { tokio::time::sleep(Duration::from_millis(10)).await });
            cx.run_until_parked();
            let complete = cx.update(|cx| {
                let view = view.read(cx);
                view.restore_restart_required
                    && !view.is_importing
                    && view.restore_coordinator.is_some()
                    && view.prepared_restore.is_none()
            });
            if complete {
                terminal_complete = true;
                break;
            }
        }
        assert!(
            terminal_complete,
            "root A confirmation did not reach its successful restart terminal"
        );
        assert!(live_store_a.pool().is_closed());
        assert!(safety_snapshot.join("manifest.json").is_file());
        assert_eq!(
            data_source_names(&runtime, &root_a.join("datasources.db")),
            vec!["settings-terminal-restored-a"]
        );
        let lock_before_rebind = run_settings_restore_lock_child(&root_a);
        assert_settings_restore_lock_child(
            &lock_before_rebind,
            "root A confirmation terminal must hold its OS Store lock before rebind",
        );
        let terminal_state_before = cx.update(|cx| restore_store_binding_snapshot(view.read(cx)));

        cx.update(|cx| {
            view.update(cx, |view, _| view.set_store(live_store_a.clone()));
        });
        let terminal_state_after_same_root =
            cx.update(|cx| restore_store_binding_snapshot(view.read(cx)));
        let lock_after_same_root = run_settings_restore_lock_child(&root_a);

        cx.update(|cx| {
            view.update(cx, |view, _| view.set_store(store_b.clone()));
        });
        let terminal_state_after_cross_root =
            cx.update(|cx| restore_store_binding_snapshot(view.read(cx)));
        let lock_after_rebind = run_settings_restore_lock_child(&root_a);
        let blocked_a_write = runtime.block_on(live_store_a.create(
            "settings-must-not-write-after-terminal-rebind",
            "127.0.0.1",
            3309,
            "blocked-user",
            b"blocked-password",
        ));
        let write_b = runtime.block_on(live_store_b.create(
            "settings-write-on-independent-root-b",
            "127.0.0.1",
            3310,
            "root-b-write-user",
            b"root-b-write-password",
        ));
        let root_b_names =
            data_source_names_with_sidecars(&runtime, &root_b.join("datasources.db"));
        let root_b_pool_was_open = !live_store_b.pool().is_closed();
        runtime.block_on(live_store_b.pool().close());

        assert!(
            terminal_state_before.restart_required
                && terminal_state_before.coordinator_present
                && terminal_state_before.pool_closed == Some(true)
        );
        assert_eq!(
            terminal_state_before.database_path,
            Some(root_a.join("datasources.db"))
        );
        assert_eq!(
            terminal_state_after_same_root, terminal_state_before,
            "even a same-root Store clone must not clear or rewrite any confirmation-terminal state"
        );
        assert_settings_restore_lock_child(
            &lock_after_same_root,
            "same-root terminal rebind rejection must retain root A OS Store lock",
        );
        assert_eq!(
            terminal_state_after_cross_root, terminal_state_before,
            "cross-root set_store must preserve the complete confirmation-terminal state"
        );
        assert_settings_restore_lock_child(
            &lock_after_rebind,
            "terminal rebind rejection must retain root A OS Store lock",
        );
        let blocked_a_write =
            blocked_a_write.expect_err("root A terminal write gate must remain closed");
        assert!(blocked_a_write.to_string().contains("write_gate_closed"));
        write_b.expect("rejected terminal rebind must leave independent root B writable");
        assert!(root_b_pool_was_open);
        assert_eq!(
            root_b_names,
            vec![
                "settings-terminal-current-b",
                "settings-write-on-independent-root-b"
            ]
        );
        assert!(settings_restore_live_instances(&root_b).is_empty());
        assert!(!root_b.join("backups").exists());
        assert!(safety_snapshot.join("manifest.json").is_file());
    }

    #[test]
    fn restore_confirmation_failure_phase_is_structured_and_not_string_classified() {
        assert_restore_confirmation_safety_phase_source_contract();
    }

    #[gpui::test]
    fn production_restore_pre_safety_failure_reports_not_verified_without_error_text_guessing(
        cx: &mut TestAppContext,
    ) {
        const PASSPHRASE: &str = "T130-settings-pre-safety-phase-passphrase";
        const CORRUPT_STAGING: &[u8] = b"not-a-sqlite-database-pre-safety-sensitive-canary";

        cx.update(|cx| {
            gpui_component::theme::init(cx);
            gpui_component::init(cx);
        });
        let temp_dir = tempfile::tempdir().expect("create pre-safety Settings fixture");
        let source_root = temp_dir.path().join("pre-safety-source");
        let target_root = temp_dir.path().join("pre-safety-target");
        let archive = temp_dir.path().join("pre-safety.age.tar");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("create deterministic pre-safety Tokio runtime");
        let (live_store, view) = settings_restore_failure_fixture(
            cx,
            &runtime,
            &source_root,
            &target_root,
            &archive,
            "settings-pre-safety-restored",
            "settings-pre-safety-current",
            PASSPHRASE,
        );
        wait_for_settings_restore_ready(cx, &runtime, &view);

        let (operation_id, safety_snapshot) = cx.update(|cx| {
            let prepared = view
                .read(cx)
                .prepared_restore
                .as_ref()
                .expect("pre-safety Ready retains PreparedRestore");
            (
                prepared.db_instance_operation_id().to_owned(),
                prepared.safety_backup_path().to_path_buf(),
            )
        });
        let live = target_root
            .join(".hivegui-db-staging-v1")
            .join(format!("restore-{operation_id}"));
        let manifest_path = live.join(".hivegui-db-instance-v1.json");
        let manifest_before = fs::read(&manifest_path).expect("read pre-safety unarmed manifest");
        let manifest: serde_json::Value =
            serde_json::from_slice(&manifest_before).expect("parse pre-safety unarmed manifest");
        assert_eq!(manifest["ownership_state"], "unarmed");
        assert!(!safety_snapshot.exists());
        fs::write(live.join("datasources.db"), CORRUPT_STAGING)
            .expect("replace staged database with deterministic non-SQLite bytes");
        assert_eq!(
            fs::read(live.join("datasources.db")).expect("read corrupt staged database"),
            CORRUPT_STAGING
        );

        let status = run_settings_restore_confirmation(cx, &runtime, &view, &live_store);
        let terminal = cx.update(|cx| {
            let view = view.read(cx);
            (
                view.restore_restart_required,
                view.is_importing,
                view.restore_coordinator.is_some(),
                view.prepared_restore
                    .as_ref()
                    .is_some_and(|prepared| prepared.db_instance_operation_id() == operation_id),
                view.store
                    .as_ref()
                    .expect("pre-safety terminal Store remains bound")
                    .pool()
                    .is_closed(),
                view.is_error,
            )
        });
        assert_eq!(terminal, (true, false, true, true, true, true));
        let blocked_write = runtime
            .block_on(live_store.create(
                "settings-must-not-write-after-pre-safety-failure",
                "127.0.0.1",
                3308,
                "blocked-user",
                b"blocked-password",
            ))
            .expect_err("pre-safety failure must keep the canonical write gate closed");
        assert!(blocked_write.to_string().contains("write_gate_closed"));
        assert_settings_restore_lock_child(
            &run_settings_restore_lock_child(&target_root),
            "pre-safety confirmation failure must retain the production Store OS lock",
        );
        assert!(!safety_snapshot.exists());
        assert_eq!(
            data_source_names_with_sidecars(&runtime, &target_root.join("datasources.db")),
            vec!["settings-pre-safety-current"],
            "pre-safety validation failure must not enter the database switch"
        );
        assert_eq!(
            fs::read(live.join("datasources.db")).expect("reread corrupt staged database"),
            CORRUPT_STAGING
        );
        assert_eq!(
            fs::read(&manifest_path).expect("reread pre-safety manifest"),
            manifest_before
        );
        assert!(!live.join(".hivegui-db-recovery-v1.json").exists());
        assert!(!live.join(".hivegui-db-recovery-v1.json.staging").exists());
        for forbidden in [
            safety_snapshot.to_string_lossy().as_ref(),
            archive.to_string_lossy().as_ref(),
            live.to_string_lossy().as_ref(),
            PASSPHRASE,
            std::str::from_utf8(CORRUPT_STAGING).expect("ASCII corrupt canary"),
            "安全备份已验证",
            "invalid manifest",
            "InvalidManifest",
            "open restore database",
            "restore SQLite health",
            "SQLite format 3",
        ] {
            assert!(
                !status.contains(forbidden),
                "pre-safety status leaked or guessed from internal error text: {status}"
            );
        }
        assert_eq!(status, RESTORE_PRE_SAFETY_FAILURE_STATUS);
    }

    #[gpui::test]
    fn production_restore_post_safety_failure_reports_verified_snapshot_without_error_text_guessing(
        cx: &mut TestAppContext,
    ) {
        const PASSPHRASE: &str = "T130-settings-post-safety-phase-passphrase";

        cx.update(|cx| {
            gpui_component::theme::init(cx);
            gpui_component::init(cx);
        });
        let temp_dir = tempfile::tempdir().expect("create post-safety Settings fixture");
        let source_root = temp_dir.path().join("post-safety-source");
        let target_root = temp_dir.path().join("post-safety-target");
        let archive = temp_dir.path().join("post-safety.age.tar");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("create deterministic post-safety Tokio runtime");
        let (live_store, view) = settings_restore_failure_fixture(
            cx,
            &runtime,
            &source_root,
            &target_root,
            &archive,
            "settings-post-safety-restored",
            "settings-post-safety-current",
            PASSPHRASE,
        );
        wait_for_settings_restore_ready(cx, &runtime, &view);

        let (operation_id, safety_snapshot) = cx.update(|cx| {
            let prepared = view
                .read(cx)
                .prepared_restore
                .as_ref()
                .expect("post-safety Ready retains PreparedRestore");
            (
                prepared.db_instance_operation_id().to_owned(),
                prepared.safety_backup_path().to_path_buf(),
            )
        });
        let live = target_root
            .join(".hivegui-db-staging-v1")
            .join(format!("restore-{operation_id}"));
        let manifest_path = live.join(".hivegui-db-instance-v1.json");
        let mut armed_manifest: serde_json::Value = serde_json::from_slice(
            &fs::read(&manifest_path).expect("read post-safety unarmed manifest"),
        )
        .expect("parse post-safety unarmed manifest");
        assert_eq!(armed_manifest["ownership_state"], "unarmed");
        armed_manifest["ownership_state"] = serde_json::Value::String("armed".into());
        let armed_manifest_bytes =
            serde_json::to_vec(&armed_manifest).expect("serialize deterministic armed manifest");
        fs::write(&manifest_path, &armed_manifest_bytes)
            .expect("preseed deterministic post-safety armed manifest failure");
        assert!(!safety_snapshot.exists());
        assert!(!live.join(".hivegui-db-recovery-v1.json").exists());

        let status = run_settings_restore_confirmation(cx, &runtime, &view, &live_store);
        let terminal = cx.update(|cx| {
            let view = view.read(cx);
            (
                view.restore_restart_required,
                view.is_importing,
                view.restore_coordinator.is_some(),
                view.prepared_restore
                    .as_ref()
                    .is_some_and(|prepared| prepared.db_instance_operation_id() == operation_id),
                view.store
                    .as_ref()
                    .expect("post-safety terminal Store remains bound")
                    .pool()
                    .is_closed(),
                view.is_error,
            )
        });
        assert_eq!(terminal, (true, false, true, true, true, true));
        let blocked_write = runtime
            .block_on(live_store.create(
                "settings-must-not-write-after-post-safety-failure",
                "127.0.0.1",
                3308,
                "blocked-user",
                b"blocked-password",
            ))
            .expect_err("post-safety failure must keep the canonical write gate closed");
        assert!(blocked_write.to_string().contains("write_gate_closed"));
        assert_settings_restore_lock_child(
            &run_settings_restore_lock_child(&target_root),
            "post-safety confirmation failure must retain the production Store OS lock",
        );

        let safety_manifest_path = safety_snapshot.join("manifest.json");
        let safety_database = safety_snapshot.join("datasources.db");
        assert!(safety_manifest_path.is_file());
        assert!(safety_database.is_file());
        let safety_manifest: serde_json::Value = serde_json::from_slice(
            &fs::read(&safety_manifest_path).expect("read verified post-safety manifest"),
        )
        .expect("parse verified post-safety manifest");
        assert_eq!(safety_manifest["schema_version"], 1);
        assert_eq!(safety_manifest["operation_id"], operation_id);
        assert_eq!(safety_manifest["database"]["path"], "datasources.db");
        assert_eq!(
            safety_manifest["database"]["size_bytes"],
            fs::metadata(&safety_database)
                .expect("read verified safety database metadata")
                .len()
        );
        assert_eq!(
            safety_manifest["database"]["sha256"],
            settings_restore_sha256(&safety_database)
        );
        for identity in ["source_identity", "snapshot_identity"] {
            assert!(
                safety_manifest["database"][identity]
                    .as_str()
                    .is_some_and(|value| !value.is_empty()),
                "verified safety manifest must bind {identity}"
            );
        }
        assert_eq!(
            data_source_names(&runtime, &safety_database),
            vec!["settings-post-safety-current"],
            "verified safety database must contain the exact old current state"
        );
        assert_eq!(
            data_source_names_with_sidecars(&runtime, &target_root.join("datasources.db")),
            vec!["settings-post-safety-current"],
            "armed-manifest rejection must occur before the database switch"
        );
        assert_eq!(
            data_source_names(&runtime, &live.join("datasources.db")),
            vec!["settings-post-safety-restored"],
            "post-safety rejection must retain the new database in its live staging instance"
        );
        assert_eq!(
            fs::read(&manifest_path).expect("reread preseeded armed manifest"),
            armed_manifest_bytes
        );
        assert!(
            !live.join("old-datasources.db").exists(),
            "post-safety rejection must occur before creating the database-switch old slot"
        );
        assert!(!live.join(".hivegui-db-recovery-v1.json").exists());
        assert!(!live.join(".hivegui-db-recovery-v1.json.staging").exists());
        for forbidden in [
            archive.to_string_lossy().as_ref(),
            live.to_string_lossy().as_ref(),
            PASSPHRASE,
            "restore manifest cannot be armed",
            "invalid manifest",
            "InvalidManifest",
            "ownership_state",
        ] {
            assert!(
                !status.contains(forbidden),
                "post-safety status leaked or guessed from internal error text: {status}"
            );
        }
        let expected_status = format!(
            "恢复未完成：安全备份已验证：{}；数据切换或收口未完成；Store 已冻结，必须重启 HiveGUI 进行恢复校验",
            safety_snapshot.display()
        );
        assert_eq!(status, expected_status);
    }
}
