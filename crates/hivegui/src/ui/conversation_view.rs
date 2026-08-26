//! 会话视图（本地对话、路由、流式事件和停止/取消明细）。
//! scroll:agent_execution

use crate::agent::local_agent::{
    LocalAgentError, LocalAgentRuntime, LocalAgentTurnResult, SessionHandle,
};
use crate::datasource::Store;
use crate::runtime::diagnostics::{DiagnosticBundle, ExecutionEventCollector, RedactionConfig};
use crate::runtime::provider_resolver::LocalProviderDecisionModel;
use crate::runtime::tool_adapter::{LocalPersistedToolTargetRunner, PersistedToolExecutor};
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::ActiveTheme as _;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::scroll::ScrollableElement;
use hive_runtime_core::execution::{EventSink, RuntimeEvent, RuntimeEventKind};
use std::path::Path;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

actions!(
    hivegui_conversation,
    [ConversationTab, ConversationActivate]
);

#[derive(Debug, Clone)]
struct SessionRow {
    id: String,
    title: String,
    handle: SessionHandle,
    messages: Vec<(String, String, bool)>,
    event_log: Vec<String>,
    current_agent: String,
    direct_children: Vec<String>,
    tool_state: String,
    workflow_state: String,
    route_attempts: usize,
    in_flight_execution: Option<String>,
}

struct MessageFeedback {
    value: SharedString,
}

impl Render for MessageFeedback {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        focus_marker(format!("CONVERSATION_MESSAGE_VALUE-{}", self.value))
    }
}

impl SessionRow {
    fn new(handle: SessionHandle, title: String) -> Self {
        Self {
            id: handle.as_str().to_string(),
            title,
            handle,
            messages: Vec::new(),
            event_log: Vec::new(),
            current_agent: "root".to_string(),
            direct_children: Vec::new(),
            tool_state: "空闲".to_string(),
            workflow_state: "空闲".to_string(),
            route_attempts: 0,
            in_flight_execution: None,
        }
    }

    fn append_event(&mut self, event: impl Into<String>) {
        self.event_log.push(event.into());
        while self.event_log.len() > 20 {
            let _ = self.event_log.remove(0);
        }
    }
}

pub struct ConversationView {
    runtime: Option<LocalAgentRuntime>,
    store: Option<Store>,
    collector: Arc<ExecutionEventCollector>,
    sessions: Vec<SessionRow>,
    active_session_id: Option<String>,
    route_input_state: Option<Entity<InputState>>,
    route_input: SharedString,
    message_input_state: Option<Entity<InputState>>,
    message_input: SharedString,
    message_feedback: Entity<MessageFeedback>,
    status_message: Option<SharedString>,
    is_error: bool,
    is_busy: bool,
    is_stopping: bool,
    stop_feedback_visible: bool,
    confirm_delete: bool,
    new_focus: FocusHandle,
    send_focus: FocusHandle,
    stop_focus: FocusHandle,
    delete_focus: FocusHandle,
}

impl ConversationView {
    pub fn new(
        cx: &mut Context<Self>,
        runtime: Option<LocalAgentRuntime>,
        store: Option<Store>,
        collector: Arc<ExecutionEventCollector>,
    ) -> Self {
        cx.bind_keys([
            KeyBinding::new("tab", ConversationTab, Some("HiveguiConversation")),
            KeyBinding::new("enter", ConversationActivate, Some("HiveguiConversation")),
            KeyBinding::new("space", ConversationActivate, Some("HiveguiConversation")),
        ]);
        let message_feedback = cx.new(|_| MessageFeedback { value: "".into() });
        Self {
            runtime,
            store,
            collector,
            sessions: Vec::new(),
            active_session_id: None,
            route_input_state: None,
            route_input: "".into(),
            message_input_state: None,
            message_input: "".into(),
            message_feedback,
            status_message: Some("本地会话已就绪，可开始对话".into()),
            is_error: false,
            is_busy: false,
            is_stopping: false,
            stop_feedback_visible: false,
            confirm_delete: false,
            new_focus: cx.focus_handle(),
            send_focus: cx.focus_handle(),
            stop_focus: cx.focus_handle(),
            delete_focus: cx.focus_handle(),
        }
    }

    fn ensure_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.message_input_state.is_none() {
            self.message_input_state = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("输入对话内容…")
                    .default_value(self.message_input.to_string())
            }));
            if let Some(state) = &self.message_input_state {
                cx.subscribe_in(state, window, move |this, state, event, window, cx| {
                    if let InputEvent::Change = event {
                        let value: SharedString = state.read(cx).value().to_string().into();
                        this.message_input = value.clone();
                        this.message_feedback.update(cx, |feedback, cx| {
                            feedback.value = value;
                            cx.notify();
                        });
                    }
                    if let InputEvent::PressEnter { .. } = event {
                        this.send_message(window, cx);
                    }
                })
                .detach();
            }
        }

        if self.route_input_state.is_none() {
            self.route_input_state = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("子 Agent identifier")
                    .default_value(self.route_input.to_string())
            }));
            if let Some(state) = &self.route_input_state {
                cx.subscribe_in(state, window, move |this, input, event, window, cx| {
                    if let InputEvent::Change = event {
                        this.route_input = input.read(cx).value().to_string().into();
                        cx.notify();
                    }
                    if let InputEvent::PressEnter { .. } = event {
                        let route = input.read(cx).value().to_string();
                        this.route_to_child(&route, cx, window);
                    }
                })
                .detach();
            }
        }

        if let Some(msg_state) = &self.message_input_state {
            self.message_input = msg_state.read(cx).value().to_string().into();
        }
        if let Some(route_state) = &self.route_input_state {
            self.route_input = route_state.read(cx).value().to_string().into();
        }
    }

    fn active_session_mut(&mut self) -> Option<&mut SessionRow> {
        self.active_session_id
            .as_deref()
            .and_then(|id| self.sessions.iter_mut().find(|s| s.id == id))
    }

    fn record_event(&mut self, session_id: &str, event: impl Into<String>) {
        if let Some(session) = self.sessions.iter_mut().find(|s| s.id == session_id) {
            let ev = event.into();
            session.append_event(ev.clone());
            if let Some(execution_id) = &session.in_flight_execution {
                self.collector
                    .record_agent_event(execution_id.clone(), "conversation_event", ev);
            }
        }
    }

    fn sync_runtime_states(&mut self, _cx: &mut Context<Self>) {
        let Some(runtime) = self.runtime.as_ref() else {
            return;
        };
        for session in self.sessions.iter_mut() {
            if let Some(state) = runtime.session_state(&session.handle) {
                match state.as_str() {
                    "running_tool" => {
                        session.tool_state = "运行中".to_string();
                    }
                    "awaiting_model" => {
                        session.workflow_state = "等待模型".to_string();
                    }
                    "rolled_back" => {
                        session.workflow_state = "回滚中".to_string();
                    }
                    "terminated" => {
                        session.tool_state = "已终止".to_string();
                        session.workflow_state = "已终止".to_string();
                    }
                    "idle" => {
                        session.tool_state = "空闲".to_string();
                        session.workflow_state = "空闲".to_string();
                    }
                    _ => {}
                }
            }
            if let Some(snapshot) = runtime.snapshot(&session.handle) {
                session.current_agent = snapshot.agent.identifier().to_string();
                session.direct_children = snapshot
                    .direct_children
                    .into_iter()
                    .map(|child| child.identifier().to_string())
                    .collect();
            }
        }
    }

    fn send_message(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_busy {
            self.status_message = Some("已有消息正在处理".into());
            self.is_error = true;
            cx.notify();
            return;
        }
        let text = self.message_input.trim().to_string();
        if text.is_empty() {
            self.status_message = Some("消息不能为空".into());
            self.is_error = true;
            cx.notify();
            return;
        }

        let Some(runtime) = self.runtime.clone() else {
            self.status_message = Some("本地会话运行时未就绪".into());
            self.is_error = true;
            cx.notify();
            return;
        };
        let Some(store) = self.store.clone() else {
            self.status_message = Some("本地数据存储未就绪".into());
            self.is_error = true;
            cx.notify();
            return;
        };

        let entity = cx.entity();
        let collector = self.collector.clone();
        self.is_busy = true;
        self.is_stopping = false;
        self.stop_feedback_visible = false;
        self.is_error = false;
        self.status_message = Some("已提交消息".into());
        self.message_input = "".into();
        self.message_feedback.update(cx, |feedback, cx| {
            feedback.value = "".into();
            cx.notify();
        });
        if let Some(input) = &self.message_input_state {
            input.update(cx, |state, cx| state.set_value("", window, cx));
        }

        match self.active_session_id.clone() {
            Some(id) => {
                if let Some(session) = self.sessions.iter().find(|s| s.id == id) {
                    let handle = session.handle.clone();
                    let execution_id = uuid::Uuid::new_v4().to_string();
                    if let Err(error) = runtime.begin_turn(&handle, &text) {
                        self.is_busy = false;
                        self.is_error = true;
                        self.status_message = Some(local_agent_error_message(&error).into());
                        cx.notify();
                        return;
                    }
                    self.prepare_turn_ui(&id, &text, &execution_id);
                    collector.record_agent_event(
                        &execution_id,
                        "user_message",
                        "user message appended",
                    );
                    let task_runtime = runtime.clone();
                    let task_store = store.clone();
                    let task_handle = handle.clone();
                    let task_execution_id = execution_id.clone();
                    let task_collector = collector.clone();
                    let task = crate::ui::spawn_tokio(async move {
                        execute_local_turn(
                            &task_runtime,
                            &task_store,
                            &task_handle,
                            &task_execution_id,
                            task_collector,
                        )
                        .await
                    });
                    cx.spawn(async move |_this, cx| {
                        let result = task.await.unwrap_or(Err(LocalAgentError::AgentRejected(
                            "local execution background task failed",
                        )));
                        finish_local_turn_ui(&entity, cx, &id, &execution_id, &collector, result);
                    })
                    .detach();
                }
            }
            None => {
                let execution_id = uuid::Uuid::new_v4().to_string();
                let entity = cx.entity();
                let collector = collector.clone();
                let start_runtime = runtime.clone();
                let start_text = text.clone();
                let start_task = crate::ui::spawn_tokio(async move {
                    start_runtime.start_session(&start_text).await
                });
                cx.spawn(async move |_this, cx| {
                    match start_task
                        .await
                        .unwrap_or(Err(LocalAgentError::AgentRejected(
                            "session background task failed",
                        ))) {
                        Ok((handle, _token)) => {
                            entity.update(cx, |this, cx| {
                                let session = SessionRow::new(handle.clone(), "新会话".to_string());
                                this.sessions.push(session);
                                this.active_session_id = Some(handle.as_str().to_string());
                                this.prepare_turn_ui(handle.as_str(), &text, &execution_id);
                                this.record_event(handle.as_str(), "会话启动完成");
                                cx.notify();
                            });
                            collector.record_agent_event(
                                &execution_id,
                                "user_message",
                                "session started with user message",
                            );
                            let session_id = handle.as_str().to_string();
                            let task_runtime = runtime.clone();
                            let task_store = store.clone();
                            let task_handle = handle.clone();
                            let task_execution_id = execution_id.clone();
                            let task_collector = collector.clone();
                            let task = crate::ui::spawn_tokio(async move {
                                execute_local_turn(
                                    &task_runtime,
                                    &task_store,
                                    &task_handle,
                                    &task_execution_id,
                                    task_collector,
                                )
                                .await
                            });
                            let result = task.await.unwrap_or(Err(LocalAgentError::AgentRejected(
                                "local execution background task failed",
                            )));
                            finish_local_turn_ui(
                                &entity,
                                cx,
                                &session_id,
                                &execution_id,
                                &collector,
                                result,
                            );
                        }
                        Err(err) => {
                            entity.update(cx, |this, cx| {
                                this.is_busy = false;
                                this.is_error = true;
                                this.status_message = Some(format!("启动会话失败: {err}").into());
                                cx.notify();
                            });
                        }
                    }
                })
                .detach();
            }
        }
    }

    fn prepare_turn_ui(&mut self, session_id: &str, text: &str, execution_id: &str) {
        if let Some(session) = self
            .sessions
            .iter_mut()
            .find(|session| session.id == session_id)
        {
            session.in_flight_execution = Some(execution_id.to_string());
            session
                .messages
                .push(("user".to_string(), text.to_string(), false));
            session
                .messages
                .push(("assistant".to_string(), String::new(), true));
            session.append_event("等待本地模型决策".to_string());
            session.tool_state = "空闲".to_string();
            session.workflow_state = "等待模型".to_string();
        }
    }

    fn route_to_child(&mut self, target: &str, _cx: &mut Context<Self>, _window: &Window) {
        let target = target.trim().to_string();
        let Some(handle) = self.active_session_id.clone() else {
            self.status_message = Some("请先发起会话".into());
            self.is_error = true;
            return;
        };
        let Some(runtime) = self.runtime.clone() else {
            self.status_message = Some("本地会话运行时未就绪".into());
            self.is_error = true;
            return;
        };
        let Some((handle_ref, session_id)) = self
            .sessions
            .iter()
            .find(|s| s.id == handle)
            .map(|session| (session.handle.clone(), session.id.clone()))
        else {
            return;
        };
        let execution_id = uuid::Uuid::new_v4().to_string();
        let entity = _cx.entity();
        self.record_event(&session_id, format!("请求路由到 {target}"));
        let collector = self.collector.clone();
        let task = crate::ui::spawn_tokio(async move {
            let result = runtime.route_to_child(&handle_ref, &target).await;
            (handle_ref, target, result)
        });
        _cx.spawn(async move |_this, cx| {
            let (handle_ref, _target, result) = match task.await {
                Ok(result) => result,
                Err(error) => {
                    entity.update(cx, move |this, cx| {
                        this.is_error = true;
                        this.status_message = Some(format!("路由后台任务失败: {error}").into());
                        cx.notify();
                    });
                    return;
                }
            };
            match result {
                Ok(snapshot) => {
                    entity.update(cx, move |this, cx| {
                        if let Some(active) = this
                            .sessions
                            .iter_mut()
                            .find(|s| s.id == handle_ref.as_str())
                        {
                            active.current_agent = snapshot.agent.identifier().to_string();
                            active.direct_children = snapshot
                                .direct_children
                                .into_iter()
                                .map(|child| child.identifier().to_string())
                                .collect();
                            active.route_attempts += 1;
                            active.in_flight_execution = Some(execution_id.clone());
                            active.append_event("路由成功".to_string());
                            this.is_error = false;
                            this.status_message = Some("路由成功".into());
                            collector.record_agent_event(
                                &execution_id,
                                "route_ok",
                                "route child agent",
                            );
                        }
                        cx.notify();
                    });
                }
                Err(err) => {
                    entity.update(cx, move |this, cx| {
                        let active_id = this
                            .sessions
                            .iter()
                            .find(|s| s.id == handle_ref.as_str())
                            .map(|active| active.id.clone());
                        if let Some(active_id) = active_id {
                            let event = format!("路由失败: {err}");
                            this.record_event(&active_id, event.clone());
                            this.is_error = true;
                            this.status_message = Some(event.into());
                        }
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    fn cancel_active_session(&mut self, cx: &mut Context<Self>) {
        self.is_stopping = true;
        self.stop_feedback_visible = true;
        self.status_message = Some("正在停止".into());
        self.is_error = false;
        let Some(runtime) = self.runtime.clone() else {
            self.status_message = Some("本地会话运行时未就绪".into());
            self.is_error = true;
            return;
        };
        let Some(id) = self.active_session_id.clone() else {
            self.status_message = Some("请选择一个会话".into());
            self.is_error = true;
            return;
        };
        let handle = self
            .sessions
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.handle.clone());
        if let Some(handle) = handle {
            if runtime.cancel_execution(&handle).is_ok() {
                self.record_event(handle.as_str(), "已发起取消");
                if let Some(active) = self.active_session_mut() {
                    active.tool_state = "已取消".to_string();
                    active.workflow_state = "已取消".to_string();
                }
            } else {
                self.status_message = Some("未找到会话".into());
                self.is_error = true;
            }
        }
        cx.notify();
    }

    fn new_conversation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.active_session_id = None;
        self.is_busy = false;
        self.is_stopping = false;
        self.stop_feedback_visible = false;
        self.confirm_delete = false;
        self.status_message = Some("已新建本地会话草稿".into());
        self.message_feedback.update(cx, |feedback, cx| {
            feedback.value = "".into();
            cx.notify();
        });
        if let Some(input) = self.message_input_state.as_ref() {
            input.update(cx, |input, cx| {
                input.set_value("", window, cx);
                input.focus(window, cx);
            });
        }
        cx.notify();
    }

    fn request_delete_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm_delete = true;
        self.delete_focus.focus(window, cx);
        cx.notify();
    }

    fn close_delete_confirmation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm_delete = false;
        if let Some(input) = self.message_input_state.as_ref() {
            input.update(cx, |input, cx| input.focus(window, cx));
        }
        cx.notify();
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "escape" => self.close_delete_confirmation(window, cx),
            "enter" | " " | "space" if self.send_focus.is_focused(window) => {
                cx.stop_propagation();
                self.send_message(window, cx);
            }
            "enter" | " " | "space" if self.stop_focus.is_focused(window) => {
                cx.stop_propagation();
                self.cancel_active_session(cx);
            }
            _ => {}
        }
    }

    fn focus_next_conversation_control(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.send_focus.is_focused(window) {
            self.stop_focus.focus(window, cx);
        } else {
            self.send_focus.focus(window, cx);
        }
        cx.notify();
    }

    fn end_active_session(&mut self, cx: &mut Context<Self>) {
        let Some(runtime) = self.runtime.clone() else {
            self.status_message = Some("本地会话运行时未就绪".into());
            self.is_error = true;
            return;
        };
        let Some(id) = self.active_session_id.clone() else {
            self.status_message = Some("请先选择会话".into());
            self.is_error = true;
            return;
        };
        let remove_index = self.sessions.iter().position(|s| s.id == id);
        if let Some(index) = remove_index {
            let handle = self.sessions[index].handle.clone();
            let _ = runtime.end_session(&handle);
            self.sessions.remove(index);
            self.active_session_id = self.sessions.first().map(|s| s.id.clone());
            self.status_message = Some("会话已结束并移除".into());
            self.is_error = false;
            cx.notify();
        }
    }

    fn select_session(&mut self, id: String) {
        self.active_session_id = Some(id);
        self.status_message = None;
    }

    fn export_diagnostic_bundle(&mut self, cx: &mut Context<Self>) {
        let path = format!(
            "./hivegui-conversation-diagnostic-{}.json",
            uuid::Uuid::new_v4()
        );
        let path = Path::new(&path);
        let bundle = DiagnosticBundle::new(self.collector.clone());
        match bundle.export_redacted(path, &RedactionConfig::default()) {
            Ok(redacted) => {
                self.status_message = Some(
                    format!(
                        "诊断导出成功：{} (events={})",
                        redacted.path().display(),
                        redacted.event_count(),
                    )
                    .into(),
                );
                self.is_error = false;
            }
            Err(err) => {
                self.status_message = Some(format!("诊断导出失败：{err}").into());
                self.is_error = true;
            }
        }
        cx.notify();
    }
}

struct ConversationRuntimeEventSink {
    collector: Arc<ExecutionEventCollector>,
    observed_first_token: AtomicBool,
}

impl ConversationRuntimeEventSink {
    fn new(collector: Arc<ExecutionEventCollector>) -> Self {
        Self {
            collector,
            observed_first_token: AtomicBool::new(false),
        }
    }
}

impl EventSink for ConversationRuntimeEventSink {
    fn emit(&self, event: RuntimeEvent) {
        match event.kind() {
            RuntimeEventKind::Token { .. }
                if !self.observed_first_token.swap(true, Ordering::SeqCst) =>
            {
                self.collector.record_llm_event(
                    event.execution_id(),
                    "stream_started",
                    "local Provider emitted its first decision delta",
                );
            }
            RuntimeEventKind::FallbackUsed { .. } => self.collector.record_llm_event(
                event.execution_id(),
                "fallback_used",
                "local Preset advanced to its next configured Provider",
            ),
            RuntimeEventKind::Cancelled { .. } => self.collector.record_llm_event(
                event.execution_id(),
                "cancelled",
                "local Provider execution was cancelled",
            ),
            _ => {}
        }
    }
}

async fn execute_local_turn(
    runtime: &LocalAgentRuntime,
    store: &Store,
    handle: &SessionHandle,
    execution_id: &str,
    collector: Arc<ExecutionEventCollector>,
) -> Result<LocalAgentTurnResult, LocalAgentError> {
    let model = LocalProviderDecisionModel::from_store(
        store,
        execution_id,
        handle.as_str(),
        Arc::new(ConversationRuntimeEventSink::new(collector)),
    )
    .with_cancel_token(
        runtime
            .cancel_token(handle)
            .ok_or(LocalAgentError::AgentRejected("turn not started"))?,
    );
    let executor = PersistedToolExecutor::new(
        store.pool().clone(),
        Arc::new(LocalPersistedToolTargetRunner::from_store(store)),
    );
    runtime.run_turn(handle, &model, &executor).await
}

fn finish_local_turn_ui(
    entity: &Entity<ConversationView>,
    cx: &mut AsyncApp,
    session_id: &str,
    execution_id: &str,
    collector: &ExecutionEventCollector,
    result: Result<LocalAgentTurnResult, LocalAgentError>,
) {
    let session_id = session_id.to_string();
    let execution_id = execution_id.to_string();
    let completed = result.is_ok();
    let reply = result
        .as_ref()
        .map(|turn| turn.reply().to_string())
        .unwrap_or_else(|error| local_agent_error_message(error).to_string());
    let tool_calls = result.as_ref().map_or(0, LocalAgentTurnResult::tool_calls);
    entity.update(cx, move |this, cx| {
        if let Some(session) = this
            .sessions
            .iter_mut()
            .find(|session| session.id == session_id)
        {
            if let Some(message) = session
                .messages
                .iter_mut()
                .rev()
                .find(|(role, _, streaming)| role == "assistant" && *streaming)
            {
                message.1 = reply.clone();
                message.2 = false;
            }
            session.in_flight_execution = None;
            session.tool_state = "空闲".to_string();
            session.workflow_state = if completed {
                "已完成".to_string()
            } else {
                "失败".to_string()
            };
            session.append_event(if completed {
                format!("回复完成（Tool 调用 {tool_calls} 次）")
            } else {
                "本地 Agent 执行失败".to_string()
            });
        }
        this.status_message = Some(if completed {
            "回复已完成".into()
        } else {
            reply.clone().into()
        });
        this.is_error = !completed;
        this.is_busy = false;
        if this.is_stopping {
            this.status_message = Some("已停止".into());
            this.stop_feedback_visible = true;
        }
        this.is_stopping = false;
        cx.notify();
    });
    collector.record_agent_event(
        execution_id,
        if completed {
            "assistant_done"
        } else {
            "turn_failed"
        },
        if completed {
            "local Agent turn completed"
        } else {
            "local Agent turn failed at a stable boundary"
        },
    );
}

fn local_agent_error_message(error: &LocalAgentError) -> &'static str {
    match error {
        LocalAgentError::NoDefaultRoot => "未配置默认根 Agent",
        LocalAgentError::InvalidHierarchy(_) => "Agent 层级配置无效",
        LocalAgentError::InvalidUserMessage(_) => "消息格式无效",
        LocalAgentError::InvalidInput { .. } => "本地运行时命令参数无效",
        LocalAgentError::AgentRejected(_) => "本地模型或 Agent 拒绝了本次执行",
        LocalAgentError::RemoteToolForbidden(_) => "已拒绝非本地 Tool",
        LocalAgentError::Store(_) => "本地数据存储操作失败",
        LocalAgentError::ToolExecution(_) => "本地 Tool 执行失败",
    }
}

impl Render for ConversationView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_inputs(window, cx);
        self.sync_runtime_states(cx);

        let view = cx.weak_entity();
        let theme = cx.theme();
        let status = if let Some(msg) = &self.status_message {
            div()
                .p(px(8.0))
                .text_color(if self.is_error {
                    theme.danger
                } else {
                    theme.foreground
                })
                .child(msg.clone())
        } else {
            div()
        };

        let message_input = self.message_input_state.clone().unwrap();
        let message_input_focused = message_input.read(cx).focus_handle(cx).is_focused(window);
        let message_feedback = self.message_feedback.clone();
        let route_input = self.route_input_state.clone().unwrap();
        let active_id = self.active_session_id.clone();

        let left = div()
            .min_w(px(220.0))
            .w(px(220.0))
            .flex()
            .flex_col()
            .gap(px(8.0))
            .p(px(8.0))
            .border_r_1()
            .border_color(theme.border)
            .child(
                div()
                    .text_size(px(16.0))
                    .font_weight(FontWeight::BOLD)
                    .child("本地会话"),
            )
            .child(
                btn(
                    "CONVERSATION_NEW",
                    "新建会话",
                    theme.primary,
                    theme.primary_hover,
                    theme.primary_foreground,
                    theme.primary_foreground,
                    {
                        let view = view.clone();
                        move |_, window, cx| {
                            view.update(cx, |this, cx| this.new_conversation(window, cx))
                                .ok();
                        }
                    },
                )
                .track_focus(&self.new_focus),
            )
            .child(
                div()
                    .debug_selector(|| "CONVERSATION_HISTORY_SCROLL".to_string())
                    .flex_1()
                    .overflow_y_scrollbar()
                    .child(div().flex().flex_col().gap(px(6.0)).children(
                        self.sessions.iter().map(|session| {
                            let id = session.id.clone();
                            let view = view.clone();
                            let label = format!("{} ({})", session.title, session.id);
                            let active = active_id.as_deref() == Some(session.id.as_str());
                            let bg = if active {
                                theme.list_active
                            } else {
                                theme.background
                            };
                            div()
                                .bg(bg)
                                .p(px(8.0))
                                .rounded(px(6.0))
                                .child(
                                    div()
                                        .text_size(px(12.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(label),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.0))
                                        .text_color(theme.muted_foreground)
                                        .child(format!(
                                            "状态: {} / {}",
                                            session.tool_state, session.workflow_state
                                        )),
                                )
                                .on_mouse_down(MouseButton::Left, {
                                    let id = id.clone();
                                    move |_, _, cx| {
                                        view.update(cx, |view, cx| {
                                            view.select_session(id.clone());
                                            cx.notify();
                                        })
                                        .ok();
                                    }
                                })
                        }),
                    )),
            )
            .child(btn(
                "conversation-end",
                if self.is_busy {
                    "处理中..."
                } else {
                    "结束会话"
                },
                theme.success,
                theme.success_hover,
                theme.foreground,
                theme.foreground,
                {
                    let view = view.clone();
                    move |_, _, cx| {
                        view.update(cx, |this, cx| this.end_active_session(cx)).ok();
                    }
                },
            ))
            .child(
                btn(
                    "CONVERSATION_DELETE_ACTIVE",
                    "删除当前历史",
                    theme.danger,
                    theme.danger_hover,
                    theme.danger_foreground,
                    theme.danger_foreground,
                    {
                        let view = view.clone();
                        move |_, window, cx| {
                            view.update(cx, |this, cx| this.request_delete_active(window, cx))
                                .ok();
                        }
                    },
                )
                .track_focus(&self.delete_focus),
            );

        let (active_title, active_messages, active_events, active_children, in_flight) = self
            .active_session_id
            .as_ref()
            .and_then(|id| self.sessions.iter().find(|s| &s.id == id))
            .map(|session| {
                (
                    session.title.clone(),
                    session.messages.len(),
                    session.event_log.len(),
                    session.direct_children.clone(),
                    session.in_flight_execution.is_some(),
                )
            })
            .unwrap_or_else(|| ("未选择会话".to_string(), 0, 0, Vec::new(), false));

        let right = div()
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .flex()
            .flex_col()
            .p(px(8.0))
            .gap(px(8.0))
            .child(
                div()
                    .text_size(px(18.0))
                    .font_weight(FontWeight::BOLD)
                    .child(active_title),
            )
            .child(status)
            .when(self.is_stopping || self.stop_feedback_visible, |panel| {
                panel.child(focus_marker("CONVERSATION_STATUS-stopping"))
            })
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(theme.muted_foreground)
                    .child(format!(
                        "消息 {} | 历史事件 {} | 活动子路由 {}",
                        active_messages, active_events, in_flight
                    )),
            )
            .child(
                div().flex().gap(px(8.0)).child(
                    div()
                        .debug_selector(|| "CONVERSATION_MESSAGE_INPUT".to_string())
                        .flex_1()
                        .child(Input::new(&message_input).aria_label("对话消息"))
                        .when(message_input_focused, |input| {
                            input.child(focus_marker("CONVERSATION_MESSAGE_INPUT_FOCUSED"))
                        })
                        .child(message_feedback),
                ),
            )
            .child(div().flex().gap(px(8.0)).justify_end().children(vec![
                btn(
                    "CONVERSATION_SEND",
                    if self.is_busy {
                        "发送中..."
                    } else {
                        "发送"
                    },
                    theme.primary,
                    theme.primary_hover,
                    theme.primary_foreground,
                    theme.primary_foreground,
                    {
                        let view = view.clone();
                        move |_, window, cx| {
                            view.update(cx, |this, cx| this.send_message(window, cx))
                                .ok();
                        }
                    },
                )
                .track_focus(&self.send_focus)
                .into_any_element(),
                btn(
                    "CONVERSATION_STOP",
                    "Stop",
                    theme.warning,
                    theme.warning_hover,
                    theme.warning_foreground,
                    theme.warning_foreground,
                    {
                        let view = view.clone();
                        move |_, _, cx| {
                            view.update(cx, |this, cx| this.cancel_active_session(cx))
                                .ok();
                        }
                    },
                )
                .track_focus(&self.stop_focus)
                .when(self.stop_focus.is_focused(window), |button| {
                    button.child(focus_marker("CONVERSATION_STOP_FOCUSED"))
                })
                .into_any_element(),
            ]))
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(theme.muted_foreground)
                    .child("路由到子 Agent："),
            )
            .child(
                div().flex().gap(px(8.0)).children([
                    Input::new(&route_input).into_any_element(),
                    btn(
                        "conversation-route",
                        "路由",
                        theme.secondary,
                        theme.secondary_hover,
                        theme.foreground,
                        theme.foreground,
                        {
                            let view = view.clone();
                            move |_, window, cx| {
                                view.update(cx, |this, cx| {
                                    let target = this.route_input.to_string();
                                    this.route_to_child(target.as_str(), cx, window);
                                })
                                .ok();
                            }
                        },
                    )
                    .into_any_element(),
                ]),
            )
            .child(div().flex().items_center().gap(px(8.0)).child({
                let mut txt = "子 Agent：".to_string();
                if active_children.is_empty() {
                    txt.push('无')
                } else {
                    txt.push_str(&active_children.join(", "))
                }
                txt.into_any_element()
            }))
            .child(
                div()
                    .debug_selector(|| "CONVERSATION_MESSAGES_SCROLL".to_string())
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .bg(theme.background)
                    .p(px(8.0))
                    .border_1()
                    .border_color(theme.border)
                    .rounded(px(6.0))
                    .children(
                        self.active_session_id
                            .as_deref()
                            .and_then(|id| self.sessions.iter().find(|session| session.id == id))
                            .map(|session| {
                                session
                                    .messages
                                    .iter()
                                    .enumerate()
                                    .map(|(idx, message)| {
                                        let (role, body, streaming) =
                                            (message.0.as_str(), message.1.as_str(), message.2);
                                        let role_text =
                                            if role == "user" { "你" } else { "HiveClaw" };
                                        div()
                                            .p(px(6.0))
                                            .border_b_1()
                                            .border_color(theme.border)
                                            .child(
                                                div()
                                                    .text_size(px(12.0))
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .child(format!("{role_text}#{idx}")),
                                            )
                                            .child(div().text_size(px(13.0)).child(format!(
                                                "{}{}",
                                                body,
                                                if streaming { " ▉" } else { "" }
                                            )))
                                            .into_any_element()
                                    })
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default(),
                    )
                    .into_any_element(),
            )
            .child(
                div()
                    .flex_1()
                    .overflow_y_scrollbar()
                    .p(px(6.0))
                    .bg(theme.group_box)
                    .rounded(px(6.0))
                    .children(
                        self.active_session_id
                            .as_deref()
                            .and_then(|id| self.sessions.iter().find(|s| s.id == id))
                            .map(|session| {
                                session
                                    .event_log
                                    .iter()
                                    .map(|event| {
                                        div()
                                            .text_size(px(11.0))
                                            .text_color(theme.muted_foreground)
                                            .child(format!("• {event}"))
                                    })
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_else(|| vec![div().text_size(px(11.0)).child("暂无事件")]),
                    ),
            );

        let diagnostic = div()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(6.0))
            .child(btn(
                "conversation-diagnostic-export",
                "导出诊断样例",
                theme.secondary,
                theme.secondary_hover,
                theme.foreground,
                theme.foreground,
                {
                    let view = view.clone();
                    move |_ev, _w, cx| {
                        let _ = view.update(cx, |this, cx| this.export_diagnostic_bundle(cx));
                    }
                },
            ));

        div()
            .id("conversation-view")
            .debug_selector(|| "body".to_owned())
            .key_context("HiveguiConversation")
            .on_action(cx.listener(|view, _: &ConversationTab, window, cx| {
                cx.stop_propagation();
                view.focus_next_conversation_control(window, cx);
            }))
            .on_action(cx.listener(|view, _: &ConversationActivate, window, cx| {
                if view.stop_focus.is_focused(window) {
                    cx.stop_propagation();
                    view.cancel_active_session(cx);
                } else if view.send_focus.is_focused(window) {
                    cx.stop_propagation();
                    view.send_message(window, cx);
                }
            }))
            .capture_key_down(cx.listener(|view, event: &KeyDownEvent, window, cx| {
                view.on_key_down(event, window, cx);
            }))
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
                    .p(px(8.0))
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .text_size(px(16.0))
                            .font_weight(FontWeight::BOLD)
                            .child("对话"),
                    )
                    .child(diagnostic),
            )
            .child(div().flex().flex_1().min_h_0().child(left).child(right))
            .when(self.confirm_delete, |root| {
                root.child(
                    div()
                        .id("conversation-delete-confirm")
                        .debug_selector(|| "CONVERSATION_DELETE_CONFIRM".to_string())
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(theme.overlay.opacity(0.35))
                        .child(
                            div()
                                .w(px(360.0))
                                .p(px(20.0))
                                .rounded(px(8.0))
                                .bg(theme.popover)
                                .border_1()
                                .border_color(theme.border)
                                .child("确认删除当前会话历史？按 Escape 取消。"),
                        ),
                )
            })
    }
}

fn btn(
    selector: &'static str,
    label: &'static str,
    bg: Hsla,
    hover: Hsla,
    fg: Hsla,
    hfg: Hsla,
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
        .hover(|s| s.bg(hover).text_color(hfg))
        .on_mouse_down(MouseButton::Left, move |ev, w, cx| {
            handler(ev, w, cx);
        })
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
    use gpui::{TestAppContext, VisualTestContext, px, size};

    use crate::runtime::diagnostics::ExecutionEventCollector;

    use super::ConversationView;

    #[gpui::test]
    fn conversation_view_has_visible_layout_and_scrolling_area(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::theme::init(cx);
            gpui_component::init(cx);
        });
        let window = cx.open_window(size(px(1100.0), px(720.0)), |_, cx| {
            ConversationView::new(
                cx,
                None,
                None,
                std::sync::Arc::new(ExecutionEventCollector::new()),
            )
        });
        cx.run_until_parked();

        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let body = cx.debug_bounds("body").expect("conversation body");

        assert!(body.size.width > px(0.0));
    }
}
