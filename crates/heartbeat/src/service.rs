//! Periodic heartbeat service (port of `nanobot.heartbeat.service`).
//!
//! The Python version talks directly to an `LLMProvider`. In Rust we keep
//! the same two-phase design (Phase 1 "decide", Phase 2 "execute") but
//! abstract both over traits so this crate is provider-agnostic.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use log::{info, warn};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use utils::evaluator::NotificationEvaluator;

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

/// Plug-in that inspects `HEARTBEAT.md` content and returns a decision.
#[async_trait]
pub trait HeartbeatDecider: Send + Sync {
    async fn decide(&self, content: &str, timezone: Option<&str>) -> HeartbeatDecision;
}

/// Plug-in that executes a task and returns the resulting text.
#[async_trait]
pub trait HeartbeatExecutor: Send + Sync {
    /// Execute `tasks`, returning the response body to forward to the user
    /// (or `None` if nothing to report).
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

/// Builder-style configuration for [`HeartbeatService`].
pub struct HeartbeatConfig {
    pub workspace: PathBuf,
    pub interval_s: u64,
    pub enabled: bool,
    pub timezone: Option<String>,
    pub model: String,
}

impl HeartbeatService {
    pub fn new(
        cfg: HeartbeatConfig,
        decider: Arc<dyn HeartbeatDecider>,
        executor: Option<Arc<dyn HeartbeatExecutor>>,
        notifier: Option<Arc<dyn HeartbeatNotifier>>,
        evaluator: Option<Arc<dyn NotificationEvaluator>>,
    ) -> Self {
        Self {
            workspace: cfg.workspace,
            interval_s: cfg.interval_s,
            enabled: cfg.enabled,
            timezone: cfg.timezone,
            model: cfg.model,
            decider,
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

    /// Start the heartbeat service.
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

    /// Stop the heartbeat service.
    pub async fn stop(&self) {
        let mut state = self.state.lock().await;
        state.running = false;
        if let Some(handle) = state.task.take() {
            handle.abort();
        }
    }

    async fn tick(&self) {
        let Some(content) = self.read_heartbeat_file() else {
            log::debug!("Heartbeat: HEARTBEAT.md missing or empty");
            return;
        };
        if content.is_empty() {
            log::debug!("Heartbeat: HEARTBEAT.md empty");
            return;
        }

        info!("Heartbeat: checking for tasks...");
        let decision = self
            .decider
            .decide(&content, self.timezone.as_deref())
            .await;
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

    /// Manually trigger a heartbeat.
    pub async fn trigger_now(&self) -> Option<String> {
        let content = self.read_heartbeat_file()?;
        if content.is_empty() {
            return None;
        }
        let decision = self.decider.decide(&content, self.timezone.as_deref()).await;
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

    struct AlwaysSkip;
    #[async_trait]
    impl HeartbeatDecider for AlwaysSkip {
        async fn decide(&self, _content: &str, _tz: Option<&str>) -> HeartbeatDecision {
            HeartbeatDecision {
                action: HeartbeatAction::Skip,
                tasks: String::new(),
            }
        }
    }

    #[tokio::test]
    async fn trigger_now_returns_none_when_skip() {
        let dir = std::env::temp_dir().join(format!("hb-{}", rand_suffix()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("HEARTBEAT.md"), "x").unwrap();
        let svc = HeartbeatService::new(
            HeartbeatConfig {
                workspace: dir,
                interval_s: 60,
                enabled: true,
                timezone: None,
                model: "test".into(),
            },
            Arc::new(AlwaysSkip),
            None,
            None,
            None,
        );
        assert!(svc.trigger_now().await.is_none());
    }

    fn rand_suffix() -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
            .to_string()
    }
}
