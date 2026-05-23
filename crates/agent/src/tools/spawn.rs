//! Spawn tool for launching background subagents.
//! Port of `nanobot.agent.tools.spawn`.
//!
//! The Python original delegates to `SubagentManager.spawn(...)`. The
//! Rust port exposes a closure-shaped `SpawnCallback` so the integrating
//! layer can plug in its own subagent factory (and provide a
//! pre-populated [`ToolRegistry`] per spawn).

use std::sync::Arc;
use std::sync::Mutex;

use async_trait::async_trait;
use futures::future::BoxFuture;
use serde_json::{json, Value};

use super::base::{Tool, ToolExecError};
use super::context::RequestContext;

/// Arguments delivered to the spawn callback.
pub struct SpawnRequest {
    pub task: String,
    pub label: Option<String>,
    pub origin_channel: String,
    pub origin_chat_id: String,
    pub session_key: String,
}

/// Async callback that performs the actual spawn and returns a
/// human-readable status string.
pub type SpawnCallback =
    Arc<dyn Fn(SpawnRequest) -> BoxFuture<'static, String> + Send + Sync>;

#[derive(Clone, Default, Debug)]
pub struct SpawnContext {
    pub origin_channel: String,
    pub origin_chat_id: String,
    pub session_key: String,
}

pub struct SpawnTool {
    callback: SpawnCallback,
    context: Arc<Mutex<SpawnContext>>,
}

impl SpawnTool {
    pub fn new(callback: SpawnCallback) -> Self {
        Self {
            callback,
            context: Arc::new(Mutex::new(SpawnContext {
                origin_channel: "cli".into(),
                origin_chat_id: "direct".into(),
                session_key: "cli:direct".into(),
            })),
        }
    }

    pub fn set_context(&self, channel: &str, chat_id: &str, session_key: Option<&str>) {
        let mut ctx = self.context.lock().unwrap();
        ctx.origin_channel = channel.to_string();
        ctx.origin_chat_id = chat_id.to_string();
        ctx.session_key = session_key
            .map(String::from)
            .unwrap_or_else(|| format!("{channel}:{chat_id}"));
    }
}

#[async_trait]
impl Tool for SpawnTool {
    fn name(&self) -> &str {
        "spawn"
    }
    fn description(&self) -> String {
        "Spawn a subagent to handle a task in the background. Use this for complex or time-consuming tasks that can run independently. The subagent will complete the task and report back when done.".into()
    }
    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "task":{"type":"string","description":"The task for the subagent to complete"},
                "label":{"type":"string","description":"Optional short label for display"},
            },
            "required":["task"],
        })
    }
    async fn execute(&self, params: Value) -> Result<Value, ToolExecError> {
        let Some(task) = params.get("task").and_then(|v| v.as_str()) else {
            return Ok(Value::String("Error: task is required".into()));
        };
        let label = params
            .get("label")
            .and_then(|v| v.as_str())
            .map(String::from);
        let ctx = self.context.lock().unwrap().clone();
        let req = SpawnRequest {
            task: task.to_string(),
            label,
            origin_channel: ctx.origin_channel,
            origin_chat_id: ctx.origin_chat_id,
            session_key: ctx.session_key,
        };
        let fut = (self.callback)(req);
        let result = fut.await;
        Ok(Value::String(result))
    }

    fn set_tool_context(&self, ctx: &RequestContext) {
        let session_key = ctx.session_key.clone()
            .unwrap_or_else(|| format!("{}:{}", ctx.channel, ctx.chat_id));
        let mut guard = self.context.lock().unwrap();
        guard.origin_channel = ctx.channel.clone();
        guard.origin_chat_id = ctx.chat_id.clone();
        guard.session_key = session_key;
    }
}
