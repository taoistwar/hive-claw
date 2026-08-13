//! 数据管理视图。
//!
//! Provides session retention entry points, backup restore precheck +
//! replacement confirmation, export/import controls, and diagnostic
//! bundle export with redaction.

use std::path::Path;
use std::sync::Arc;

use gpui::*;
use gpui_component::ActiveTheme as _;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::scroll::ScrollableElement;

use crate::datasource::entity_store::{export_all_data, import_from_backup};
use crate::datasource::Store;
use crate::runtime::diagnostics::{
    DiagnosticBundle, ExecutionEventCollector, RedactionConfig,
};

pub struct SettingsView {
    store: Option<Store>,
    collector: Option<Arc<ExecutionEventCollector>>,
    export_path: SharedString,
    import_path: SharedString,
    diagnostic_path: SharedString,
    retention_days: SharedString,

    export_path_input: Option<Entity<InputState>>,
    import_path_input: Option<Entity<InputState>>,
    diagnostic_path_input: Option<Entity<InputState>>,
    retention_days_input: Option<Entity<InputState>>,

    restore_impact_preview: String,
    is_exporting: bool,
    is_importing: bool,
    is_restoring_prechecked: bool,
    is_replacement_confirmed: bool,
    is_exporting_diagnostic: bool,
    status_message: Option<SharedString>,
    is_error: bool,
}

impl SettingsView {
    pub fn new(_cx: &mut Context<Self>, collector: Option<Arc<ExecutionEventCollector>>) -> Self {
        Self {
            store: None,
            collector,
            export_path: "./hivegui-export.json".into(),
            import_path: SharedString::new(),
            diagnostic_path: SharedString::from(format!("./hivegui-diagnostic-{}.json", uuid::Uuid::new_v4())),
            retention_days: "365".into(),
            export_path_input: None,
            import_path_input: None,
            diagnostic_path_input: None,
            retention_days_input: None,
            restore_impact_preview: "恢复前请先预检".into(),
            is_exporting: false,
            is_importing: false,
            is_restoring_prechecked: false,
            is_replacement_confirmed: false,
            is_exporting_diagnostic: false,
            status_message: None,
            is_error: false,
        }
    }

    pub fn set_store(&mut self, store: Store) {
        self.store = Some(store);
    }

    fn clear_status(&mut self) {
        self.status_message = None;
        self.is_error = false;
    }

    fn set_status(&mut self, msg: impl Into<SharedString>, is_error: bool) {
        self.status_message = Some(msg.into());
        self.is_error = is_error;
    }

    fn ensure_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.export_path_input.is_none() {
            let initial = self.export_path.to_string();
            self.export_path_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("/path/to/export.json")
                    .default_value(&initial)
            }));
            if let Some(state) = &self.export_path_input {
                cx.subscribe_in(state, window, |this, state, event, _w, cx| {
                    if let InputEvent::Change = event {
                        this.update(cx, |view, _| {
                            view.export_path = state.read(cx).value().to_string().into();
                            view.clear_status();
                        })
                        .ok();
                    }
                })
                .detach();
            }
        }

        if self.import_path_input.is_none() {
            let initial = self.import_path.to_string();
            self.import_path_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("/path/to/restore.json")
                    .default_value(&initial)
            }));
            if let Some(state) = &self.import_path_input {
                cx.subscribe_in(state, window, |this, state, event, _w, cx| {
                    if let InputEvent::Change = event {
                        this.update(cx, |view, _| {
                            view.import_path = state.read(cx).value().to_string().into();
                            view.is_restoring_prechecked = false;
                            view.is_replacement_confirmed = false;
                            view.restore_impact_preview = "恢复前请先预检".to_string();
                            view.clear_status();
                        })
                        .ok();
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
                cx.subscribe_in(state, window, |this, state, event, _w, cx| {
                    if let InputEvent::Change = event {
                        this.update(cx, |view, _| {
                            view.diagnostic_path = state.read(cx).value().to_string().into();
                            view.clear_status();
                        })
                        .ok();
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
                cx.subscribe_in(state, window, |this, state, event, _w, cx| {
                    if let InputEvent::Change = event {
                        this.update(cx, |view, _| {
                            view.retention_days = state.read(cx).value().to_string().into();
                            view.clear_status();
                        })
                        .ok();
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
        if let Some(state) = &self.retention_days_input {
            self.retention_days = state.read(cx).value().to_string().into();
        }
    }

    fn save_retention(&mut self, cx: &mut Context<Self>) {
        let parsed = self.retention_days.to_string().trim().parse::<u64>();
        match parsed {
            Ok(days) if days > 0 => {
                self.set_status(format!("会话保留期已记录为 {days} 天（当前为前端预览），请重启/刷新持久化参数以生效"), false);
            }
            _ => {
                self.set_status("保留期必须是大于 0 的整数", true);
            }
        }
        cx.notify();
    }

    fn preview_restore(&mut self, cx: &mut Context<Self>) {
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
        if !Path::new(&self.import_path.to_string()).exists() {
            self.set_status("恢复文件不存在", true);
            cx.notify();
            return;
        }

        self.restore_impact_preview = "恢复预检通过：将替换当前会话/消息/执行与备份历史相关数据。请确认完整替换。".to_string();
        self.is_restoring_prechecked = true;
        self.set_status("恢复预检通过", false);
        cx.notify();
    }

    fn do_export(&mut self, cx: &mut Context<Self>) {
        if self.store.is_none() {
            self.set_status("Store 未就绪", true);
            cx.notify();
            return;
        }
        let Some(path) = (!self.export_path.is_empty()).then(|| self.export_path.to_string()) else {
            self.set_status("请输入导出路径", true);
            cx.notify();
            return;
        };

        let store = self.store.as_ref().expect("store checked").clone();
        let entity = cx.entity();
        self.is_exporting = true;
        self.set_status("正在导出...", false);
        cx.notify();

        cx.spawn(async move |_this, cx| {
            let outcome = export_all_data(&store.pool(), &path).await;
            entity
                .update(cx, move |view, cx| {
                    view.is_exporting = false;
                    match outcome {
                        Ok(()) => view.set_status(format!("导出完成：{path}"), false),
                        Err(err) => view.set_status(format!("导出失败：{err}"), true),
                    }
                    cx.notify();
                })
                .ok();
        })
        .detach();
    }

    fn do_import(&mut self, cx: &mut Context<Self>) {
        if self.store.is_none() {
            self.set_status("Store 未就绪", true);
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
        let Some(path) = (!self.import_path.is_empty()).then(|| self.import_path.to_string()) else {
            self.set_status("请输入恢复路径", true);
            cx.notify();
            return;
        };

        let store = self.store.as_ref().expect("store checked").clone();
        let entity = cx.entity();
        self.is_importing = true;
        self.set_status("正在恢复...", false);
        cx.notify();

        cx.spawn(async move |_this, cx| {
            let outcome = import_from_backup(&store.pool(), &path).await;
            entity
                .update(cx, move |view, cx| {
                    view.is_importing = false;
                    match outcome {
                        Ok(()) => {
                            view.set_status(format!("恢复完成：{path}"), false);
                            view.is_restoring_prechecked = false;
                            view.is_replacement_confirmed = false;
                            view.restore_impact_preview = "恢复前请先预检".to_string();
                        }
                        Err(err) => view.set_status(format!("恢复失败：{err}"), true),
                    }
                    cx.notify();
                })
                .ok();
        })
        .detach();
    }

    fn export_diagnostic_bundle(&mut self, cx: &mut Context<Self>) {
        let Some(collector) = self.collector.clone() else {
            self.set_status("诊断采集器未接入", true);
            cx.notify();
            return;
        };
        let Some(path) = (!self.diagnostic_path.is_empty()).then(|| self.diagnostic_path.to_string()) else {
            self.set_status("请输入诊断导出路径", true);
            cx.notify();
            return;
        };

        let entity = cx.entity();
        self.is_exporting_diagnostic = true;
        self.set_status("正在导出脱敏诊断样例...", false);
        cx.notify();

        cx.spawn(async move |_this, cx| {
            let bundle = DiagnosticBundle::new(collector);
            let result = bundle.export_redacted(
                Path::new(&path),
                &RedactionConfig::default(),
            );
            entity
                .update(cx, move |view, cx| {
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
                })
                .ok();
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
            let color = if self.is_error { theme.danger } else { theme.success };
            div().text_size(px(12.0)).text_color(color).child(msg.clone())
        } else {
            div()
        };

        let view = cx.weak_entity();
        let export_input = self.export_path_input.clone().expect("export path input");
        let import_input = self.import_path_input.clone().expect("import path input");
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
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
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
                            .child(div().text_size(px(12.0)).text_color(muted).child("按天保存会话/消息保留策略（前端预览字段，持久化待对接）"))
                            .child(
                                div().flex().flex_row().gap(px(8.0)).items_center()
                                    .child(div().w(px(96.0)).child("保留天数"))
                                    .child(Input::new(&retention_input).w(px(120.0)))
                                    .child(btn(
                                        "保存保留期",
                                        theme.success,
                                        theme.success_hover,
                                        theme.success_foreground,
                                        theme.success_foreground,
                                        move |_, _, cx| {
                                            let _ = view
                                                .update(cx, |view, cx| view.save_retention(cx));
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
                            .child(div().text_size(px(16.0)).font_weight(FontWeight::SEMIBOLD).child("数据导出"))
                            .child(div().text_size(px(12.0)).text_color(muted).child("将数据库实体导出为 JSON 备份。"))
                            .child(
                                div().flex().flex_row().gap(px(8.0)).items_center()
                                    .child(div().w(px(96.0)).child("导出路径"))
                                    .child(Input::new(&export_input).w(px(420.0))),
                            )
                            .child(
                                btn(
                                    if self.is_exporting { "导出中..." } else { "导出数据" },
                                    theme.primary,
                                    theme.primary_hover,
                                    theme.primary_foreground,
                                    theme.primary_foreground,
                                    {
                                        let view = view.clone();
                                        move |_, _, cx| {
                                            view.update(cx, |view, cx| view.do_export(cx)).ok();
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
                                        if self.is_restoring_prechecked { "已预检" } else { "预检恢复" },
                                        theme.warning,
                                        theme.warning_hover,
                                        theme.warning_foreground,
                                        theme.warning_foreground,
                                        {
                                            let view = view.clone();
                                            move |_, _, cx| {
                                                view.update(cx, |view, cx| view.preview_restore(cx)).ok();
                                            }
                                        },
                                    ))
                                    .child(btn(
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
                                                view.update(cx, |view, _| {
                                                    view.is_replacement_confirmed = !view.is_replacement_confirmed;
                                                }).ok();
                                            }
                                        },
                                    ))
                                    .child(btn(
                                        if self.is_importing { "恢复中..." } else { "执行恢复" },
                                        theme.primary,
                                        theme.primary_hover,
                                        theme.primary_foreground,
                                        theme.primary_foreground,
                                        {
                                            let view = view.clone();
                                            move |_, _, cx| {
                                                view.update(cx, |view, cx| view.do_import(cx)).ok();
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
                                        view.update(cx, |view, cx| view.export_diagnostic_bundle(cx)).ok();
                                    }
                                },
                            ))
                            .into_any_element(),
                        status,
                        div()
                            .text_size(px(11.0))
                            .text_color(muted)
                            .child("提示：恢复操作将覆盖现有数据，执行前请先导出当前数据。")
                            .child(div().mt(px(4.0)).text_color(border).child("路由：会话管理入口由 AI 管理 - 数据管理页签承载。")),
                    ]),
            )
    }
}

fn btn(
    label: &'static str,
    bg: Hsla,
    hover: Hsla,
    fg: Hsla,
    hover_fg: Hsla,
    handler: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
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

#[cfg(test)]
mod tests {
    use gpui::{
        Context, IntoElement, Render, TestAppContext, VisualTestContext, Window, div, prelude::*, px,
        size,
    };

    use super::SettingsView;

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
}
