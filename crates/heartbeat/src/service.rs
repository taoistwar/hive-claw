//! Heartbeat service - periodic agent wake-up to check for tasks.
//! Port of `nanobot.heartbeat.service`.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use log::{debug, info, warn};
use providers::{
    base::{ChatRequest, LLMProvider, RetryMode},
    types::ToolCallRequest,
};
use serde_json::json;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use utils::evaluator::NotificationEvaluator;

const HEARTBEAT_TOOL: serde_json::Value = serde_json::json!(
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
);

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

/// Periodic heartbeat service that wakes the agent to check for tasks.
pub struct HeartbeatService {
    workspace: PathBuf,
    interval_s: u64,
    enabled: bool,
    timezone: Option<String>,
    model: String,

    provider: Option<Arc<dyn LLMProvider>>,
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
        workspace: PathBuf,
        provider: Option<Arc<dyn LLMProvider>>,
        model: String,
        executor: Option<Arc<dyn HeartbeatExecutor>>,
        notifier: Option<Arc<dyn HeartbeatNotifier>>,
        evaluator: Option<Arc<dyn NotificationEvaluator>>,
        interval_s: u64,
        enabled: bool,
        timezone: Option<String>,
    ) -> Self {
        Self {
            workspace,
            interval_s,
            enabled,
            timezone,
            model,
            provider,
            executor,
            notifier,
            evaluator,
            state: Arc::new(Mutex::new(State::default())),
        }
    }

    pub fn heartbeat_file(&self) -> PathBuf {
        self.workspace.join("HEARTBEAT.md")
    }

    fn read_heartbeat_file(&self) -> Option<String> {
        std::fs::read_to_string(self.heartbeat_file()).ok()
    }

    async fn _decide(&self, content: &str) -> HeartbeatDecision {
        let Some(provider) = &self.provider else {
            warn!("No LLM provider configured for heartbeat");
            return HeartbeatDecision {
                action: HeartbeatAction::Skip,
                tasks: String::new(),
            };
        };

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

        let response = provider.chat_with_retry(req, RetryMode::Standard, None).await;

        if !response.should_execute_tools() {
            if response.has_tool_calls() {
                warn!(
                    "Ignoring heartbeat tool calls under finish_reason='{}'",
                    response.finish_reason
                );
            }
            return HeartbeatDecision {
                action: HeartbeatAction::Skip,
                tasks: String::new(),
            };
        }

        if let Some(tool_call) = response.tool_calls.first() {
            let action = tool_call
                .arguments
                .get("action")
                .and_then(|v| v.as_str())
                .unwrap_or("skip");
            let tasks = tool_call
                .arguments
                .get("tasks")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let action = if action == "run" {
                HeartbeatAction::Run
            } else {
                HeartbeatAction::Skip
            };
            HeartbeatDecision { action, tasks }
        } else {
            HeartbeatDecision {
                action: HeartbeatAction::Skip,
                tasks: String::new(),
            }
        }
    }

    pub async fn start(self: Arc<Self>) {
        if !self.enabled {
            info!("Heartbeat disabled");
            return;
        }
        let mut state = self.state.lock().await;
        if state.running {
            warn!("Heartbeat already running");
            return;
        }

        state.running = true;
        let interval = self.interval_s;
        let this = self.clone();
        let handle = tokio::spawn(async move {
            loop {
                tokio::time::sleep(tokio::time::Duration::from_secs(interval)).await;
                let running = this.state.lock().await.running;
                if !running {
                    break;
                }
                this.tick().await;
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
        let decision = self._decide(&content).await;

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
        if !self._is_deliverable(&response) {
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
            &self.model,
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
        let decision = self._decide(&content).await;
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
