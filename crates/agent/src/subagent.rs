//! Subagent manager for background task execution.
//! Port of `nanobot.agent.subagent`.
//!
//! This Rust port exposes the orchestration surface (spawn / cancel /
//! running counts) and the `_SubagentHook` used by the Python original.
//! Tool registration for subagents is left as a caller responsibility —
//! pass a pre-populated [`ToolRegistry`] into [`SubagentManager`].

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use async_trait::async_trait;
use log::{debug, info};
use serde_json::Value;
use tokio::task::JoinHandle;
use uuid::Uuid;

use bus::{InboundMessage, MessageBus};
use providers::LLMProvider;

use crate::hook::{AgentHook, AgentHookContext, ToolEvent};
use crate::runner::{AgentRunResult, AgentRunSpec, AgentRunner};
use crate::tools::ToolRegistry;

/// Real-time status of a running subagent.
#[derive(Debug, Clone)]
pub struct SubagentStatus {
    pub task_id: String,
    pub label: String,
    pub task_description: String,
    pub started_at: Instant,
    pub phase: String,
    pub iteration: usize,
    pub tool_events: Vec<ToolEvent>,
    pub usage: HashMap<String, i64>,
    pub stop_reason: Option<String>,
    pub error: Option<String>,
}

impl SubagentStatus {
    fn new(task_id: String, label: String, description: String) -> Self {
        Self {
            task_id,
            label,
            task_description: description,
            started_at: Instant::now(),
            phase: "initializing".into(),
            iteration: 0,
            tool_events: Vec::new(),
            usage: HashMap::new(),
            stop_reason: None,
            error: None,
        }
    }
}

/// Hook for subagent execution — logs tool calls and updates status.
pub struct SubagentHook {
    task_id: String,
    status: Arc<Mutex<SubagentStatus>>,
}

impl SubagentHook {
    pub fn new(task_id: String, status: Arc<Mutex<SubagentStatus>>) -> Self {
        Self { task_id, status }
    }
}

#[async_trait]
impl AgentHook for SubagentHook {
    async fn before_execute_tools(&self, ctx: &mut AgentHookContext) {
        for tc in &ctx.tool_calls {
            let args_str = serde_json::to_string(&tc.arguments).unwrap_or_else(|_| "{}".into());
            debug!(
                "Subagent [{}] executing: {} with arguments: {}",
                self.task_id, tc.name, args_str
            );
        }
    }
    async fn after_iteration(&self, ctx: &mut AgentHookContext) {
        let mut status = self.status.lock().unwrap();
        status.iteration = ctx.iteration;
        status.tool_events = ctx.tool_events.clone();
        status.usage = ctx.usage.clone();
        if let Some(err) = &ctx.error {
            status.error = Some(err.clone());
        }
    }
}

/// Configuration for the subagent manager.
pub struct SubagentConfig {
    pub workspace: std::path::PathBuf,
    pub max_tool_result_chars: usize,
    pub model: Option<String>,
    pub restrict_to_workspace: bool,
    pub disabled_skills: HashSet<String>,
    /// Maximum iterations for each spawned subagent.
    pub max_iterations: u32,
    /// Optional per-session LLM wall timeout override (seconds).
    pub llm_wall_timeout_for_session:
        Option<Arc<dyn Fn(Option<&str>) -> Option<f64> + Send + Sync>>,
}

/// Manages background subagent execution.
pub struct SubagentManager {
    provider: Arc<dyn LLMProvider>,
    config: SubagentConfig,
    bus: Arc<MessageBus>,
    runner: Arc<AgentRunner>,
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    running_tasks: HashMap<String, JoinHandle<()>>,
    task_statuses: HashMap<String, Arc<Mutex<SubagentStatus>>>,
    session_tasks: HashMap<String, HashSet<String>>,
}

impl SubagentManager {
    pub fn new(
        provider: Arc<dyn LLMProvider>,
        bus: Arc<MessageBus>,
        config: SubagentConfig,
    ) -> Self {
        let runner = Arc::new(AgentRunner::new(provider.clone()));
        Self {
            provider,
            config,
            bus,
            runner,
            inner: Mutex::new(Inner::default()),
        }
    }

    /// Spawn a subagent against the pre-built tool registry.
    ///
    /// The caller is responsible for registering appropriate tools
    /// (filesystem/web/exec/...) before calling this method.
    pub fn spawn(
        self: Arc<Self>,
        tools: ToolRegistry,
        task: String,
        label: Option<String>,
        origin_channel: String,
        origin_chat_id: String,
        session_key: Option<String>,
        origin_message_id: Option<String>,
    ) -> String {
        let task_id = Uuid::new_v4().to_string()[..8].to_string();
        let display_label = label.unwrap_or_else(|| {
            let trimmed: String = task.chars().take(30).collect();
            if task.chars().count() > 30 {
                format!("{trimmed}...")
            } else {
                trimmed
            }
        });
        let status = Arc::new(Mutex::new(SubagentStatus::new(
            task_id.clone(),
            display_label.clone(),
            task.clone(),
        )));
        self.inner
            .lock()
            .unwrap()
            .task_statuses
            .insert(task_id.clone(), status.clone());
        if let Some(key) = session_key.as_ref() {
            self.inner
                .lock()
                .unwrap()
                .session_tasks
                .entry(key.clone())
                .or_default()
                .insert(task_id.clone());
        }

        let this = Arc::clone(&self);
        let tid_for_task = task_id.clone();
        let label_for_task = display_label.clone();
        let origin_channel_c = origin_channel.clone();
        let origin_chat_id_c = origin_chat_id.clone();
        let session_key_c = session_key.clone();
        let origin_message_id_c = origin_message_id.clone();
        let handle = tokio::spawn(async move {
            this.run_subagent(
                &tid_for_task,
                &task,
                &label_for_task,
                (
                    &origin_channel_c,
                    &origin_chat_id_c,
                    session_key_c.as_deref(),
                ),
                status,
                tools,
                origin_message_id_c.as_deref(),
            )
            .await;

            let mut inner = this.inner.lock().unwrap();
            inner.running_tasks.remove(&tid_for_task);
            inner.task_statuses.remove(&tid_for_task);
            if let Some(key) = &session_key_c {
                if let Some(ids) = inner.session_tasks.get_mut(key) {
                    ids.remove(&tid_for_task);
                    if ids.is_empty() {
                        inner.session_tasks.remove(key);
                    }
                }
            }
        });
        self.inner
            .lock()
            .unwrap()
            .running_tasks
            .insert(task_id.clone(), handle);

        info!("Spawned subagent [{task_id}]: {display_label}");
        format!(
            "Subagent [{display_label}] started (id: {task_id}). I'll notify you when it completes."
        )
    }

    async fn run_subagent(
        &self,
        task_id: &str,
        task: &str,
        label: &str,
        origin: (&str, &str, Option<&str>),
        status: Arc<Mutex<SubagentStatus>>,
        tools: ToolRegistry,
        origin_message_id: Option<&str>,
    ) {
        info!("Subagent [{task_id}] starting task: {label}");
        let model = self
            .config
            .model
            .clone()
            .unwrap_or_else(|| self.provider.default_model());
        let system_prompt = self.build_subagent_prompt();
        let messages = vec![
            serde_json::json!({"role":"system","content":system_prompt}),
            serde_json::json!({"role":"user","content":task}),
        ];

        let mut spec = AgentRunSpec::new(
            messages,
            tools,
            model,
            self.config.max_iterations,
            self.config.max_tool_result_chars,
        );
        spec.hook = Some(Arc::new(SubagentHook::new(
            task_id.to_string(),
            status.clone(),
        )));
        spec.max_iterations_message =
            Some("Task completed but no final response was generated.".into());
        spec.error_message = None;
        spec.fail_on_tool_error = true;
        spec.session_key = origin.2.map(String::from);
        spec.llm_timeout_s = self
            .config
            .llm_wall_timeout_for_session
            .as_ref()
            .and_then(|f| f(origin.2));

        let result = self.runner.run(spec).await;
        {
            let mut s = status.lock().unwrap();
            s.phase = "done".into();
            s.stop_reason = Some(result.stop_reason.clone());
        }

        match result.stop_reason.as_str() {
            "tool_error" => {
                {
                    let mut s = status.lock().unwrap();
                    s.tool_events = result.tool_events.clone();
                }
                let detail = format_partial_progress(&result);
                self.announce_result(
                    task_id,
                    label,
                    task,
                    &detail,
                    origin,
                    "error",
                    origin_message_id,
                )
                .await;
            }
            "error" => {
                let detail = result
                    .error
                    .clone()
                    .unwrap_or_else(|| "Error: subagent execution failed.".into());
                self.announce_result(
                    task_id,
                    label,
                    task,
                    &detail,
                    origin,
                    "error",
                    origin_message_id,
                )
                .await;
            }
            _ => {
                let final_result = result.final_content.clone().unwrap_or_else(|| {
                    "Task completed but no final response was generated.".into()
                });
                info!("Subagent [{task_id}] completed successfully");
                self.announce_result(
                    task_id,
                    label,
                    task,
                    &final_result,
                    origin,
                    "ok",
                    origin_message_id,
                )
                .await;
            }
        }
    }

    async fn announce_result(
        &self,
        task_id: &str,
        label: &str,
        task: &str,
        result: &str,
        origin: (&str, &str, Option<&str>),
        status: &str,
        origin_message_id: Option<&str>,
    ) {
        let (channel, chat_id, session_key) = origin;
        let status_text = if status == "ok" {
            "completed successfully"
        } else {
            "failed"
        };
        let announce_content = format!(
            "Subagent [{label}] {status_text}\n\nOriginal task:\n{task}\n\nResult:\n{result}"
        );
        let override_key = session_key
            .map(String::from)
            .unwrap_or_else(|| format!("{channel}:{chat_id}"));
        let mut metadata = HashMap::new();
        metadata.insert(
            "injected_event".to_string(),
            Value::String("subagent_result".into()),
        );
        metadata.insert(
            "subagent_task_id".to_string(),
            Value::String(task_id.to_string()),
        );
        if let Some(msg_id) = origin_message_id {
            metadata.insert(
                "origin_message_id".to_string(),
                Value::String(msg_id.to_string()),
            );
        }

        let msg = InboundMessage {
            channel: "system".into(),
            sender_id: "subagent".into(),
            chat_id: format!("{channel}:{chat_id}"),
            content: announce_content,
            session_key_override: Some(override_key),
            metadata,
            timestamp: chrono::Local::now(),
            media: Vec::new(),
        };
        self.bus.publish_inbound(msg).await;
        debug!("Subagent [{task_id}] announced result to {channel}:{chat_id}");
    }

    fn build_subagent_prompt(&self) -> String {
        let time_ctx =
            crate::context::ContextBuilder::build_runtime_context(None, None, None, None, None);
        let skills_loader = crate::skills::SkillsLoader::new(
            self.config.workspace.clone(),
            Some(self.config.disabled_skills.clone()),
        );
        let skills_summary = skills_loader.build_skills_summary(Some(&HashSet::new()));
        format!(
            "You are a focused background subagent.\n\n{time_ctx}\n\nWorkspace: {}\n\n{}",
            self.config.workspace.display(),
            skills_summary
        )
    }

    /// Cancel all subagents for the given session. Returns the list of cancelled subagent IDs.
    pub fn cancel_by_session(&self, session_key: &str) -> Vec<String> {
        let mut inner = self.inner.lock().unwrap();
        let ids: Vec<String> = inner
            .session_tasks
            .get(session_key)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .collect();
        let tasks: Vec<_> = ids
            .iter()
            .filter_map(|tid| inner.running_tasks.remove(tid))
            .collect();
        // Clean up task_statuses for cancelled tasks
        for tid in &ids {
            inner.task_statuses.remove(tid);
        }
        // Clean up session_tasks entry
        inner.session_tasks.remove(session_key);
        // Drop the lock before aborting tasks
        drop(inner);
        for t in &tasks {
            t.abort();
        }
        ids
    }

    pub fn running_count(&self) -> usize {
        self.inner.lock().unwrap().running_tasks.len()
    }

    pub fn running_count_by_session(&self, session_key: &str) -> usize {
        let inner = self.inner.lock().unwrap();
        inner
            .session_tasks
            .get(session_key)
            .map(|ids| {
                ids.iter()
                    .filter(|tid| inner.running_tasks.contains_key(*tid))
                    .count()
            })
            .unwrap_or(0)
    }
}

fn format_partial_progress(result: &AgentRunResult) -> String {
    let completed: Vec<&ToolEvent> = result
        .tool_events
        .iter()
        .filter(|e| e.status == "ok")
        .collect();
    let failure = result
        .tool_events
        .iter()
        .rev()
        .find(|e| e.status == "error");
    let mut lines: Vec<String> = Vec::new();
    if !completed.is_empty() {
        lines.push("Completed steps:".into());
        for event in completed
            .iter()
            .rev()
            .take(3)
            .collect::<Vec<_>>()
            .iter()
            .rev()
        {
            lines.push(format!("- {}: {}", event.name, event.detail));
        }
    }
    if let Some(f) = failure {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push("Failure:".into());
        lines.push(format!("- {}: {}", f.name, f.detail));
    }
    if let Some(err) = &result.error {
        if failure.is_none() {
            if !lines.is_empty() {
                lines.push(String::new());
            }
            lines.push("Failure:".into());
            lines.push(format!("- {err}"));
        }
    }
    if lines.is_empty() {
        result
            .error
            .clone()
            .unwrap_or_else(|| "Error: subagent execution failed.".into())
    } else {
        lines.join("\n")
    }
}
