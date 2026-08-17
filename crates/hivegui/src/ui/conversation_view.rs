//! 会话视图（本地对话、路由、流式事件和停止/取消明细）。

use crate::agent::local_agent::{LocalAgentRuntime, SessionHandle};
use crate::agent::session::AgentMessage;
use crate::runtime::diagnostics::{DiagnosticBundle, ExecutionEventCollector, RedactionConfig};
use gpui::*;
use gpui_component::ActiveTheme as _;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::scroll::ScrollableElement;
use std::path::Path;
use std::sync::Arc;

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
    collector: Arc<ExecutionEventCollector>,
    sessions: Vec<SessionRow>,
    active_session_id: Option<String>,
    route_input_state: Option<Entity<InputState>>,
    route_input: SharedString,
    message_input_state: Option<Entity<InputState>>,
    message_input: SharedString,
    status_message: Option<SharedString>,
    is_error: bool,
    is_busy: bool,
}

impl ConversationView {
    pub fn new(
        _cx: &mut Context<Self>,
        runtime: Option<LocalAgentRuntime>,
        collector: Arc<ExecutionEventCollector>,
    ) -> Self {
        Self {
            runtime,
            collector,
            sessions: Vec::new(),
            active_session_id: None,
            route_input_state: None,
            route_input: "".into(),
            message_input_state: None,
            message_input: "".into(),
            status_message: Some("本地会话已就绪，可开始对话".into()),
            is_error: false,
            is_busy: false,
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
                        this.message_input = state.read(cx).value().to_string().into();
                        cx.notify();
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

        let entity = cx.entity();
        let collector = self.collector.clone();
        self.is_busy = true;
        self.is_error = false;
        self.status_message = Some("已提交消息".into());
        self.message_input = "".into();
        if let Some(input) = &self.message_input_state {
            input.update(cx, |state, cx| state.set_value("", window, cx));
        }

        match self.active_session_id.clone() {
            Some(id) => {
                if let Some(session) = self.sessions.iter().find(|s| s.id == id) {
                    let handle = session.handle.clone();
                    let execution_id = uuid::Uuid::new_v4().to_string();
                    session_placeholder_append_runtime(
                        runtime,
                        handle,
                        id,
                        text,
                        execution_id,
                        collector,
                        entity,
                        cx,
                    );
                }
            }
            None => {
                let execution_id = uuid::Uuid::new_v4().to_string();
                let entity = cx.entity();
                let collector = collector.clone();
                cx.spawn(
                    async move |_this, cx| match runtime.start_session(&text).await {
                        Ok((handle, _token)) => {
                            entity.update(cx, |this, _| {
                                let session = SessionRow::new(handle.clone(), "新会话".to_string());
                                this.sessions.push(session);
                                this.active_session_id = Some(handle.as_str().to_string());
                                if let Some(active) = this.active_session_mut() {
                                    active.in_flight_execution = Some(execution_id.clone());
                                    active.append_event("会话已启动".to_string());
                                    active
                                        .messages
                                        .push(("user".to_string(), text.clone(), false));
                                    active.messages.push((
                                        "assistant".to_string(),
                                        String::new(),
                                        true,
                                    ));
                                }
                                this.is_busy = false;
                                this.record_event(handle.as_str(), "会话启动完成");
                            });
                            let _ =
                                runtime.append_message(&handle, AgentMessage::user(text.clone()));
                            let _ = runtime.append_message(
                                &handle,
                                AgentMessage::assistant("处理中".to_string()),
                            );
                            simulation_streaming_reply(
                                handle.as_str(),
                                execution_id,
                                runtime,
                                collector,
                                entity,
                                cx,
                            )
                            .await;
                        }
                        Err(err) => {
                            entity.update(cx, |this, cx| {
                                this.is_busy = false;
                                this.is_error = true;
                                this.status_message = Some(format!("启动会话失败: {err}").into());
                                cx.notify();
                            });
                        }
                    },
                )
                .detach();
            }
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
        _cx.spawn(async move |_this, cx| {
            match runtime.route_to_child(&handle_ref, &target).await {
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

fn session_placeholder_append_runtime(
    runtime: LocalAgentRuntime,
    handle: SessionHandle,
    session_id: String,
    text: String,
    execution_id: String,
    collector: Arc<ExecutionEventCollector>,
    entity: Entity<ConversationView>,
    cx: &mut Context<ConversationView>,
) {
    let _ = runtime.append_message(&handle, AgentMessage::user(text.clone()));
    let initial_session_id = session_id.clone();
    let initial_execution_id = execution_id.clone();
    let initial_collector = collector.clone();
    entity.update(cx, move |this, cx| {
        if let Some(session) = this
            .sessions
            .iter_mut()
            .find(|s| s.id == initial_session_id)
        {
            session.in_flight_execution = Some(initial_execution_id.clone());
            session
                .messages
                .push(("user".to_string(), text.clone(), false));
            session
                .messages
                .push(("assistant".to_string(), String::new(), true));
            session.append_event("追加消息".to_string());
            session.tool_state = "运行工具".to_string();
            initial_collector.record_agent_event(
                &initial_execution_id,
                "user_message",
                "user message appended",
            );
        }
        this.sync_runtime_states(cx);
        this.is_busy = false;
        cx.notify();
    });
    cx.spawn(async move |_this, cx| {
        let chunks = ["正在处理", "执行 Tool", "汇总结果"];
        for i in chunks {
            tokio::time::sleep(std::time::Duration::from_millis(260)).await;
            let chunk_session_id = session_id.clone();
            entity.update(cx, move |this, cx| {
                if let Some(session) = this.sessions.iter_mut().find(|s| s.id == chunk_session_id) {
                    if let Some(msg) = session
                        .messages
                        .iter_mut()
                        .find(|(role, _c, streaming)| role.as_str() == "assistant" && *streaming)
                    {
                        msg.2 = true;
                        msg.1 = format!("{}{}", msg.1, i);
                    }
                    session.tool_state = "空闲".to_string();
                }
                cx.notify();
            });
        }

        let _ = runtime.append_message(&handle, AgentMessage::assistant("完成".to_string()));
        entity.update(cx, move |this, cx| {
            if let Some(session) = this.sessions.iter_mut().find(|s| s.id == session_id) {
                if let Some(msg) = session
                    .messages
                    .iter_mut()
                    .find(|(role, _, streaming)| role.as_str() == "assistant" && *streaming)
                {
                    msg.2 = false;
                    msg.1.push_str(" 完成");
                }
                collector.record_agent_event(&execution_id, "assistant_done", "assistant done");
                session.in_flight_execution = None;
            }
            this.status_message = Some("回复已完成".into());
            this.is_busy = false;
            cx.notify();
        });
    })
    .detach();
}

async fn simulation_streaming_reply(
    handle_id: &str,
    execution_id: String,
    _runtime: LocalAgentRuntime,
    collector: Arc<ExecutionEventCollector>,
    entity: Entity<ConversationView>,
    cx: &mut AsyncApp,
) {
    let chunks = ["处理中", "已准备工具参数", "生成最终响应"];
    for chunk in chunks {
        tokio::time::sleep(std::time::Duration::from_millis(180)).await;
        let stream_collector = collector.clone();
        let stream_execution_id = execution_id.clone();
        entity.update(cx, move |this, cx| {
            if let Some(session) = this.sessions.iter_mut().find(|s| s.id == handle_id) {
                if let Some(msg) = session
                    .messages
                    .iter_mut()
                    .find(|(role, _, streaming)| role.as_str() == "assistant" && *streaming)
                {
                    msg.2 = true;
                    msg.1 = format!("{}{}", msg.1, chunk);
                }
                stream_collector.record_agent_event(&stream_execution_id, "stream", chunk);
            }
            cx.notify();
        });
    }
    entity.update(cx, move |this, cx| {
        if let Some(session) = this.sessions.iter_mut().find(|s| s.id == handle_id) {
            if let Some(msg) = session
                .messages
                .iter_mut()
                .find(|(role, _, streaming)| role.as_str() == "assistant" && *streaming)
            {
                msg.2 = false;
                msg.1.push_str(" 完成");
            }
            session.in_flight_execution = None;
            session.append_event("回复完成");
        }
        collector.record_agent_event(
            &execution_id,
            "assistant_done",
            "response streaming finished",
        );
        cx.notify();
    });
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
                div().flex_1().overflow_y_scrollbar().child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.0))
                        .children(self.sessions.iter().map(|session| {
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
                        })),
                ),
            )
            .child(btn(
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
            ));

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
                div()
                    .flex()
                    .gap(px(8.0))
                    .child(div().flex_1().child(Input::new(&message_input))),
            )
            .child(div().flex().gap(px(8.0)).justify_end().children(vec![
                btn(
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
                .into_any_element(),
                btn(
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
            .debug_selector(|| "body".to_owned())
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
    }
}

fn btn(
    label: &'static str,
    bg: Hsla,
    hover: Hsla,
    fg: Hsla,
    hfg: Hsla,
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
        .hover(|s| s.bg(hover).text_color(hfg))
        .on_mouse_down(MouseButton::Left, move |ev, w, cx| {
            handler(ev, w, cx);
        })
        .child(label)
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
                std::sync::Arc::new(ExecutionEventCollector::new()),
            )
        });
        cx.run_until_parked();

        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let body = cx.debug_bounds("body").expect("conversation body");

        assert!(body.size.width > px(0.0));
    }
}
