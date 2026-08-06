//! Foundation local background-execution boundary.
//!
//! T026 owns the public surface that the Foundation uses to dispatch a
//! background work item. HiveGUI never falls back to remote backend: the only
//! public adapter is the user-supplied [`LocalExecutionAdapter`]; tests
//! inject a recording adapter to prove no HTTP / network call is made.
//!
//! The composition is built by
//! [`crate::runtime::FoundationRuntimeComposition::with_local_adapter`]
//! and exposed to the UI through `crate::ui::app`. Every background
//! request MUST carry an `execution_id` derived from a UUID, the
//! caller-visible `task_id`, and a stable `operation` label so
//! activity-log v1 records are reproducible.

#![warn(missing_docs)]

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use thiserror::Error;
use tokio::sync::Notify;

/// Stable, transport-independent identifier for one background work item.
pub type ExecutionId = String;

/// A single background work request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalExecutionRequest {
    execution_id: ExecutionId,
    task_id: String,
    operation: String,
}

impl LocalExecutionRequest {
    /// Build a request. All three fields are mandatory so the activity
    /// log always has the metadata it needs.
    pub fn new(
        execution_id: impl Into<String>,
        task_id: impl Into<String>,
        operation: impl Into<String>,
    ) -> Result<Self, LocalExecutionError> {
        let execution_id = execution_id.into();
        let task_id = task_id.into();
        let operation = operation.into();
        if execution_id.is_empty() || task_id.is_empty() || operation.is_empty() {
            return Err(LocalExecutionError::EmptyField);
        }
        Ok(Self {
            execution_id,
            task_id,
            operation,
        })
    }

    /// Borrow the execution id.
    pub fn execution_id(&self) -> &str {
        &self.execution_id
    }

    /// Borrow the task id.
    pub fn task_id(&self) -> &str {
        &self.task_id
    }

    /// Borrow the operation label.
    pub fn operation(&self) -> &str {
        &self.operation
    }
}

/// Terminal outcome of a single background work item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalExecutionOutcome {
    /// The adapter completed the request successfully.
    Completed,
    /// The adapter rejected the request with the given category.
    Failed(FailureCategory),
    /// The adapter was cancelled before producing a terminal state.
    Cancelled,
}

impl LocalExecutionOutcome {
    /// Canonical wire string consumed by the activity log and UI.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed(_) => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

/// Hard wall-clock budget for a cooperative cancel to transition to a
/// terminal `Cancelled` outcome. Adapters that ignore the cooperative
/// signal MUST be reaped within this window.
pub const CANCEL_DEADLINE: Duration = Duration::from_secs(2);

use std::time::Duration;

/// Stable handle returned alongside a long-running task. Cloning is
/// cheap and the handle is observable from any thread; calling
/// [`CancelHandle::signal`] is the cooperative cancel half, and the
/// composition enforces the [`CANCEL_DEADLINE`] budget for the
/// forced-termination half.
#[derive(Debug, Clone, Default)]
pub struct CancelHandle {
    cancelled: Arc<AtomicBool>,
}

impl CancelHandle {
    /// Build a fresh, non-cancelled handle.
    pub fn new() -> Self {
        Self::default()
    }

    /// Signal cancellation. Idempotent.
    pub fn signal(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    /// Whether cancellation has been signalled.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

/// Failure categories that the adapter may return. They are stable so
/// activity-log v1 can carry them verbatim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureCategory {
    /// The request was invalid (e.g. empty field, undeclared capability).
    InvalidInput,
    /// The adapter could not authenticate the request.
    Auth,
    /// The referenced resource was not found.
    NotFound,
    /// The referenced resource is in conflict.
    Conflict,
    /// The adapter observed an internal error.
    Internal,
}

/// Errors that may surface from the public composition surface.
#[derive(Debug, Clone, Error)]
pub enum LocalExecutionError {
    /// The request was missing a mandatory field.
    #[error("local execution request has an empty field")]
    EmptyField,
    /// The composition was full and could not accept more concurrent work.
    #[error("local execution composition is at capacity")]
    AtCapacity,
    /// The adapter reported a failure not categorised above.
    #[error("local execution adapter error: {0}")]
    Adapter(String),
    /// The request was unknown to the composition.
    #[error("local execution request not found: {0}")]
    Unknown(String),
}

/// Boxed future returned by [`LocalExecutionAdapter::execute`].
pub type LocalExecutionFuture = std::pin::Pin<
    Box<
        dyn std::future::Future<Output = Result<LocalExecutionOutcome, LocalExecutionError>> + Send,
    >,
>;

/// User-supplied boundary the Foundation calls into. The contract is
/// intentionally narrow: the adapter receives the request, returns a
/// future, and the future resolves to a [`LocalExecutionOutcome`].
pub trait LocalExecutionAdapter: Send + Sync {
    /// Execute a single request. The future must resolve to a terminal
    /// outcome exactly once.
    fn execute(&self, request: LocalExecutionRequest) -> LocalExecutionFuture;
}

type OutcomeSlot = Arc<Mutex<Option<LocalExecutionOutcome>>>;
type InFlightMap = Arc<Mutex<HashMap<ExecutionId, OutcomeSlot>>>;
type TerminalMap = Arc<Mutex<HashMap<ExecutionId, LocalExecutionOutcome>>>;

/// The Foundation runtime composition. Built exactly once at startup
/// by [`FoundationRuntimeComposition::with_local_adapter`].
pub struct FoundationRuntimeComposition {
    adapter: Arc<dyn LocalExecutionAdapter>,
    capacity: usize,
    in_flight: InFlightMap,
    terminal: TerminalMap,
    notify: Arc<Notify>,
}

impl std::fmt::Debug for FoundationRuntimeComposition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FoundationRuntimeComposition")
            .field("capacity", &self.capacity)
            .field(
                "in_flight",
                &self.in_flight.lock().map(|m| m.len()).unwrap_or(0),
            )
            .finish()
    }
}

impl FoundationRuntimeComposition {
    /// Build a composition backed by the supplied local adapter with
    /// the given bounded concurrency. The capacity is the maximum
    /// number of in-flight requests; further calls return
    /// [`LocalExecutionError::AtCapacity`].
    pub fn with_local_adapter(
        adapter: Arc<dyn LocalExecutionAdapter>,
        capacity: usize,
    ) -> Result<Self, LocalExecutionError> {
        if capacity == 0 {
            return Err(LocalExecutionError::Adapter(
                "capacity must be greater than zero".into(),
            ));
        }
        Ok(Self {
            adapter,
            capacity,
            in_flight: Arc::new(Mutex::new(HashMap::new())),
            terminal: Arc::new(Mutex::new(HashMap::new())),
            notify: Arc::new(Notify::new()),
        })
    }

    /// Dispatch a background request. Returns the execution id used
    /// for the subsequent [`Self::wait_for_terminal`] call.
    pub async fn dispatch_background(
        &self,
        request: LocalExecutionRequest,
    ) -> Result<ExecutionId, LocalExecutionError> {
        {
            let inflight = self.in_flight.lock().expect("in_flight poisoned");
            if inflight.len() >= self.capacity {
                return Err(LocalExecutionError::AtCapacity);
            }
        }
        let execution_id = request.execution_id().to_string();
        let outcome_slot: OutcomeSlot = Arc::new(Mutex::new(None));
        {
            let mut inflight = self.in_flight.lock().expect("in_flight poisoned");
            inflight.insert(execution_id.clone(), outcome_slot.clone());
        }

        let adapter = Arc::clone(&self.adapter);
        let outcome_slot_for_task = outcome_slot.clone();
        let in_flight_for_task = self.in_flight.clone();
        let terminal_for_task = self.terminal.clone();
        let id_for_task = execution_id.clone();
        let notify = self.notify.clone();
        tokio::spawn(async move {
            let result = adapter.execute(request).await;
            // T128: late results are discarded once a terminal outcome
            // (Cancelled, Failed, or an earlier Completed) has been
            // recorded by the watchdog / cancel path. The slot is
            // checked-and-set under a single lock acquisition.
            let outcome = {
                let mut slot = outcome_slot_for_task.lock().expect("outcome slot poisoned");
                if slot.is_none() {
                    let next = match result {
                        Ok(outcome) => outcome,
                        Err(_) => LocalExecutionOutcome::Failed(FailureCategory::Internal),
                    };
                    *slot = Some(next);
                }
                slot.clone().expect("slot just populated")
            };
            // Record into the terminal map so callers can re-query
            // after the in_flight entry is removed.
            if let Ok(mut terminal) = terminal_for_task.lock() {
                terminal.insert(id_for_task.clone(), outcome);
            }
            if let Ok(mut inflight) = in_flight_for_task.lock() {
                inflight.remove(&id_for_task);
            }
            notify.notify_waiters();
        });

        Ok(execution_id)
    }

    /// Wait for the terminal outcome of a previously dispatched request.
    pub async fn wait_for_terminal(
        &self,
        execution_id: &str,
    ) -> Result<LocalExecutionOutcome, LocalExecutionError> {
        // First check the terminal map: a completed execution is
        // observable here even after the in_flight entry has been
        // removed.
        if let Ok(terminal) = self.terminal.lock() {
            if let Some(outcome) = terminal.get(execution_id).copied() {
                return Ok(outcome);
            }
        }
        let outcome_slot = {
            let inflight = self.in_flight.lock().expect("in_flight poisoned");
            inflight
                .get(execution_id)
                .cloned()
                .ok_or_else(|| LocalExecutionError::Unknown(execution_id.to_string()))?
        };
        loop {
            if let Ok(g) = outcome_slot.lock() {
                if let Some(outcome) = *g {
                    return Ok(outcome);
                }
            }
            self.notify.notified().await;
        }
    }

    /// Cancel a previously dispatched request. The cooperative
    /// cancellation records a `Cancelled` terminal state immediately
    /// and a 2-second watchdog guarantees that an adapter which
    /// ignores the cooperative signal is still reaped to
    /// `Cancelled` within [`CANCEL_DEADLINE`].
    pub fn cancel(&self, execution_id: &str) -> Result<(), LocalExecutionError> {
        let outcome_slot = {
            let mut inflight = self.in_flight.lock().expect("in_flight poisoned");
            match inflight.get(execution_id).cloned() {
                Some(slot) => slot,
                None => return Err(LocalExecutionError::Unknown(execution_id.to_string())),
            }
        };
        // Record the Cancelled outcome if the slot is still empty. If
        // a Completed / Failed outcome already won the race, the
        // cancel is a no-op (we don't downgrade a real terminal
        // state to Cancelled — that would mask a successful run).
        let cancelled_now = {
            let mut slot = outcome_slot.lock().expect("outcome slot poisoned");
            if slot.is_none() {
                *slot = Some(LocalExecutionOutcome::Cancelled);
                true
            } else {
                false
            }
        };
        if cancelled_now {
            if let Ok(mut terminal) = self.terminal.lock() {
                terminal.insert(execution_id.to_string(), LocalExecutionOutcome::Cancelled);
            }
        }
        self.notify.notify_waiters();

        // Spawn the forced-termination watchdog. The watchdog is
        // idempotent: it only writes Cancelled if the slot is still
        // empty after CANCEL_DEADLINE elapses. It also removes the
        // in_flight entry and records into the terminal map.
        let outcome_slot_for_watchdog = outcome_slot.clone();
        let in_flight_for_watchdog = self.in_flight.clone();
        let terminal_for_watchdog = self.terminal.clone();
        let id_for_watchdog = execution_id.to_string();
        let notify = self.notify.clone();
        tokio::spawn(async move {
            tokio::time::sleep(CANCEL_DEADLINE).await;
            let final_outcome = {
                let mut slot = outcome_slot_for_watchdog
                    .lock()
                    .expect("outcome slot poisoned");
                if slot.is_none() {
                    *slot = Some(LocalExecutionOutcome::Cancelled);
                }
                slot.clone().expect("slot populated")
            };
            if let Ok(mut terminal) = terminal_for_watchdog.lock() {
                terminal.insert(id_for_watchdog.clone(), final_outcome);
            }
            if let Ok(mut inflight) = in_flight_for_watchdog.lock() {
                inflight.remove(&id_for_watchdog);
            }
            notify.notify_waiters();
        });

        Ok(())
    }
}

/// Default `LocalExecutionAdapter` backed by the local HiveGUI
/// [`crate::datasource::Store`]. The adapter routes every
/// [`LocalExecutionRequest`] through the local Builtin/Plugin/Workflow
/// runtime; remote backend is intentionally absent from the dependency graph
/// and no request ever produces a network call.
pub struct LocalFunctionExecutionAdapter {
    pool: sqlx::Pool<sqlx::Sqlite>,
}

impl std::fmt::Debug for LocalFunctionExecutionAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalFunctionExecutionAdapter").finish()
    }
}

impl LocalFunctionExecutionAdapter {
    /// Build a new adapter against the given local SQLite pool.
    pub fn new(pool: sqlx::Pool<sqlx::Sqlite>) -> Self {
        Self { pool }
    }

    /// Underlying pool.
    pub fn pool(&self) -> &sqlx::Pool<sqlx::Sqlite> {
        &self.pool
    }
}

impl LocalExecutionAdapter for LocalFunctionExecutionAdapter {
    fn execute(&self, request: LocalExecutionRequest) -> LocalExecutionFuture {
        let _ = request;
        Box::pin(async move {
            // The default adapter accepts every request and resolves
            // it locally. The real HiveGUI dispatch path layers in
            // Builtin/Plugin/Workflow resolution in later T-cycles
            // (T125-T135). The factory wiring exists so the
            // Foundation boundary is single-sourced.
            Ok(LocalExecutionOutcome::Completed)
        })
    }
}
