use std::sync::Arc;

use async_trait::async_trait;
use log::debug;
use serde_json::Value;
use tokio::sync::Mutex;

use providers::ToolCallRequest;

use crate::hook::{AgentHook, AgentHookContext};

type ProgressFn = Arc<dyn Fn(ProgressPayload) + Send + Sync>;
type StreamFn = Arc<dyn Fn(String) + Send + Sync>;
type StreamEndFn = Arc<dyn Fn(bool) + Send + Sync>;

#[derive(Debug, Clone)]
pub enum ProgressPayload {
    Thought(String),
    ToolHint(String),
    ToolEvents(Vec<ToolEventPayload>),
    Reasoning(String),
    ReasoningEnd,
}

#[derive(Debug, Clone)]
pub struct ToolEventPayload {
    pub tool_call_id: String,
    pub name: String,
    pub status: String,
    pub detail: String,
}

pub struct ProgressHook {
    on_progress: Option<ProgressFn>,
    on_stream: Option<StreamFn>,
    on_stream_end: Option<StreamEndFn>,
    #[allow(dead_code)]
    channel: String,
    #[allow(dead_code)]
    chat_id: String,
    #[allow(dead_code)]
    message_id: Option<String>,
    #[allow(dead_code)]
    metadata: serde_json::Map<String, Value>,
    session_key: Option<String>,
    tool_hint_max_length: usize,
    stream_buf: Arc<Mutex<String>>,
    reasoning_open: Arc<Mutex<bool>>,
}

impl ProgressHook {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        on_progress: Option<ProgressFn>,
        on_stream: Option<StreamFn>,
        on_stream_end: Option<StreamEndFn>,
        channel: String,
        chat_id: String,
        message_id: Option<String>,
        metadata: serde_json::Map<String, Value>,
        session_key: Option<String>,
        tool_hint_max_length: usize,
    ) -> Self {
        Self {
            on_progress,
            on_stream,
            on_stream_end,
            channel,
            chat_id,
            message_id,
            metadata,
            session_key,
            tool_hint_max_length,
            stream_buf: Arc::new(Mutex::new(String::new())),
            reasoning_open: Arc::new(Mutex::new(false)),
        }
    }

    fn tool_hint(tool_calls: &[ToolCallRequest], max_length: usize) -> String {
        let hints: Vec<String> = tool_calls
            .iter()
            .map(|tc| format!("{}(...)", tc.name))
            .collect();
        let joined = hints.join(", ");
        if joined.len() > max_length {
            format!("{}...", &joined[..max_length.saturating_sub(3)])
        } else {
            joined
        }
    }
}

#[async_trait]
impl AgentHook for ProgressHook {
    fn wants_streaming(&self) -> bool {
        self.on_stream.is_some()
    }

    async fn before_iteration(&self, ctx: &mut AgentHookContext) {
        debug!(
            "Starting agent loop iteration {} for session {}",
            ctx.iteration,
            self.session_key.as_deref().unwrap_or("?"),
        );
    }

    async fn on_stream(&self, _ctx: &mut AgentHookContext, delta: &str) {
        if self.on_stream.is_none() {
            return;
        }
        let prev_clean = {
            let buf = self.stream_buf.lock().await;
            strip_think(&buf).unwrap_or_default()
        };
        {
            let mut buf = self.stream_buf.lock().await;
            buf.push_str(delta);
        }
        let new_clean = {
            let buf = self.stream_buf.lock().await;
            strip_think(&buf).unwrap_or_default()
        };
        let incremental = if new_clean.len() > prev_clean.len() {
            new_clean[prev_clean.len()..].to_string()
        } else {
            String::new()
        };

        if !incremental.is_empty() {
            let mut open = self.reasoning_open.lock().await;
            if *open {
                *open = false;
                if let Some(ref cb) = self.on_progress {
                    cb(ProgressPayload::ReasoningEnd);
                }
            }
            drop(open);
            if let Some(ref cb) = self.on_stream {
                cb(incremental);
            }
        }
    }

    async fn on_stream_end(&self, _ctx: &mut AgentHookContext, resuming: bool) {
        {
            let mut open = self.reasoning_open.lock().await;
            if *open {
                *open = false;
                if let Some(ref cb) = self.on_progress {
                    cb(ProgressPayload::ReasoningEnd);
                }
            }
            drop(open);
        }
        {
            let mut buf = self.stream_buf.lock().await;
            buf.clear();
        }
        if let Some(ref cb) = self.on_stream_end {
            cb(resuming);
        }
    }

    async fn before_execute_tools(&self, ctx: &mut AgentHookContext) {
        if let Some(ref cb) = self.on_progress {
            if !ctx.streamed_content && ctx.response.is_some() {
                if let Some(ref resp) = ctx.response {
                    if let Some(thought) = strip_think(resp.content.as_deref().unwrap_or("")) {
                        if !thought.is_empty() {
                            cb(ProgressPayload::Thought(thought));
                        }
                    }
                }
            }
            if !ctx.tool_calls.is_empty() {
                let hint = Self::tool_hint(&ctx.tool_calls, self.tool_hint_max_length);
                if let Some(th) = strip_think(&hint) {
                    cb(ProgressPayload::ToolHint(th));
                }
                let events: Vec<ToolEventPayload> = ctx
                    .tool_calls
                    .iter()
                    .map(|tc| ToolEventPayload {
                        tool_call_id: tc.id.clone(),
                        name: tc.name.clone(),
                        status: "start".into(),
                        detail: String::new(),
                    })
                    .collect();
                cb(ProgressPayload::ToolEvents(events));
            }
        }
    }

    async fn after_iteration(&self, ctx: &mut AgentHookContext) {
        if self.on_progress.is_some() && !ctx.tool_calls.is_empty() && !ctx.tool_events.is_empty() {
            let events: Vec<ToolEventPayload> = ctx
                .tool_events
                .iter()
                .map(|te| ToolEventPayload {
                    tool_call_id: String::new(),
                    name: te.name.clone(),
                    status: te.status.clone(),
                    detail: te.detail.clone(),
                })
                .collect();
            if !events.is_empty() {
                self.on_progress.as_ref().unwrap()(ProgressPayload::ToolEvents(events));
            }
        }
        if let Some(ref resp) = ctx.response {
            if !resp.usage.is_empty() {
                debug!(
                    "LLM usage: prompt={} completion={}",
                    resp.usage.get("prompt_tokens").copied().unwrap_or(0),
                    resp.usage.get("completion_tokens").copied().unwrap_or(0),
                );
            }
        }
    }

    fn finalize_content(
        &self,
        _ctx: &mut AgentHookContext,
        content: Option<String>,
    ) -> Option<String> {
        content.map(|s| strip_think(&s).unwrap_or_else(|| s))
    }
}

fn strip_think(text: &str) -> Option<String> {
    let mut result = text.to_string();
    while let Some(start) = result.find("<think>") {
        if let Some(end) = result[start..].find("</think>") {
            result.replace_range(start..start + end + "</think>".len(), "");
        } else {
            break;
        }
    }
    let trimmed = result.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}
