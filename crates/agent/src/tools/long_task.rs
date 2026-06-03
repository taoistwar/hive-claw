use async_trait::async_trait;
use chrono::Utc;
use log::warn;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;

use super::base::{Tool, ToolExecError};
use super::context::{ContextAware, RequestContext, ToolContext};

const GOAL_STATE_KEY: &str = "goal_state";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GoalState {
    pub status: String,
    pub objective: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ui_summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recap: Option<String>,
}

fn iso_now() -> String {
    Utc::now().to_rfc3339()
}

pub fn parse_goal_state(raw: Option<&Value>) -> Option<GoalState> {
    raw.and_then(|v| serde_json::from_value::<GoalState>(v.clone()).ok())
}

pub fn goal_state_raw(metadata: &HashMap<String, Value>) -> Option<&Value> {
    metadata.get(GOAL_STATE_KEY)
}

pub fn goal_state_ws_blob(_metadata: &HashMap<String, Value>) -> Value {
    todo!("TODO: implement WebSocket goal state serialization")
}

pub fn discard_legacy_goal_state_key(_metadata: &mut HashMap<String, Value>) {
    todo!("TODO: implement legacy goal state key cleanup")
}

pub struct Session {
    pub key: String,
    pub metadata: HashMap<String, Value>,
}

impl Session {
    pub fn new(key: String) -> Self {
        Self {
            key,
            metadata: HashMap::new(),
        }
    }
}

trait GoalToolsMixin: ContextAware {
    fn session_manager(&self) -> Option<Arc<session::manager::SessionManager>>;
    fn message_bus(&self) -> Option<Arc<bus::MessageBus>>;
    fn request_ctx(&self) -> Option<&RequestContext>;

    fn session(&self) -> Option<Arc<Session>> {
        let _ctx = self.request_ctx()?;
        let _sessions = self.session_manager()?;
        todo!("TODO: adapt session::manager::SessionManager to return Arc<long_task::Session>")
    }

    async fn publish_goal_state_ws(&self, _metadata: &HashMap<String, Value>) {
        let _bus = self.message_bus();
        let rc = match self.request_ctx() {
            Some(r) => r,
            None => return,
        };
        let _cid = rc.chat_id.trim();
        if _cid.is_empty() {
            return;
        }
        todo!("TODO: publish goal state to WebSocket bus")
    }
}

pub struct LongTaskTool {
    sessions: Option<Arc<session::manager::SessionManager>>,
    bus: Option<Arc<bus::MessageBus>>,
    request_ctx: Option<RequestContext>,
}

impl LongTaskTool {
    pub fn new(
        sessions: Option<Arc<session::manager::SessionManager>>,
        bus: Option<Arc<bus::MessageBus>>,
    ) -> Self {
        Self {
            sessions,
            bus,
            request_ctx: None,
        }
    }

    pub fn create(ctx: &ToolContext) -> Self {
        Self {
            sessions: ctx.session_manager.clone(),
            bus: ctx.bus.clone(),
            request_ctx: None,
        }
    }

    pub fn enabled(ctx: &ToolContext) -> bool {
        ctx.session_manager.is_some()
    }
}

impl ContextAware for LongTaskTool {
    fn set_context(&mut self, ctx: &RequestContext) {
        self.request_ctx = Some(ctx.clone());
    }
}

impl GoalToolsMixin for LongTaskTool {
    fn session_manager(&self) -> Option<Arc<session::manager::SessionManager>> {
        self.sessions.clone()
    }

    fn message_bus(&self) -> Option<Arc<bus::MessageBus>> {
        self.bus.clone()
    }

    fn request_ctx(&self) -> Option<&RequestContext> {
        self.request_ctx.as_ref()
    }
}

#[async_trait]
impl Tool for LongTaskTool {
    fn name(&self) -> &str {
        "long_task"
    }

    fn description(&self) -> String {
        "Mark this thread as a sustained long-running task. First read the built-in **long-goal** skill, especially its Start fast section; then call this as soon as the user's intent is clear. Write a good idempotent goal, but do not delay the tool call with long planning, research, or execution-detail thinking. The active goal is mirrored in Runtime Context each turn. Use normal tools until done, then call complete_goal when the objective is satisfied, cancelled, or replaced. If a goal is already active, finish it or call complete_goal before registering another.".into()
    }

    fn parameters(&self) -> Value {
        let mut props = serde_json::Map::new();

        props.insert(
            "goal".into(),
            json!({
                "type": "string",
                "maxLength": 12000,
                "description": "Sustained objective for this chat thread. First read the built-in **long-goal** skill, especially its Start fast section, then call this promptly once the user's intent is clear. The goal must still be idempotent, self-contained, bounded, and explicit about done-ness; do not delay this tool call to over-plan, research, or decide execution details."
            }),
        );

        props.insert(
            "ui_summary".into(),
            json!({
                "type": ["string", "null"],
                "maxLength": 120,
                "description": "Optional one-line label for session lists / logs (<=120 chars)."
            }),
        );

        json!({
            "type": "object",
            "properties": props,
            "required": ["goal"]
        })
    }

    async fn execute(&self, params: Value) -> Result<Value, ToolExecError> {
        let goal = params
            .get("goal")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolExecError::InvalidParams("missing required parameter: goal".into()))?
            .to_string();

        let ui_summary = params
            .get("ui_summary")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let session = match self.session() {
            Some(s) => s,
            None => {
                return Ok(Value::String(
                    "Error: long_task requires an active chat session (missing routing context)."
                        .into(),
                ));
            }
        };

        let prior = parse_goal_state(goal_state_raw(&session.metadata));
        if let Some(ref p) = prior {
            if p.status == "active" {
                return Ok(Value::String(
                    "Error: a sustained goal is already active. Use complete_goal when finished, or ask the user before replacing it.".into(),
                ));
            }
        }

        let summary = ui_summary
            .unwrap_or_default()
            .trim()
            .chars()
            .take(120)
            .collect::<String>();

        let blob = GoalState {
            status: "active".into(),
            objective: goal.trim().to_string(),
            ui_summary: if summary.is_empty() {
                None
            } else {
                Some(summary.clone())
            },
            started_at: Some(iso_now()),
            completed_at: None,
            recap: None,
        };

        let blob_value = serde_json::to_value(&blob).unwrap_or(Value::Null);
        if let Some(session_mut) = Arc::get_mut(&mut session.clone()) {
            session_mut
                .metadata
                .insert(GOAL_STATE_KEY.into(), blob_value);
            discard_legacy_goal_state_key(&mut session_mut.metadata);
        } else {
            warn!("Could not get mutable reference to session for goal state update");
        }

        self.publish_goal_state_ws(&session.metadata).await;

        if let Some(_sm) = &self.sessions {
            todo!("TODO: save session via session::manager::SessionManager");
        }

        let extra = if !summary.is_empty() {
            format!("\nSummary line: {}", summary)
        } else {
            String::new()
        };

        Ok(Value::String(format!(
            "Goal recorded. Keep working toward the objective using ordinary tools. When fully done (verified against what was asked), call complete_goal with a short recap.{}",
            extra
        )))
    }
}

pub struct CompleteGoalTool {
    sessions: Option<Arc<session::manager::SessionManager>>,
    bus: Option<Arc<bus::MessageBus>>,
    request_ctx: Option<RequestContext>,
}

impl CompleteGoalTool {
    pub fn new(
        sessions: Option<Arc<session::manager::SessionManager>>,
        bus: Option<Arc<bus::MessageBus>>,
    ) -> Self {
        Self {
            sessions,
            bus,
            request_ctx: None,
        }
    }

    pub fn create(ctx: &ToolContext) -> Self {
        Self {
            sessions: ctx.session_manager.clone(),
            bus: ctx.bus.clone(),
            request_ctx: None,
        }
    }

    pub fn enabled(ctx: &ToolContext) -> bool {
        ctx.session_manager.is_some()
    }
}

impl ContextAware for CompleteGoalTool {
    fn set_context(&mut self, ctx: &RequestContext) {
        self.request_ctx = Some(ctx.clone());
    }
}

impl GoalToolsMixin for CompleteGoalTool {
    fn session_manager(&self) -> Option<Arc<session::manager::SessionManager>> {
        self.sessions.clone()
    }

    fn message_bus(&self) -> Option<Arc<bus::MessageBus>> {
        self.bus.clone()
    }

    fn request_ctx(&self) -> Option<&RequestContext> {
        self.request_ctx.as_ref()
    }
}

#[async_trait]
impl Tool for CompleteGoalTool {
    fn name(&self) -> &str {
        "complete_goal"
    }

    fn description(&self) -> String {
        "End bookkeeping for the active sustained goal. Use when the objective is fully achieved and verified—recap what was delivered. Also call when the user cancels, redirects, or replaces the goal: recap must reflect what actually happened (not necessarily success). If no goal is active, the tool reports that and leaves metadata unchanged.".into()
    }

    fn parameters(&self) -> Value {
        let mut props = serde_json::Map::new();

        props.insert(
            "recap".into(),
            json!({
                "type": ["string", "null"],
                "maxLength": 8000,
                "description": "Brief recap for the user (plain text). When the goal succeeded, confirm outcomes; if the user cancelled, pivoted, or replaced the objective, say so honestly."
            }),
        );

        json!({
            "type": "object",
            "properties": props,
            "required": []
        })
    }

    async fn execute(&self, params: Value) -> Result<Value, ToolExecError> {
        let recap = params
            .get("recap")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let session = match self.session() {
            Some(s) => s,
            None => {
                return Ok(Value::String(
                    "Error: complete_goal requires an active chat session.".into(),
                ));
            }
        };

        let prior = parse_goal_state(goal_state_raw(&session.metadata));
        match prior {
            Some(ref p) if p.status == "active" => {}
            _ => {
                return Ok(Value::String("No active goal to complete.".into()));
            }
        }

        let ended = iso_now();
        let recap_str = recap.unwrap_or_default().trim().to_string();

        let blob = GoalState {
            status: "completed".into(),
            objective: prior
                .as_ref()
                .map(|p| p.objective.clone())
                .unwrap_or_default(),
            ui_summary: prior.as_ref().and_then(|p| p.ui_summary.clone()),
            started_at: prior.as_ref().and_then(|p| p.started_at.clone()),
            completed_at: Some(ended.clone()),
            recap: if recap_str.is_empty() {
                None
            } else {
                Some(recap_str.clone())
            },
        };

        let blob_value = serde_json::to_value(&blob).unwrap_or(Value::Null);
        if let Some(session_mut) = Arc::get_mut(&mut session.clone()) {
            session_mut
                .metadata
                .insert(GOAL_STATE_KEY.into(), blob_value);
            discard_legacy_goal_state_key(&mut session_mut.metadata);
        } else {
            warn!("Could not get mutable reference to session for goal state update");
        }

        self.publish_goal_state_ws(&session.metadata).await;

        if let Some(_sm) = &self.sessions {
            todo!("TODO: save session via session::manager::SessionManager");
        }

        let tail = recap_str.trim().to_string();
        if !tail.is_empty() {
            return Ok(Value::String(format!(
                "Goal marked complete ({}). Recap:\n{}",
                ended, tail
            )));
        }

        Ok(Value::String(format!("Goal marked complete ({}).", ended)))
    }
}
