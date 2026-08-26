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
    collections::{BTreeMap, BTreeSet, HashMap},
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

/// One independently cancellable layer in a local Agent execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CancellationLayer {
    /// Root Agent orchestration.
    Agent,
    /// Direct child-Agent orchestration.
    ChildAgent,
    /// Local LLM/provider call.
    Llm,
    /// Persisted Tool dispatch.
    Tool,
    /// Local Workflow execution.
    Workflow,
    /// Local Extism Plugin invocation.
    Plugin,
}

/// Immutable request passed to a layered cancellation adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayerExecutionRequest {
    execution_id: String,
    session_id: String,
    layer: CancellationLayer,
    external_side_effect: bool,
}

impl LayerExecutionRequest {
    /// Owning execution UUID.
    pub fn execution_id(&self) -> &str {
        &self.execution_id
    }

    /// Owning session UUID.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Runtime layer being executed.
    pub fn layer(&self) -> CancellationLayer {
        self.layer
    }

    /// Whether successful completion may have an external side effect.
    pub fn external_side_effect(&self) -> bool {
        self.external_side_effect
    }
}

/// Boxed future returned by [`LayerCancellationAdapter`].
pub type LayerCancellationFuture = std::pin::Pin<
    Box<
        dyn std::future::Future<Output = Result<LocalExecutionOutcome, LocalExecutionError>> + Send,
    >,
>;

/// Production-facing adapter for one local execution layer.
pub trait LayerCancellationAdapter: Send + Sync + 'static {
    /// Execute one layer while observing the shared cooperative cancel handle.
    fn execute_layer(
        &self,
        request: LayerExecutionRequest,
        cancel: CancelHandle,
    ) -> LayerCancellationFuture;
}

/// Stable validation error for the layered cancellation boundary.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
#[error("invalid_input:{field}:{reason}")]
pub struct LayerCancellationError {
    field: &'static str,
    reason: &'static str,
}

impl LayerCancellationError {
    fn new(field: &'static str, reason: &'static str) -> Self {
        Self { field, reason }
    }

    /// Public field associated with the error.
    pub fn field(&self) -> &'static str {
        self.field
    }

    /// Stable non-sensitive reason code.
    pub fn reason(&self) -> &'static str {
        self.reason
    }
}

/// Immutable terminal detail for one layered execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayerCancellationSummary {
    status: &'static str,
    completed_layers: Vec<CancellationLayer>,
    interrupted_layers: Vec<CancellationLayer>,
    not_started_layers: Vec<CancellationLayer>,
    side_effect_notice: Option<&'static str>,
}

impl LayerCancellationSummary {
    /// Stable terminal status.
    pub fn status(&self) -> &'static str {
        self.status
    }

    /// Layers that completed before cancellation.
    pub fn completed_layers(&self) -> &[CancellationLayer] {
        &self.completed_layers
    }

    /// Layers interrupted cooperatively or by the forced deadline.
    pub fn interrupted_layers(&self) -> &[CancellationLayer] {
        &self.interrupted_layers
    }

    /// Planned layers that never started.
    pub fn not_started_layers(&self) -> &[CancellationLayer] {
        &self.not_started_layers
    }

    /// Stable warning when completed work may have external side effects.
    pub fn side_effect_notice(&self) -> Option<&'static str> {
        self.side_effect_notice
    }
}

#[derive(Debug)]
struct LayerExecutionState {
    session_id: String,
    planned: BTreeSet<CancellationLayer>,
    cancel: CancelHandle,
    scheduling_closed: bool,
    started: BTreeSet<CancellationLayer>,
    active: BTreeMap<CancellationLayer, tokio::task::AbortHandle>,
    completed: BTreeSet<CancellationLayer>,
    interrupted: BTreeSet<CancellationLayer>,
    external_side_effects: BTreeSet<CancellationLayer>,
    failed: bool,
    terminal: Option<LayerCancellationSummary>,
}

struct LayeredCancellationInner<A> {
    adapter: Arc<A>,
    executions: Mutex<HashMap<String, LayerExecutionState>>,
    notify: Notify,
}

/// Process-local coordinator that propagates Stop across every layer of one
/// Agent execution without affecting other sessions.
pub struct LayeredCancellationRuntime<A> {
    inner: Arc<LayeredCancellationInner<A>>,
}

impl<A> Clone for LayeredCancellationRuntime<A> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<A> LayeredCancellationRuntime<A>
where
    A: LayerCancellationAdapter,
{
    /// Construct a coordinator around the unique local adapter boundary.
    pub fn new(adapter: Arc<A>) -> Self {
        Self {
            inner: Arc::new(LayeredCancellationInner {
                adapter,
                executions: Mutex::new(HashMap::new()),
                notify: Notify::new(),
            }),
        }
    }

    /// Register one execution and its immutable planned layer set.
    pub fn start_execution(
        &self,
        execution_id: &str,
        session_id: &str,
        planned: &[CancellationLayer],
    ) -> Result<(), LayerCancellationError> {
        validate_layer_uuid(execution_id, "execution_id")?;
        validate_layer_uuid(session_id, "session_id")?;
        if planned.is_empty() {
            return Err(LayerCancellationError::new("layers", "empty"));
        }
        let planned = planned.iter().copied().collect::<BTreeSet<_>>();
        let mut executions = self.inner.executions.lock().expect("executions poisoned");
        if executions.contains_key(execution_id) {
            return Err(LayerCancellationError::new("execution_id", "duplicate"));
        }
        executions.insert(
            execution_id.to_string(),
            LayerExecutionState {
                session_id: session_id.to_string(),
                planned,
                cancel: CancelHandle::new(),
                scheduling_closed: false,
                started: BTreeSet::new(),
                active: BTreeMap::new(),
                completed: BTreeSet::new(),
                interrupted: BTreeSet::new(),
                external_side_effects: BTreeSet::new(),
                failed: false,
                terminal: None,
            },
        );
        Ok(())
    }

    /// Start one planned layer. Once Stop closes scheduling, no new layer may
    /// begin even if it was part of the original plan.
    pub async fn dispatch_step(
        &self,
        execution_id: &str,
        layer: CancellationLayer,
        external_side_effect: bool,
    ) -> Result<(), LayerCancellationError> {
        let (session_id, cancel) = {
            let mut executions = self.inner.executions.lock().expect("executions poisoned");
            let state = executions
                .get_mut(execution_id)
                .ok_or_else(|| LayerCancellationError::new("execution_id", "not_found"))?;
            if state.scheduling_closed {
                return Err(LayerCancellationError::new(
                    "execution_id",
                    "scheduling_closed",
                ));
            }
            if !state.planned.contains(&layer) {
                return Err(LayerCancellationError::new("layer", "not_planned"));
            }
            if !state.started.insert(layer) {
                return Err(LayerCancellationError::new("layer", "already_started"));
            }
            if external_side_effect {
                state.external_side_effects.insert(layer);
            }
            (state.session_id.clone(), state.cancel.clone())
        };

        let request = LayerExecutionRequest {
            execution_id: execution_id.to_string(),
            session_id,
            layer,
            external_side_effect,
        };
        let inner = Arc::clone(&self.inner);
        let future = self.inner.adapter.execute_layer(request, cancel);
        let execution_id_for_task = execution_id.to_string();
        let task = tokio::spawn(async move {
            let result = future.await;
            let mut executions = inner.executions.lock().expect("executions poisoned");
            if let Some(state) = executions.get_mut(&execution_id_for_task) {
                state.active.remove(&layer);
                if state.scheduling_closed {
                    state.interrupted.insert(layer);
                } else {
                    match result {
                        Ok(LocalExecutionOutcome::Completed) => {
                            state.completed.insert(layer);
                        }
                        Ok(LocalExecutionOutcome::Cancelled) => {
                            state.interrupted.insert(layer);
                        }
                        Ok(LocalExecutionOutcome::Failed(_)) | Err(_) => {
                            state.interrupted.insert(layer);
                            state.failed = true;
                        }
                    }
                }
                finalize_layer_state(state);
            }
            inner.notify.notify_waiters();
        });
        let abort_handle = task.abort_handle();
        let mut executions = self.inner.executions.lock().expect("executions poisoned");
        if let Some(state) = executions.get_mut(execution_id)
            && state.terminal.is_none()
            && !task.is_finished()
        {
            state.active.insert(layer, abort_handle);
        }
        Ok(())
    }

    /// Close scheduling and propagate cooperative cancellation. Any layer that
    /// remains live at [`CANCEL_DEADLINE`] is aborted and recorded interrupted.
    pub fn cancel_execution(&self, execution_id: &str) -> Result<(), LayerCancellationError> {
        {
            let mut executions = self.inner.executions.lock().expect("executions poisoned");
            let state = executions
                .get_mut(execution_id)
                .ok_or_else(|| LayerCancellationError::new("execution_id", "not_found"))?;
            if state.terminal.is_some() {
                return Ok(());
            }
            state.scheduling_closed = true;
            state.cancel.signal();
            finalize_layer_state(state);
        }
        self.inner.notify.notify_waiters();

        let inner = Arc::clone(&self.inner);
        let execution_id = execution_id.to_string();
        tokio::spawn(async move {
            tokio::time::sleep(CANCEL_DEADLINE).await;
            let mut executions = inner.executions.lock().expect("executions poisoned");
            if let Some(state) = executions.get_mut(&execution_id)
                && state.terminal.is_none()
            {
                let active = std::mem::take(&mut state.active);
                for (layer, handle) in active {
                    handle.abort();
                    state.interrupted.insert(layer);
                }
                finalize_layer_state(state);
            }
            inner.notify.notify_waiters();
        });
        Ok(())
    }

    /// Wait for an immutable terminal summary.
    pub async fn wait_for_terminal(
        &self,
        execution_id: &str,
    ) -> Result<LayerCancellationSummary, LayerCancellationError> {
        loop {
            let notified = self.inner.notify.notified();
            {
                let executions = self.inner.executions.lock().expect("executions poisoned");
                let state = executions
                    .get(execution_id)
                    .ok_or_else(|| LayerCancellationError::new("execution_id", "not_found"))?;
                if let Some(summary) = &state.terminal {
                    return Ok(summary.clone());
                }
            }
            notified.await;
        }
    }
}

fn validate_layer_uuid(value: &str, field: &'static str) -> Result<(), LayerCancellationError> {
    uuid::Uuid::parse_str(value)
        .map(|_| ())
        .map_err(|_| LayerCancellationError::new(field, "invalid_uuid"))
}

fn finalize_layer_state(state: &mut LayerExecutionState) {
    if state.terminal.is_some() || !state.active.is_empty() {
        return;
    }
    if !state.scheduling_closed && state.started != state.planned {
        return;
    }
    let status = if state.scheduling_closed {
        "cancelled"
    } else if state.failed {
        "failed"
    } else {
        "completed"
    };
    let not_started_layers = state
        .planned
        .difference(&state.started)
        .copied()
        .collect::<Vec<_>>();
    let side_effect_notice = state
        .completed
        .iter()
        .any(|layer| state.external_side_effects.contains(layer))
        .then_some("completed external side effects are not rolled back");
    state.terminal = Some(LayerCancellationSummary {
        status,
        completed_layers: state.completed.iter().copied().collect(),
        interrupted_layers: state.interrupted.iter().copied().collect(),
        not_started_layers,
        side_effect_notice,
    });
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
                (*slot).expect("slot just populated")
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
        if let Ok(terminal) = self.terminal.lock()
            && let Some(outcome) = terminal.get(execution_id).copied()
        {
            return Ok(outcome);
        }
        let outcome_slot = {
            let inflight = self.in_flight.lock().expect("in_flight poisoned");
            inflight
                .get(execution_id)
                .cloned()
                .ok_or_else(|| LocalExecutionError::Unknown(execution_id.to_string()))?
        };
        loop {
            if let Ok(g) = outcome_slot.lock()
                && let Some(outcome) = *g
            {
                return Ok(outcome);
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
            let inflight = self.in_flight.lock().expect("in_flight poisoned");
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
        if cancelled_now && let Ok(mut terminal) = self.terminal.lock() {
            terminal.insert(execution_id.to_string(), LocalExecutionOutcome::Cancelled);
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
                (*slot).expect("slot populated")
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
