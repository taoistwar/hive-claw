//! Heartbeat service - periodic agent wake-up to check for tasks.
//! Port of `nanobot.heartbeat.service`.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::LazyLock;

use async_trait::async_trait;
use log::{debug, info, warn};
use providers::base::{ChatRequest, LLMProvider, RetryMode};
use serde_json::json;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use utils::evaluator::NotificationEvaluator;

static HEARTBEAT_TOOL: LazyLock<serde_json::Value> = LazyLock::new(|| {
    json!(
        {
            "type": "function",
            "function": {
                "name": "heartbeat",
                "description": "Report heartbeat decision after reviewing tasks.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "action": {
                            "type": "string",
                            "enum": ["skip", "run"],
                            "description": "skip = nothing to do, run = has active tasks"
                        },
                        "tasks": {
                            "type": "string",
                            "description": "Natural-language summary of active tasks (required for run)"
                        }
                    },
                    "required": ["action"]
                }
            }
        }
    )
});

/// Possible outcomes of the Phase-1 "decide" call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeartbeatAction {
    Skip,
    Run,
}

/// Result of the decision phase.
#[derive(Debug, Clone)]
pub struct HeartbeatDecision {
    pub action: HeartbeatAction,
    pub tasks: String,
}

/// Plug-in that executes a task and returns the resulting text.
#[async_trait]
pub trait HeartbeatExecutor: Send + Sync {
    async fn execute(&self, tasks: &str) -> Option<String>;
}

/// Plug-in that delivers a successful run's response text.
#[async_trait]
pub trait HeartbeatNotifier: Send + Sync {
    async fn notify(&self, text: &str);
}

/// Heartbeat configuration.
#[derive(Debug, Clone)]
pub struct HeartbeatConfig {
    pub workspace: PathBuf,
    pub interval_s: u64,
    pub enabled: bool,
    pub timezone: Option<String>,
    pub model: String,
}

impl HeartbeatConfig {
    pub fn new(workspace: PathBuf, model: String) -> Self {
        Self {
            workspace,
            interval_s: 30 * 60,
            enabled: true,
            timezone: None,
            model,
        }
    }
    pub fn with_interval_s(mut self, interval_s: u64) -> Self {
        self.interval_s = interval_s;
        self
    }
    pub fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
    pub fn with_timezone(mut self, timezone: Option<String>) -> Self {
        self.timezone = timezone;
        self
    }
}

/// Trait for deciding whether there are active tasks to process.
#[async_trait]
pub trait HeartbeatDecider: Send + Sync {
    async fn decide(&self, content: &str) -> HeartbeatDecision;
}

/// LLM-based heartbeat decider.
pub struct LLMHeartbeatDecider {
    provider: Arc<dyn LLMProvider>,
    model: String,
    timezone: Option<String>,
}

impl LLMHeartbeatDecider {
    pub fn new(provider: Arc<dyn LLMProvider>, model: String) -> Self {
        Self {
            provider,
            model,
            timezone: None,
        }
    }
    pub fn with_timezone(mut self, timezone: Option<String>) -> Self {
        self.timezone = timezone;
        self
    }
}

#[async_trait]
impl HeartbeatDecider for LLMHeartbeatDecider {
    async fn decide(&self, content: &str) -> HeartbeatDecision {
        let time_str = utils::helpers::current_time_str(self.timezone.as_deref());
        let messages = vec![
            json!({
                "role": "system",
                "content": "You are a heartbeat agent. Call the heartbeat tool to report your decision."
            }),
            json!({
                "role": "user",
                "content": format!(
                    "Current Time: {time_str}\n\nReview the following HEARTBEAT.md and decide whether there are active tasks.\n\n{content}"
                )
            }),
        ];

        let req = ChatRequest {
            messages,
            tools: Some(vec![HEARTBEAT_TOOL.clone()]),
            model: Some(self.model.clone()),
            max_tokens: 512,
            temperature: 0.0,
            reasoning_effort: None,
            tool_choice: None,
        };

        let response = self
            .provider
            .chat_with_retry(req, RetryMode::Standard, None)
            .await;

        if !response.should_execute_tools() {
            if response.has_tool_calls() {
                warn!(
                    "Ignoring heartbeat tool calls under finish_reason='{}'",
                    response.finish_reason
                );
            }
            warn!("Heartbeat LLM did not return tool calls");
            return HeartbeatDecision {
                action: HeartbeatAction::Skip,
                tasks: String::new(),
            };
        }

        let tool_calls = response.tool_calls;
        if tool_calls.is_empty() {
            return HeartbeatDecision {
                action: HeartbeatAction::Skip,
                tasks: String::new(),
            };
        }

        let call = &tool_calls[0];
        if call.name != "heartbeat" {
            warn!("Heartbeat LLM called unexpected tool '{}'", call.name);
            return HeartbeatDecision {
                action: HeartbeatAction::Skip,
                tasks: String::new(),
            };
        }

        let action_str = call
            .arguments
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("skip");
        let tasks = call
            .arguments
            .get("tasks")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        if action_str == "run" && !tasks.is_empty() {
            info!(
                "Heartbeat decision: run ({})",
                &tasks[..tasks.len().min(80)]
            );
            HeartbeatDecision {
                action: HeartbeatAction::Run,
                tasks,
            }
        } else {
            debug!("Heartbeat decision: skip");
            HeartbeatDecision {
                action: HeartbeatAction::Skip,
                tasks: String::new(),
            }
        }
    }
}

/// Periodic heartbeat service that wakes the agent to check for tasks.
pub struct HeartbeatService {
    config: HeartbeatConfig,
    decider: Arc<dyn HeartbeatDecider>,
    executor: Option<Arc<dyn HeartbeatExecutor>>,
    notifier: Option<Arc<dyn HeartbeatNotifier>>,
    evaluator: Option<Arc<dyn NotificationEvaluator>>,

    state: Arc<Mutex<State>>,
}

#[derive(Default)]
struct State {
    running: bool,
    task: Option<JoinHandle<()>>,
}

impl HeartbeatService {
    pub fn new(
        config: HeartbeatConfig,
        decider: Arc<dyn HeartbeatDecider>,
        executor: Option<Arc<dyn HeartbeatExecutor>>,
        notifier: Option<Arc<dyn HeartbeatNotifier>>,
        evaluator: Option<Arc<dyn NotificationEvaluator>>,
    ) -> Self {
        Self {
            config,
            decider,
            executor,
            notifier,
            evaluator,
            state: Arc::new(Mutex::new(State::default())),
        }
    }

    pub fn heartbeat_file(&self) -> PathBuf {
        self.config.workspace.join("HEARTBEAT.md")
    }

    fn read_heartbeat_file(&self) -> Option<String> {
        std::fs::read_to_string(self.heartbeat_file()).ok()
    }

    pub async fn start(self: Arc<Self>) {
        if !self.config.enabled {
            info!("Heartbeat disabled");
            return;
        }
        let mut state = self.state.lock().await;
        if state.running {
            warn!("Heartbeat already running");
            return;
        }

        state.running = true;
        let interval = self.config.interval_s;
        let this = self.clone();
        let handle = tokio::spawn(async move {
            loop {
                tokio::time::sleep(tokio::time::Duration::from_secs(interval)).await;
                let running = this.state.lock().await.running;
                if !running {
                    break;
                }
                // Spawn tick() in a sub-task so that a panic is caught as
                // a JoinError rather than aborting the entire loop.
                let tick_task = tokio::spawn({
                    let this = this.clone();
                    async move {
                        this.tick().await;
                    }
                });
                if let Err(e) = tick_task.await {
                    warn!("Heartbeat tick panicked: {e}");
                }
            }
        });
        state.task = Some(handle);
        info!("Heartbeat started (every {interval}s)");
    }

    pub fn stop(&self) {
        let mut state = self.state.blocking_lock();
        state.running = false;
        if let Some(handle) = state.task.take() {
            handle.abort();
        }
    }

    async fn tick(&self) {
        let Some(content) = self.read_heartbeat_file() else {
            debug!("Heartbeat: HEARTBEAT.md missing or empty");
            return;
        };
        if content.is_empty() {
            debug!("Heartbeat: HEARTBEAT.md empty");
            return;
        }

        info!("Heartbeat: checking for tasks...");
        let decision = self.decider.decide(&content).await;

        if decision.action != HeartbeatAction::Run {
            info!("Heartbeat: OK (nothing to report)");
            return;
        }
        info!("Heartbeat: tasks found, executing...");

        let Some(executor) = &self.executor else {
            return;
        };
        let Some(response) = executor.execute(&decision.tasks).await else {
            return;
        };
        if response.is_empty() {
            return;
        }
        if !Self::_is_deliverable(&response) {
            info!(
                "Heartbeat: suppressed non-deliverable response ({})",
                &response[..response.len().min(80)]
            );
            return;
        }
        let should_notify = utils::evaluator::evaluate_response(
            &response,
            &decision.tasks,
            self.evaluator.as_deref(),
            &self.config.model,
        )
        .await;
        match (should_notify, &self.notifier) {
            (true, Some(notifier)) => {
                info!("Heartbeat: completed, delivering response");
                notifier.notify(&response).await;
            }
            _ => info!("Heartbeat: silenced by post-run evaluation"),
        }
    }

    fn _is_deliverable(response: &str) -> bool {
        let text = response.to_lowercase();

        if text.contains("couldn't produce a final answer") {
            return false;
        }

        let leaked_patterns = [
            "heartbeat.md",
            "awareness.md",
            "judgment call:",
            "decision logic",
            "valid options are",
            "my instructions",
            "i am supposed to",
            "strict heartbeat interpretation",
        ];
        if leaked_patterns.iter().any(|p| text.contains(p)) {
            return false;
        }

        true
    }

    pub async fn trigger_now(&self) -> Option<String> {
        let content = self.read_heartbeat_file()?;
        if content.is_empty() {
            return None;
        }
        let decision = self.decider.decide(&content).await;
        if decision.action != HeartbeatAction::Run {
            return None;
        }
        let executor = self.executor.as_ref()?;
        executor.execute(&decision.tasks).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_deliverable_filters_runner_fallback() {
        assert!(!HeartbeatService::_is_deliverable(
            "I couldn't produce a final answer"
        ));
    }

    #[test]
    fn is_deliverable_filters_leaked_reasoning() {
        assert!(!HeartbeatService::_is_deliverable(
            "Based on heartbeat.md I decided..."
        ));
    }

    #[test]
    fn is_deliverable_accepts_normal_response() {
        assert!(HeartbeatService::_is_deliverable(
            "Database migration completed successfully."
        ));
    }
}
