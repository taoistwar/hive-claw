//! 数据管理视图 - 数据导出/导入功能 (FR-026)
use crate::datasource::Store;
use crate::datasource::entity_store::{export_all_data, import_from_backup};
use gpui::*;
use gpui_component::ActiveTheme as _;
use gpui_component::input::{Input, InputState};
use gpui_component::scroll::ScrollableElement;

pub struct SettingsView {
    store: Option<Store>,
    export_path_input: Option<Entity<InputState>>,
    import_path_input: Option<Entity<InputState>>,
    export_path: SharedString,
    import_path: SharedString,
    status_message: Option<SharedString>,
    is_error: bool,
    is_exporting: bool,
    is_importing: bool,
}

impl SettingsView {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        SettingsView {
            store: None,
            export_path_input: None,
            import_path_input: None,
            export_path: "".into(),
            import_path: "".into(),
            status_message: None,
            is_error: false,
            is_exporting: false,
            is_importing: false,
        }
    }

    pub fn set_store(&mut self, store: Store) {
        self.store = Some(store);
    }

    fn do_export(&mut self, cx: &mut Context<Self>) {
        let path = self.export_path.to_string();
        if path.is_empty() {
            self.status_message = Some("请输入导出文件路径".into());
            self.is_error = true;
            cx.notify();
            return;
        }

        if let Some(ref store) = self.store {
            let pool = store.pool().clone();
            let entity = cx.entity();
            self.is_exporting = true;
            self.status_message = Some("正在导出数据...".into());
            self.is_error = false;
            cx.notify();

            cx.spawn(async move |_this, cx| {
                let result = export_all_data(&pool, &path).await;
                _ = entity.update(cx, |this, cx| {
                    this.is_exporting = false;
                    match result {
                        Ok(()) => {
                            this.status_message =
                                Some(format!("数据已成功导出到: {}", path).into());
                            this.is_error = false;
                        }
                        Err(e) => {
                            this.status_message = Some(format!("导出失败: {}", e).into());
                            this.is_error = true;
                        }
                    }
                    cx.notify();
                });
            })
            .detach();
        }
    }

    fn do_import(&mut self, cx: &mut Context<Self>) {
        let path = self.import_path.to_string();
        if path.is_empty() {
            self.status_message = Some("请输入导入文件路径".into());
            self.is_error = true;
            cx.notify();
            return;
        }

        if let Some(ref store) = self.store {
            let pool = store.pool().clone();
            let entity = cx.entity();
            self.is_importing = true;
            self.status_message = Some("正在导入数据...".into());
            self.is_error = false;
            cx.notify();

            cx.spawn(async move |_this, cx| {
                let result = import_from_backup(&pool, &path).await;
                _ = entity.update(cx, |this, cx| {
                    this.is_importing = false;
                    match result {
                        Ok(()) => {
                            this.status_message =
                                Some(format!("数据已成功从 {} 导入", path).into());
                            this.is_error = false;
                        }
                        Err(e) => {
                            this.status_message = Some(format!("导入失败: {}", e).into());
                            this.is_error = true;
                        }
                    }
                    cx.notify();
                });
            })
            .detach();
        }
    }
}

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 初始化输入框
        if self.export_path_input.is_none() {
            self.export_path_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("/path/to/export.json")
                    .default_value(&self.export_path.to_string())
            }));
        }
        if self.import_path_input.is_none() {
            self.import_path_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("/path/to/import.json")
                    .default_value(&self.import_path.to_string())
            }));
        }

        // 同步输入框值
        if let Some(ref inp) = self.export_path_input {
            self.export_path = inp.read(cx).value().to_string().into();
        }
        if let Some(ref inp) = self.import_path_input {
            self.import_path = inp.read(cx).value().to_string().into();
        }

        let export_input = self.export_path_input.clone().unwrap();
        let import_input = self.import_path_input.clone().unwrap();
        let background = cx.theme().background;
        let foreground = cx.theme().foreground;
        let border = cx.theme().border;
        let group_box = cx.theme().group_box;
        let group_box_foreground = cx.theme().group_box_foreground;
        let muted_foreground = cx.theme().muted_foreground;
        let primary = cx.theme().primary;
        let primary_hover = cx.theme().primary_hover;
        let primary_foreground = cx.theme().primary_foreground;
        let warning = cx.theme().warning;
        let warning_hover = cx.theme().warning_hover;
        let warning_foreground = cx.theme().warning_foreground;
        let danger = cx.theme().danger;
        let success = cx.theme().success;

        let status_display = if let Some(ref msg) = self.status_message {
            let color = if self.is_error { danger } else { success };
            div()
                .text_size(px(13.0))
                .text_color(color)
                .child(msg.clone())
                .into_any_element()
        } else {
            div().into_any_element()
        };

        div().flex().flex_col().size_full().bg(background).text_color(foreground)
            .child(
                div().flex().items_center().justify_between().p(px(16.0)).border_b_1().border_color(border)
                    .child(div().text_size(px(18.0)).font_weight(FontWeight::BOLD).child("数据管理"))
                    .child(div())
            )
            .child(div().flex().flex_col().flex_1().min_h_0().overflow_y_scrollbar().p(px(24.0)).gap(px(24.0))
                // 数据导出部分
                .child(div().flex().flex_col().gap(px(12.0)).p(px(16.0)).bg(group_box).text_color(group_box_foreground).rounded(px(8.0))
                    .child(div().text_size(px(16.0)).font_weight(FontWeight::SEMIBOLD).child("数据导出"))
                    .child(div().text_size(px(12.0)).text_color(muted_foreground)
                        .child("将所有实体数据（标签、分类、能力、插件、函数、工作流、工具、技能、Agent）导出为 JSON 文件"))
                    .child(div().flex().flex_row().gap(px(8.0)).items_center()
                        .child(div().w(px(100.0)).text_size(px(13.0)).child("导出路径:"))
                        .child(div().flex_1().child(Input::new(&export_input))))
                    .child(div().flex().flex_row().gap(px(8.0)).justify_end()
                        .child(btn(if self.is_exporting { "导出中..." } else { "导出数据" }, primary, primary_hover, primary_foreground,
                            cx.listener(|this, _, _, cx| {
                                if !this.is_exporting { this.do_export(cx); }
                            })))))

                // 数据导入部分
                .child(div().flex().flex_col().gap(px(12.0)).p(px(16.0)).bg(group_box).text_color(group_box_foreground).rounded(px(8.0))
                    .child(div().text_size(px(16.0)).font_weight(FontWeight::SEMIBOLD).child("数据导入"))
                    .child(div().text_size(px(12.0)).text_color(muted_foreground)
                        .child("从 JSON 备份文件导入数据。注意：导入会覆盖现有数据，请确保备份文件来自可信来源"))
                    .child(div().flex().flex_row().gap(px(8.0)).items_center()
                        .child(div().w(px(100.0)).text_size(px(13.0)).child("导入路径:"))
                        .child(div().flex_1().child(Input::new(&import_input))))
                    .child(div().flex().flex_row().gap(px(8.0)).justify_end()
                        .child(btn(if self.is_importing { "导入中..." } else { "导入数据" }, warning, warning_hover, warning_foreground,
                            cx.listener(|this, _, _, cx| {
                                if !this.is_importing { this.do_import(cx); }
                            })))))

                // 状态消息
                .child(status_display)

                // 提示信息
                .child(div().flex().flex_col().gap(px(8.0)).p(px(12.0)).bg(warning.opacity(0.15)).rounded(px(6.0)).border_1().border_color(warning)
                    .child(div().text_size(px(13.0)).font_weight(FontWeight::SEMIBOLD).text_color(warning).child("注意事项"))
                    .child(div().text_size(px(12.0)).text_color(foreground)
                        .child("• 导出文件将包含所有实体数据的完整备份"))
                    .child(div().text_size(px(12.0)).text_color(foreground)
                        .child("• 导入操作会先创建当前数据的备份，然后再执行导入"))
                    .child(div().text_size(px(12.0)).text_color(foreground)
                        .child("• 导入的 JSON 文件必须符合指定的格式要求"))
                    .child(div().text_size(px(12.0)).text_color(foreground)
                        .child("• 建议在导入前手动备份重要数据")))
            )
    }
}

fn btn(
    label: &'static str,
    bg: Hsla,
    hover: Hsla,
    fg: Hsla,
    handler: impl Fn(&gpui::MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .px(px(16.0))
        .py(px(8.0))
        .bg(bg)
        .rounded(px(6.0))
        .text_size(px(13.0))
        .text_color(fg)
        .cursor(CursorStyle::PointingHand)
        .hover(move |s| s.bg(hover))
        .on_mouse_down(MouseButton::Left, handler)
        .child(label)
}

#[cfg(test)]
mod tests {
    use gpui::{
        Context, IntoElement, Render, TestAppContext, VisualTestContext, Window, div, prelude::*,
        px, size,
    };

    struct SettingsLayoutTestView;

    impl Render for SettingsLayoutTestView {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .flex()
                .flex_col()
                .size_full()
                .child(div().h(px(48.0)))
                .child(
                    div().flex_1().child(
                        div()
                            .size_full()
                            .debug_selector(|| "SETTINGS_CONTENT".to_owned()),
                    ),
                )
        }
    }

    #[gpui::test]
    fn settings_content_is_constrained_to_the_available_height(cx: &mut TestAppContext) {
        let window = cx.open_window(size(px(800.0), px(480.0)), |_, _| SettingsLayoutTestView);
        cx.run_until_parked();

        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let content = cx
            .debug_bounds("SETTINGS_CONTENT")
            .expect("settings content bounds");

        assert_eq!(content.top(), px(48.0));
        assert_eq!(content.bottom(), px(480.0));
    }
}
