use crate::datasource::workflow_store::WorkflowStore;
use crate::datasource::workflow_store::WorkflowStoreError;
use crate::datasource::{Store, entity_store::Workflow};
use crate::runtime::{
    CancelHandle, LocalWorkflowNodeExecutor, WorkflowExecutor,
    WorkflowNodeStatus as RuntimeNodeStatus, WorkflowRunStatus,
};
use crate::ui::dag_editor_view::DagEditorView;
use crate::ui::management_style::{
    ActionRole, ActionSize, ManagementStyle, action_button, list_actions, list_cell,
    list_container, list_header, list_header_cell, list_row, management_modal_layer,
    management_modal_panel, management_modal_scroll,
};
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::ActiveTheme as _;
use gpui_component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_component::scroll::ScrollableElement;

/// 单个节点的执行诊断（T098：节点级诊断）。
#[derive(Debug, Clone)]
struct NodeDiagnostic {
    node_key: String,
    status: NodeRunStatus,
    output: Option<String>,
    error: Option<String>,
    elapsed_ms: u64,
}

/// 节点执行状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NodeRunStatus {
    Completed,
    Failed,
    TimedOut,
    NotStarted,
    Cancelled,
}

impl NodeRunStatus {
    fn label(self) -> &'static str {
        match self {
            Self::Completed => "完成",
            Self::Failed => "失败",
            Self::TimedOut => "超时",
            Self::NotStarted => "未执行",
            Self::Cancelled => "取消",
        }
    }
}

/// 一次 workflow 后台执行的状态（T098：后台执行/停止 + 节点诊断 + 副作用提示）。
struct WorkflowRunState {
    workflow_id: i64,
    workflow_name: String,
    status: String,
    cancel: CancelHandle,
    diagnostics: Vec<NodeDiagnostic>,
    failed_node: Option<String>,
    failed_reason: Option<String>,
    side_effects_may_have_occurred: bool,
    elapsed_ms: u64,
}

pub struct WorkflowView {
    store: Entity<Store>,
    items: Vec<Workflow>,
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
    form_timeout_ms: String,
    form_category_id: String,
    form_input_schema: String,
    form_start_description: String,
    form_output_schema: String,
    form_required_capabilities: String,
    error_message: Option<String>,
    identifier_conflict: Option<String>,
    confirm_delete_id: Option<i64>,
    confirm_delete_conflict: Option<WorkflowStoreError>,
    identifier_input: Option<Entity<InputState>>,
    name_input: Option<Entity<InputState>>,
    description_input: Option<Entity<InputState>>,
    timeout_input: Option<Entity<InputState>>,
    category_id_input: Option<Entity<InputState>>,
    input_schema_input: Option<Entity<TextareaState>>,
    start_description_input: Option<Entity<TextareaState>>,
    output_schema_input: Option<Entity<TextareaState>>,
    required_capabilities_input: Option<Entity<InputState>>,
    search_input: Option<Entity<InputState>>,
    // DAG 编辑器状态
    show_dag_editor: bool,
    dag_editor_workflow_id: Option<i64>,
    dag_editor_view: Option<Entity<DagEditorView>>,
    form_focus: FocusHandle,
    add_focus: FocusHandle,
    error_focus: FocusHandle,
    // 后台执行状态（T098）
    run_state: Option<WorkflowRunState>,
    show_run_panel: bool,
    run_scroll: ScrollHandle,
}

impl WorkflowView {
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
            form_timeout_ms: "30000".into(),
            form_category_id: String::new(),
            form_input_schema: String::new(),
            form_start_description: String::new(),
            form_output_schema: String::new(),
            form_required_capabilities: String::new(),
            error_message: None,
            identifier_conflict: None,
            confirm_delete_id: None,
            confirm_delete_conflict: None,
            identifier_input: None,
            name_input: None,
            description_input: None,
            timeout_input: None,
            category_id_input: None,
            input_schema_input: None,
            start_description_input: None,
            output_schema_input: None,
            required_capabilities_input: None,
            search_input: None,
            show_dag_editor: false,
            dag_editor_workflow_id: None,
            dag_editor_view: None,
            form_focus: cx.focus_handle(),
            add_focus: cx.focus_handle(),
            error_focus: cx.focus_handle(),
            run_state: None,
            show_run_panel: false,
            run_scroll: ScrollHandle::default(),
        };
        v.load(cx);
        v
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
            let items = Workflow::list(store.pool(), search.clone(), 20, offset).await?;
            let count = Workflow::count(store.pool(), search).await?;
            this.update(cx, |v, cx| {
                v.items = items;
                v.total_count = count;
                v.loading = false;
                cx.notify();
            })
        })
        .detach();
    }

    fn show_add_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_form = true;
        self.editing_id = None;
        self.form_identifier.clear();
        self.form_name.clear();
        self.form_description.clear();
        self.form_timeout_ms = "30000".into();
        self.form_category_id.clear();
        self.form_input_schema.clear();
        self.form_start_description.clear();
        self.form_output_schema.clear();
        self.form_required_capabilities.clear();
        self.error_message = None;
        self.identifier_conflict = None;
        self.confirm_delete_id = None;
        self.confirm_delete_conflict = None;

        // 初始化输入框
        self.identifier_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("唯一标识符")
                .default_value("")
        }));
        self.name_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("工作流名称")
                .default_value("")
        }));
        self.description_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("描述（可选）")
                .default_value("")
        }));
        self.timeout_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("30000")
                .default_value("30000")
        }));
        self.category_id_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("分类 ID（可选）")
                .default_value("")
        }));
        self.input_schema_input = Some(cx.new(|cx| {
            let mut state = TextareaState::new(window, cx).placeholder("输入 Schema（JSON，可选）");
            state.set_value(self.form_input_schema.clone(), window, cx);
            state
        }));
        self.start_description_input = Some(cx.new(|cx| {
            let mut state = TextareaState::new(window, cx).placeholder("起始提示（可选）");
            state.set_value(self.form_start_description.clone(), window, cx);
            state
        }));
        self.output_schema_input = Some(cx.new(|cx| {
            let mut state = TextareaState::new(window, cx).placeholder("输出 Schema（JSON，可选）");
            state.set_value(self.form_output_schema.clone(), window, cx);
            state
        }));
        self.required_capabilities_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("必需能力（可选，JSON 或以逗号分隔）")
                .default_value("")
        }));

        cx.notify();
    }

    fn show_edit_form(&mut self, window: &mut Window, item: Workflow, cx: &mut Context<Self>) {
        self.show_form = true;
        self.editing_id = Some(item.id);
        self.form_identifier = item.identifier.clone();
        self.form_name = item.name.clone();
        self.form_description = item.description.clone().unwrap_or_default();
        self.form_timeout_ms = item.timeout_ms.to_string();
        self.form_category_id = item
            .category_id
            .map(|id| id.to_string())
            .unwrap_or_default();
        self.form_input_schema = item.input_schema.unwrap_or_default();
        self.form_start_description = item.start_description.unwrap_or_default();
        self.form_output_schema = item.output_schema.unwrap_or_default();
        self.form_required_capabilities = item.required_capabilities.unwrap_or_default();
        self.error_message = None;
        self.identifier_conflict = None;
        self.confirm_delete_id = None;
        self.confirm_delete_conflict = None;

        // 初始化输入框
        self.identifier_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("唯一标识符")
                .default_value(&item.identifier)
        }));
        self.name_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("工作流名称")
                .default_value(&item.name)
        }));
        self.description_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("描述（可选）")
                .default_value(item.description.unwrap_or_default())
        }));
        self.timeout_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("30000")
                .default_value(item.timeout_ms.to_string())
        }));
        self.category_id_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("分类 ID（可选）")
                .default_value(self.form_category_id.as_str())
        }));
        self.input_schema_input = Some(cx.new(|cx| {
            let mut state = TextareaState::new(window, cx).placeholder("输入 Schema（JSON，可选）");
            state.set_value(self.form_input_schema.clone(), window, cx);
            state
        }));
        self.start_description_input = Some(cx.new(|cx| {
            let mut state = TextareaState::new(window, cx).placeholder("起始提示（可选）");
            state.set_value(self.form_start_description.clone(), window, cx);
            state
        }));
        self.output_schema_input = Some(cx.new(|cx| {
            let mut state = TextareaState::new(window, cx).placeholder("输出 Schema（JSON，可选）");
            state.set_value(self.form_output_schema.clone(), window, cx);
            state
        }));
        self.required_capabilities_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("必需能力（可选，JSON 或以逗号分隔）")
                .default_value(self.form_required_capabilities.as_str())
        }));

        cx.notify();
    }

    /// Keyboard handler for the workflow form. Esc closes the form,
    /// Enter submits when the form is visible.
    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.show_form {
            return;
        }
        match event.keystroke.key.as_str() {
            "escape" => self.hide_form(window, cx),
            "enter" => self.save(window, cx),
            _ => {}
        }
    }

    fn hide_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.form_scroll.set_offset(point(px(0.0), px(0.0)));
        self.show_form = false;
        self.editing_id = None;
        self.error_message = None;
        self.identifier_conflict = None;
        self.add_focus.focus(window, cx);
        cx.notify();
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // 同步输入框值
        if let Some(ref inp) = self.identifier_input {
            self.form_identifier = inp.read(cx).value().to_string();
        }
        if let Some(ref inp) = self.name_input {
            self.form_name = inp.read(cx).value().to_string();
        }
        if let Some(ref inp) = self.description_input {
            self.form_description = inp.read(cx).value().to_string();
        }
        if let Some(ref inp) = self.timeout_input {
            self.form_timeout_ms = inp.read(cx).value().to_string();
        }
        if let Some(ref inp) = self.category_id_input {
            self.form_category_id = inp.read(cx).value().to_string();
        }
        if let Some(ref inp) = self.input_schema_input {
            self.form_input_schema = inp.read(cx).value().to_string();
        }
        if let Some(ref inp) = self.start_description_input {
            self.form_start_description = inp.read(cx).value().to_string();
        }
        if let Some(ref inp) = self.output_schema_input {
            self.form_output_schema = inp.read(cx).value().to_string();
        }
        if let Some(ref inp) = self.required_capabilities_input {
            self.form_required_capabilities = inp.read(cx).value().to_string();
        }

        if self.form_identifier.trim().is_empty() || self.form_name.trim().is_empty() {
            self.error_message = Some("Identifier 和名称不能为空".into());
            self.identifier_conflict = None;
            self.error_focus.focus(window, cx);
            cx.notify();
            return;
        }
        let timeout: i64 = self.form_timeout_ms.parse().unwrap_or(30000);
        let category_id = if self.form_category_id.trim().is_empty() {
            None
        } else {
            match self.form_category_id.trim().parse::<i64>() {
                Ok(v) => Some(v),
                Err(_) => {
                    self.error_message = Some("分类 ID 必须是数字".into());
                    self.identifier_conflict = None;
                    self.error_focus.focus(window, cx);
                    cx.notify();
                    return;
                }
            }
        };
        let input_schema = if self.form_input_schema.trim().is_empty() {
            None
        } else {
            Some(self.form_input_schema.clone())
        };
        let start_description = if self.form_start_description.trim().is_empty() {
            None
        } else {
            Some(self.form_start_description.clone())
        };
        let output_schema = if self.form_output_schema.trim().is_empty() {
            None
        } else {
            Some(self.form_output_schema.clone())
        };
        let required_capabilities = if self.form_required_capabilities.trim().is_empty() {
            None
        } else {
            Some(self.form_required_capabilities.clone())
        };
        let store = self.store.read(cx).clone();
        let idf = self.form_identifier.clone();
        let name = self.form_name.clone();
        let desc = if self.form_description.is_empty() {
            None
        } else {
            Some(self.form_description.clone())
        };

        if let Some(eid) = self.editing_id {
            cx.spawn_in(window, async move |this, cx| {
                let result = Workflow::update(
                    store.pool(),
                    eid,
                    idf.clone(),
                    name,
                    desc,
                    timeout,
                    category_id,
                    input_schema,
                    start_description,
                    output_schema,
                    required_capabilities,
                )
                .await;
                _ = cx.update(|window, cx| {
                    _ = this.update(cx, |v, cx| match result {
                        Ok(_) => {
                            v.hide_form(window, cx);
                            v.load(cx);
                        }
                        Err(error) => {
                            v.publish_save_error("更新失败", &idf, &error, window, cx);
                        }
                    });
                });
            })
            .detach();
        } else {
            cx.spawn_in(window, async move |this, cx| {
                let result = Workflow::create(
                    store.pool(),
                    idf.clone(),
                    name,
                    desc,
                    timeout,
                    category_id,
                    input_schema,
                    start_description,
                    output_schema,
                    required_capabilities,
                )
                .await;
                _ = cx.update(|window, cx| {
                    _ = this.update(cx, |v, cx| match result {
                        Ok(_) => {
                            v.hide_form(window, cx);
                            v.load(cx);
                        }
                        Err(error) => {
                            v.publish_save_error("创建失败", &idf, &error, window, cx);
                        }
                    });
                });
            })
            .detach();
        }
    }

    fn publish_save_error(
        &mut self,
        prefix: &str,
        identifier: &str,
        error: &anyhow::Error,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let diagnostic = error.to_string();
        let duplicate_identifier = diagnostic.contains("identifier")
            && (diagnostic.contains("已存在")
                || diagnostic.contains("UNIQUE constraint failed")
                || diagnostic.to_ascii_lowercase().contains("duplicate"));
        self.identifier_conflict = duplicate_identifier.then(|| identifier.to_string());
        self.error_message = Some(if duplicate_identifier {
            format!("{prefix}: identifier 已存在，请使用其他值")
        } else {
            format!("{prefix}: 工作流数据无效或暂时无法保存")
        });
        self.error_focus.focus(window, cx);
        cx.notify();
    }

    fn delete(&mut self, id: i64, cx: &mut Context<Self>) {
        let store = self.store.read(cx).clone();
        cx.spawn(async move |this, cx| {
            let store = match WorkflowStore::new(store.pool().clone()) {
                Ok(s) => s,
                Err(e) => {
                    this.update(cx, |v, cx| {
                        v.confirm_delete_conflict = Some(e);
                        cx.notify();
                    })
                    .ok();
                    return;
                }
            };
            match store.delete(id).await {
                Ok(_) => {
                    this.update(cx, |v, cx| {
                        v.confirm_delete_conflict = None;
                        v.confirm_delete_id = None;
                        v.load(cx);
                    })
                    .ok();
                }
                Err(e) => {
                    this.update(cx, |v, cx| {
                        v.confirm_delete_conflict = Some(e);
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

    fn show_dag_editor(&mut self, workflow_id: i64, cx: &mut Context<Self>) {
        self.show_dag_editor = true;
        self.dag_editor_workflow_id = Some(workflow_id);
        self.dag_editor_view =
            Some(cx.new(|cx| DagEditorView::new(self.store.clone(), workflow_id, cx)));
        cx.notify();
    }

    fn hide_dag_editor(&mut self, cx: &mut Context<Self>) {
        self.show_dag_editor = false;
        self.dag_editor_workflow_id = None;
        self.dag_editor_view = None;
        cx.notify();
    }

    /// 后台执行一个 workflow（T098：不阻塞主 UI，提供停止入口）。
    fn run_workflow(&mut self, workflow_id: i64, cx: &mut Context<Self>) {
        let store = self.store.read(cx).clone();
        let pool = store.pool().clone();
        let plugin_root = store.plugin_root().to_path_buf();
        let crypto = store.crypto().clone();

        let cancel = CancelHandle::new();
        let cancel_for_ui = cancel.clone();
        self.run_state = Some(WorkflowRunState {
            workflow_id,
            workflow_name: String::new(),
            status: "running".to_string(),
            cancel: cancel_for_ui,
            diagnostics: Vec::new(),
            failed_node: None,
            failed_reason: None,
            side_effects_may_have_occurred: false,
            elapsed_ms: 0,
        });
        self.show_run_panel = true;
        cx.notify();

        cx.spawn(async move |this, cx| {
            // 加载完整 DAG 图。
            let workflow = Workflow::get(&pool, workflow_id).await.ok().flatten();
            let name = workflow
                .as_ref()
                .map(|w| w.identifier.clone())
                .unwrap_or_default();
            let timeout = std::time::Duration::from_millis(
                workflow
                    .as_ref()
                    .map(|workflow| workflow.timeout_ms.max(1) as u64)
                    .unwrap_or(30_000),
            );
            let workflow_store = match WorkflowStore::new(pool.clone()) {
                Ok(s) => s,
                Err(e) => {
                    this.update(cx, |v, cx| {
                        if let Some(run) = v.run_state.as_mut() {
                            run.status = "failed".to_string();
                            run.failed_reason = Some(format!("加载 DAG 失败: {e}"));
                        }
                        cx.notify();
                    })
                    .ok();
                    return;
                }
            };
            let graph = match workflow_store.load_graph(workflow_id, name.clone()).await {
                Ok(g) => g,
                Err(e) => {
                    this.update(cx, |v, cx| {
                        if let Some(run) = v.run_state.as_mut() {
                            run.status = "failed".to_string();
                            run.failed_reason = Some(format!("加载 DAG 失败: {e}"));
                        }
                        cx.notify();
                    })
                    .ok();
                    return;
                }
            };

            let executor =
                WorkflowExecutor::new(LocalWorkflowNodeExecutor::new(pool, plugin_root, crypto));
            let outcome = executor
                .execute_report_with_timeout(&graph, serde_json::json!({}), cancel, timeout)
                .await;

            this.update(cx, |v, cx| {
                let Some(run) = v.run_state.as_mut() else {
                    return;
                };
                run.workflow_name = name;
                match outcome {
                    Ok(report) => {
                        run.status = match report.status {
                            WorkflowRunStatus::Completed => "completed",
                            WorkflowRunStatus::Failed => "failed",
                            WorkflowRunStatus::Cancelled => "cancelled",
                        }
                        .to_string();
                        run.failed_node = report.failed_node;
                        run.failed_reason = report.error_category;
                        run.side_effects_may_have_occurred = report.side_effects_may_have_occurred;
                        run.elapsed_ms = report.elapsed_ms;
                        run.diagnostics = report
                            .node_results
                            .into_iter()
                            .map(|node| NodeDiagnostic {
                                node_key: node.node_key,
                                status: match node.status {
                                    RuntimeNodeStatus::Completed => NodeRunStatus::Completed,
                                    RuntimeNodeStatus::Failed => NodeRunStatus::Failed,
                                    RuntimeNodeStatus::TimedOut => NodeRunStatus::TimedOut,
                                    RuntimeNodeStatus::Cancelled => NodeRunStatus::Cancelled,
                                    RuntimeNodeStatus::NotStarted => NodeRunStatus::NotStarted,
                                },
                                output: node.output.as_ref().map(truncate_output),
                                error: node.error,
                                elapsed_ms: node.elapsed_ms,
                            })
                            .collect();
                    }
                    Err(err) => {
                        run.status = "failed".to_string();
                        run.failed_node = None;
                        run.failed_reason = Some(err.to_string());
                        run.side_effects_may_have_occurred = false;
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// 停止正在后台执行的 workflow。
    fn stop_workflow(&mut self, cx: &mut Context<Self>) {
        if let Some(run) = self.run_state.as_mut() {
            run.cancel.signal();
            run.status = "stopping".to_string();
        }
        cx.notify();
    }

    fn close_run_panel(&mut self, cx: &mut Context<Self>) {
        // 运行中不允许关闭，避免隐藏停止入口。
        if let Some(run) = self.run_state.as_ref()
            && matches!(run.status.as_str(), "running" | "stopping")
        {
            return;
        }
        self.show_run_panel = false;
        cx.notify();
    }
}

/// 截断节点输出用于诊断展示，避免超长输出撑爆面板。
fn truncate_output(value: &serde_json::Value) -> String {
    let s = value.to_string();
    if s.len() > 512 {
        let mut cut = s;
        cut.truncate(509);
        cut.push_str("...");
        cut
    } else {
        s
    }
}

impl Render for WorkflowView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tp = (self.total_count + self.page_size - 1) / self.page_size;

        // T098 执行面板快照（渲染闭包无法借用 self，提前 clone）。
        let run_title = self
            .run_state
            .as_ref()
            .map(|r| r.workflow_name.clone())
            .unwrap_or_default();
        let run_status_label: &'static str = self
            .run_state
            .as_ref()
            .map(|r| match r.status.as_str() {
                "running" => "正在执行...",
                "stopping" => "正在停止...",
                "completed" => "执行完成",
                "failed" => "执行失败",
                "cancelled" => "已取消",
                _ => "",
            })
            .unwrap_or("");
        let run_is_running = self
            .run_state
            .as_ref()
            .map(|r| matches!(r.status.as_str(), "running" | "stopping"))
            .unwrap_or(false);
        let run_side_effect_warning = self
            .run_state
            .as_ref()
            .map(|r| r.side_effects_may_have_occurred)
            .unwrap_or(false);
        let run_elapsed_ms = self
            .run_state
            .as_ref()
            .map(|run| run.elapsed_ms)
            .unwrap_or(0);
        let run_diagnostics = self
            .run_state
            .as_ref()
            .map(|r| r.diagnostics.clone())
            .unwrap_or_default();
        let run_failed_node = self.run_state.as_ref().and_then(|r| r.failed_node.clone());
        let run_failed_reason = self
            .run_state
            .as_ref()
            .and_then(|r| r.failed_reason.clone());

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

        // 确保输入框已初始化
        if self.show_form && self.identifier_input.is_none() {
            self.identifier_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("唯一标识符")
                    .default_value(&self.form_identifier)
            }));
            self.name_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("工作流名称")
                    .default_value(&self.form_name)
            }));
            self.description_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("描述（可选）")
                    .default_value(&self.form_description)
            }));
            self.timeout_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("30000")
                    .default_value(&self.form_timeout_ms)
            }));
            self.category_id_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("分类 ID（可选）")
                    .default_value(&self.form_category_id)
            }));
            self.input_schema_input = Some(cx.new(|cx| {
                let mut state =
                    TextareaState::new(window, cx).placeholder("输入 Schema（JSON，可选）");
                state.set_value(self.form_input_schema.clone(), window, cx);
                state
            }));
            self.start_description_input = Some(cx.new(|cx| {
                let mut state = TextareaState::new(window, cx).placeholder("起始提示（可选）");
                state.set_value(self.form_start_description.clone(), window, cx);
                state
            }));
            self.output_schema_input = Some(cx.new(|cx| {
                let mut state =
                    TextareaState::new(window, cx).placeholder("输出 Schema（JSON，可选）");
                state.set_value(self.form_output_schema.clone(), window, cx);
                state
            }));
            self.required_capabilities_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("必需能力（可选，JSON 或以逗号分隔）")
                    .default_value(&self.form_required_capabilities)
            }));
        }

        // T098 执行面板按钮回调用的 weak 引用（提前提取，避免 move 借用中的 cx）。
        let run_stop_weak = cx.weak_entity();
        let run_close_weak = cx.weak_entity();
        let run_overlay_weak = cx.weak_entity();

        let style = ManagementStyle::current(cx);
        let theme = cx.theme();

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
                            .child("工作流管理"),
                    )
                    .child(
                        action_button(
                            "add-btn",
                            "+ 添加工作流",
                            ActionRole::Main,
                            ActionSize::Page,
                            style,
                        )
                        .debug_selector(|| "WORKFLOW_ADD".to_string())
                        .track_focus(&self.add_focus)
                        .on_mouse_down(MouseButton::Left, {
                            let t = cx.weak_entity();
                            move |_, window, cx| {
                                t.update(cx, |v, cx| v.show_add_form(window, cx)).ok();
                            }
                        })
                        .when(self.add_focus.is_focused(window), |button| {
                            button.child(debug_marker("WORKFLOW_ADD_FOCUSED"))
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
                                            .child("Timeout"),
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
                                            .child(format!("{}ms", item.timeout_ms)),
                                    )
                                    .child(
                                        list_actions(None, style)
                                            .child(
                                                action_button(
                                                    ("run", id as u64),
                                                    "执行",
                                                    ActionRole::Main,
                                                    ActionSize::Row,
                                                    style,
                                                )
                                                .on_mouse_down(MouseButton::Left, {
                                                    let t = cx.weak_entity();
                                                    move |_, _, cx| {
                                                        t.update(cx, |v, cx| {
                                                            v.run_workflow(id, cx);
                                                        })
                                                        .ok();
                                                    }
                                                }),
                                            )
                                            .child(
                                                action_button(
                                                    ("dag", id as u64),
                                                    "编辑DAG",
                                                    ActionRole::Edit,
                                                    ActionSize::Row,
                                                    style,
                                                )
                                                .on_mouse_down(MouseButton::Left, {
                                                    let t = cx.weak_entity();
                                                    move |_, _, cx| {
                                                        t.update(cx, |v, cx| {
                                                            v.show_dag_editor(id, cx);
                                                        })
                                                        .ok();
                                                    }
                                                }),
                                            )
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
                                                .debug_selector(move || {
                                                    format!("WORKFLOW_DELETE-{id}")
                                                })
                                                .on_mouse_down(MouseButton::Left, {
                                                    let t = cx.weak_entity();
                                                    move |_, _, cx| {
                                                        t.update(cx, |v, cx| {
                                                            v.confirm_delete_id = Some(id);
                                                            v.confirm_delete_conflict = None;
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
                let timeout_input = self.timeout_input.clone().unwrap();
                let category_id_input = self.category_id_input.clone().unwrap();
                let input_schema_input = self.input_schema_input.clone().unwrap();
                let start_description_input = self.start_description_input.clone().unwrap();
                let output_schema_input = self.output_schema_input.clone().unwrap();
                let required_capabilities_input = self.required_capabilities_input.clone().unwrap();

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
                            move |_, window, cx| {
                                t.update(cx, |v, cx| v.hide_form(window, cx)).ok();
                            }
                        }),
                )
                .child(
                    management_modal_panel(
                        management_modal_layer(px(500.0)),
                        theme.popover,
                        theme.foreground,
                        theme.border,
                    )
                    .track_focus(&self.form_focus)
                    .on_key_down(cx.listener(|v, event: &KeyDownEvent, window, cx| {
                        v.on_key_down(event, window, cx);
                    }))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .child(
                        div()
                            .debug_selector(|| "workflow-form-scroll".to_string())
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_h_0()
                            .child(
                                management_modal_scroll(
                                    "workflow-form-native-scroll",
                                    &self.form_scroll,
                                )
                                .debug_selector(|| "WORKFLOW_FORM_SCROLL".to_string())
                                .gap(px(12.0))
                                .child(
                                    div()
                                        .text_size(px(18.0))
                                        .font_weight(FontWeight::BOLD)
                                        .child(if self.editing_id.is_some() {
                                            "编辑工作流"
                                        } else {
                                            "添加工作流"
                                        }),
                                )
                            .child(form_field(
                                "Identifier *",
                                identifier_input,
                                "WORKFLOW_IDENTIFIER",
                                theme,
                            ))
                            .child(form_field("名称 *", name_input, "WORKFLOW_NAME", theme))
                            .child(form_field(
                                "描述",
                                description_input,
                                "WORKFLOW_DESCRIPTION",
                                theme,
                            ))
                            .child(form_field(
                                "Timeout (ms)",
                                timeout_input,
                                "WORKFLOW_TIMEOUT_MS",
                                theme,
                            ))
                            .child(form_field(
                                "分类 ID",
                                category_id_input,
                                "WORKFLOW_CATEGORY_ID",
                                theme,
                            ))
                            .child(form_field_multiline(
                                "输入 Schema（JSON，可选）",
                                input_schema_input,
                                "WORKFLOW_INPUT_SCHEMA",
                                theme,
                            ))
                            .child(form_field_multiline(
                                "起始提示（可选）",
                                start_description_input,
                                "WORKFLOW_START_DESCRIPTION",
                                theme,
                            ))
                            .child(form_field_multiline(
                                "输出 Schema（JSON，可选）",
                                output_schema_input,
                                "WORKFLOW_OUTPUT_SCHEMA",
                                theme,
                            ))
                            .child(form_field(
                                "必需能力（可选，JSON 或以逗号分隔）",
                                required_capabilities_input,
                                "WORKFLOW_REQUIRED_CAPABILITIES",
                                theme,
                            ))
                            .when_some(self.error_message.as_ref(), |this, err| {
                                let conflict_selector = self
                                    .identifier_conflict
                                    .as_ref()
                                    .map(|identifier| {
                                        format!("WORKFLOW_IDENTIFIER_CONFLICT-{identifier}")
                                    });
                                let identifier_value = format!(
                                    "WORKFLOW_IDENTIFIER_VALUE-{}",
                                    self.form_identifier
                                );
                                let name_value =
                                    format!("WORKFLOW_NAME_VALUE-{}", self.form_name);
                                this.child(
                                    div()
                                        .track_focus(&self.error_focus)
                                        .tab_index(0)
                                        .p(px(8.0))
                                        .bg(theme.warning.opacity(0.1))
                                        .rounded(px(4.0))
                                        .text_size(px(12.0))
                                        .text_color(theme.warning)
                                        .child(err.clone())
                                        .child(debug_marker(identifier_value))
                                        .child(debug_marker(name_value))
                                        .when_some(conflict_selector, |error, selector| {
                                            error.child(debug_marker(selector))
                                        })
                                        .when(self.error_focus.is_focused(window), |error| {
                                            error.child(debug_marker(
                                                "WORKFLOW_FORM_ERROR_FOCUSED",
                                            ))
                                        }),
                                )
                            })
                                .child(
                                    div()
                                    .debug_selector(|| "WORKFLOW_FORM_ACTIONS".to_string())
                                    .flex()
                                    .justify_end()
                                    .gap(px(8.0))
                                    .child(
                                        action_button(
                                            "cancel",
                                            "取消",
                                            ActionRole::Neutral,
                                            ActionSize::Page,
                                            style,
                                        )
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            {
                                                let t = cx.weak_entity();
                                                move |_, window, cx| {
                                                    t.update(cx, |v, cx| {
                                                        v.hide_form(window, cx)
                                                    })
                                                    .ok();
                                                }
                                            },
                                        ),
                                    )
                                    .child(
                                        action_button(
                                            "save",
                                            "保存",
                                            ActionRole::Main,
                                            ActionSize::Page,
                                            style,
                                        )
                                        .debug_selector(|| "WORKFLOW_FORM_SAVE".to_string())
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            {
                                                let t = cx.weak_entity();
                                                move |_, window, cx| {
                                                    t.update(cx, |v, cx| v.save(window, cx)).ok();
                                                }
                                            },
                                        ),
                                    ),
                                ),
                            ),
                    ),
                )
            })
            .when(self.confirm_delete_id.is_some(), |this| {
                let id = self.confirm_delete_id.unwrap();
                let conflict_selector = format!("WORKFLOW_DELETE_CONFLICT-{id}");
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
                                    v.confirm_delete_conflict = None;
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
                                                .child("确定要删除这个工作流吗？此操作不可恢复。"),
                                        )
                                        .when_some(
                                            self.confirm_delete_conflict.as_ref(),
                                            |this, error| {
                                                let selector = conflict_selector.clone();
                                                let references = error.references().join(", ");
                                                this.child(
                                                    div()
                                                        .debug_selector(move || selector.clone())
                                                        .p(px(8.0))
                                                        .rounded(px(4.0))
                                                        .bg(theme.warning.opacity(0.1))
                                                        .text_size(px(12.0))
                                                        .text_color(theme.warning)
                                                        .child(format!(
                                                            "删除失败：{}；引用：{}",
                                                            error.reason(),
                                                            references
                                                        )),
                                                )
                                            },
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
                                                                v.confirm_delete_conflict = None;
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
                                                    .debug_selector(|| {
                                                        "confirm-delete".to_string()
                                                    })
                                                    .on_mouse_down(MouseButton::Left, {
                                                        let t = cx.weak_entity();
                                                        move |_, _, cx| {
                                                            t.update(cx, |v, cx| {
                                                                if let Some(did) =
                                                                    v.confirm_delete_id
                                                                {
                                                                    v.confirm_delete_conflict = None;
                                                                    v.delete(did, cx);
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
            .when(self.show_dag_editor, |this| {
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
                                t.update(cx, |v, cx| v.hide_dag_editor(cx)).ok();
                            }
                        }),
                )
                .child(
                    div()
                        .absolute()
                        .top(px(24.0))
                        .bottom(px(24.0))
                        .left(px(24.0))
                        .right(px(24.0))
                        .bg(theme.popover)
                        .rounded(px(12.0))
                        .shadow_lg()
                        .border_1()
                        .border_color(theme.border)
                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                            cx.stop_propagation();
                        })
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .size_full()
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
                                                .child("编辑 DAG"),
                                        )
                                        .child(
                                            action_button(
                                                "close-dag",
                                                "关闭",
                                                ActionRole::Neutral,
                                                ActionSize::Page,
                                                style,
                                            )
                                            .on_mouse_down(MouseButton::Left, {
                                                let t = cx.weak_entity();
                                                move |_, _, cx| {
                                                    t.update(cx, |v, cx| v.hide_dag_editor(cx))
                                                        .ok();
                                                }
                                            }),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_1()
                                        .min_h_0()
                                        .overflow_hidden()
                                        .when_some(self.dag_editor_view.clone(), |this, editor| {
                                            this.child(editor)
                                        }),
                                ),
                        ),
                )
            })
            .when(self.show_run_panel, move |this| {
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
                            let t = run_overlay_weak;
                            move |_, _, cx| {
                                t.update(cx, |v, cx| v.close_run_panel(cx)).ok();
                            }
                        }),
                )
                .child(
                    div()
                        .absolute()
                        .top(px(24.0))
                        .bottom(px(24.0))
                        .left(px(24.0))
                        .right(px(24.0))
                        .bg(theme.popover)
                        .rounded(px(12.0))
                        .shadow_lg()
                        .border_1()
                        .border_color(theme.border)
                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                            cx.stop_propagation();
                        })
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .size_full()
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
                                                .child(format!("执行结果 · {run_title}")),
                                        )
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap(px(8.0))
                                                .when(run_is_running, move |this| {
                                                    this.child(
                                                        action_button(
                                                            "stop-run",
                                                            "停止",
                                                            ActionRole::Warning,
                                                            ActionSize::Page,
                                                            style,
                                                        )
                                                        .on_mouse_down(MouseButton::Left, {
                                                            let t = run_stop_weak;
                                                            move |_, _, cx| {
                                                                t.update(cx, |v, cx| {
                                                                    v.stop_workflow(cx)
                                                                })
                                                                .ok();
                                                            }
                                                        }),
                                                    )
                                                })
                                                .child(
                                                    action_button(
                                                        "close-run",
                                                        "关闭",
                                                        ActionRole::Neutral,
                                                        ActionSize::Page,
                                                        style,
                                                    )
                                                    .on_mouse_down(MouseButton::Left, {
                                                        let t = run_close_weak;
                                                        move |_, _, cx| {
                                                            t.update(cx, |v, cx| {
                                                                v.close_run_panel(cx)
                                                            })
                                                            .ok();
                                                        }
                                                    }),
                                                ),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .flex_1()
                                        .min_h_0()
                                        .overflow_hidden()
                                        .p(px(16.0))
                                        .gap(px(12.0))
                                        .child(
                                            div()
                                                .text_size(px(14.0))
                                                .child(format!(
                                                    "{run_status_label} · {run_elapsed_ms} ms"
                                                )),
                                        )
                                        .when_some(run_failed_node, |this, node| {
                                            this.child(
                                                div()
                                                    .text_size(px(13.0))
                                                    .text_color(theme.warning)
                                                    .child(format!("失败节点：{node}")),
                                            )
                                        })
                                        .when_some(run_failed_reason, |this, reason| {
                                            this.child(
                                                div()
                                                    .text_size(px(13.0))
                                                    .text_color(theme.warning)
                                                    .child(format!("原因：{reason}")),
                                            )
                                        })
                                        .when(run_side_effect_warning, move |this| {
                                            this.child(
                                                div()
                                                    .bg(theme.warning.opacity(0.1))
                                                    .rounded(px(4.0))
                                                    .px(px(8.0))
                                                    .py(px(6.0))
                                                    .text_size(px(13.0))
                                                    .text_color(theme.warning)
                                                    .child(
                                                        "提示：已完成节点的外部副作用不会自动回滚。",
                                                    ),
                                            )
                                        })
                                        .child(
                                            div()
                                                .text_size(px(13.0))
                                                .text_color(theme.foreground.opacity(0.6))
                                                .child("节点诊断"),
                                        )
                                        .child(
                                            div()
                                                .flex()
                                                .flex_col()
                                                .flex_1()
                                                .min_h_0()
                                                .overflow_y_scrollbar()
                                                .gap(px(4.0))
                                                .children(run_diagnostics.into_iter().map(|d| {
                                                    div()
                                                        .flex()
                                                        .items_center()
                                                        .gap(px(8.0))
                                                        .child(
                                                            div()
                                                                .text_size(px(12.0))
                                                                .text_color(
                                                                    theme.foreground.opacity(0.6),
                                                                )
                                                                .child(d.status.label()),
                                                        )
                                                        .child(
                                                            div()
                                                                .text_size(px(13.0))
                                                                .child(d.node_key),
                                                        )
                                                        .child(
                                                            div()
                                                                .text_size(px(12.0))
                                                                .text_color(
                                                                    theme.foreground.opacity(0.6),
                                                                )
                                                                .child(format!(
                                                                    "{} ms",
                                                                    d.elapsed_ms
                                                                )),
                                                        )
                                                        .when_some(d.output, |this, out| {
                                                            this.child(
                                                                div()
                                                                    .text_size(px(12.0))
                                                                    .text_color(
                                                                        theme
                                                                            .foreground
                                                                            .opacity(0.6),
                                                                    )
                                                                .child(out),
                                                            )
                                                        })
                                                        .when_some(d.error, |this, error| {
                                                            this.child(
                                                                div()
                                                                    .text_size(px(12.0))
                                                                    .text_color(theme.warning)
                                                                    .child(error),
                                                            )
                                                        })
                                                })),
                                        ),
                                ),
                        ),
                )
            })
    }
}

fn form_field(
    label: &'static str,
    input: Entity<InputState>,
    selector: &'static str,
    theme: &gpui_component::theme::Theme,
) -> impl IntoElement {
    let debug_selector = selector.to_string();
    div()
        .flex()
        .flex_col()
        .gap(px(4.0))
        .child(div().text_size(px(13.0)).child(label))
        .child(
            div().debug_selector(move || debug_selector.clone()).child(
                Input::new(&input)
                    .w_full()
                    .h(px(32.0))
                    .px(px(8.0))
                    .border_1()
                    .border_color(theme.border)
                    .rounded(px(4.0)),
            ),
        )
}

fn form_field_multiline(
    label: &'static str,
    input: Entity<TextareaState>,
    selector: &'static str,
    theme: &gpui_component::theme::Theme,
) -> impl IntoElement {
    let debug_selector = selector.to_string();
    div()
        .flex()
        .flex_col()
        .gap(px(4.0))
        .child(div().text_size(px(13.0)).child(label))
        .child(
            div().debug_selector(move || debug_selector.clone()).child(
                Textarea::new(&input)
                    .w_full()
                    .h(px(48.0))
                    .px(px(8.0))
                    .py(px(8.0))
                    .border_1()
                    .border_color(theme.border)
                    .rounded(px(4.0)),
            ),
        )
}

fn debug_marker(selector: impl Into<SharedString>) -> Stateful<Div> {
    let selector = selector.into();
    let debug_selector = selector.clone();
    div()
        .id(selector)
        .debug_selector(move || debug_selector.to_string())
        .w(px(0.0))
        .h(px(0.0))
        .overflow_hidden()
}
