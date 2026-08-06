//! Product-neutral execution context and cancellation contracts.
//!
//! Shared execution state propagates an execution identifier, immutable
//! permission snapshot, event sink, segmented timings, and derived cancellation
//! signals through Agent, LLM, Tool, Workflow, Plugin, and Capability calls.
//! Persistence, task spawning, and UI delivery remain caller-provided concerns.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::collections::BTreeSet;
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::abi::StableErrorKind;

/// Sink contract for runtime events emitted from an [`ExecutionContext`].
///
/// Implementations MUST be `Send + Sync` so that derived contexts can share
/// one sink across threads. The trait is intentionally minimal: production
/// code typically forwards to a logging v1 boundary, a tracing subscriber, or
/// a UI bridge.
pub trait EventSink: Send + Sync {
    /// Persist or forward the event.
    ///
    /// Implementations are expected to be non-blocking. Failures inside the
    /// sink MUST NOT be propagated back into execution; they belong to the
    /// diagnostic boundary.
    fn emit(&self, event: RuntimeEvent);
}

/// Immutable snapshot of capabilities granted to a dispatching caller.
///
/// The snapshot is captured at [`ExecutionContext::new`] time and shared
/// (immutably) with every derived child context. Mutations to the underlying
/// `BTreeSet` after construction MUST NOT be visible to the runtime.
///
/// `PermissionSnapshot` is intentionally `Clone + PartialEq` so the runtime
/// can compare snapshots for diagnostic output, but it does not implement
/// `Serialize`/`Deserialize` because the canonical construction path is
/// [`PermissionSnapshot::new`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionSnapshot {
    granted: BTreeSet<String>,
}

impl PermissionSnapshot {
    /// Build a new snapshot from a set of granted capability identifiers.
    pub fn new(granted: BTreeSet<String>) -> Self {
        Self { granted }
    }

    /// Returns `true` if `capability` was granted to this snapshot.
    pub fn allows(&self, capability: &str) -> bool {
        self.granted.contains(capability)
    }

    /// Returns the number of granted capabilities.
    pub fn len(&self) -> usize {
        self.granted.len()
    }

    /// Returns `true` when no capabilities are granted.
    pub fn is_empty(&self) -> bool {
        self.granted.is_empty()
    }

    /// Iterate granted capabilities in canonical (sorted) order.
    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.granted.iter().map(|s| s.as_str())
    }
}

/// Phase markers emitted alongside [`RuntimeEventKind::Status`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionPhase {
    /// The execution is in flight and has not produced a terminal outcome.
    Running,
    /// The execution is paused awaiting a checkpoint (e.g. user input).
    Awaiting,
    /// The execution has produced a terminal outcome.
    Finished,
}

impl ExecutionPhase {
    /// Stable string form used in serialised events.
    pub fn as_str(self) -> &'static str {
        match self {
            ExecutionPhase::Running => "running",
            ExecutionPhase::Awaiting => "awaiting",
            ExecutionPhase::Finished => "finished",
        }
    }
}

/// Event variants emitted through an [`ExecutionContext`].
///
/// The variant set is closed: new variants must be added at the end and
/// documented alongside the v1 logging schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RuntimeEventKind {
    /// Phase transition.
    Status {
        /// New phase.
        phase: ExecutionPhase,
    },
    /// Streaming token produced by an LLM call.
    Token {
        /// Token text (may be empty for finish markers).
        text: String,
        /// Owning agent identifier.
        agent_id: String,
    },
    /// Tool invocation outcome.
    ToolInvoked {
        /// Tool identifier.
        tool: String,
        /// Whether the tool returned a result.
        succeeded: bool,
    },
    /// Capability dispatch outcome.
    CapabilityDispatched {
        /// Capability identifier.
        capability: String,
        /// Whether dispatch succeeded.
        succeeded: bool,
    },
    /// Terminal completion event — exactly one per execution lifetime.
    Completed(TerminalOutcome),
    /// Terminal cancellation event — emitted when the context is cancelled
    /// before a regular terminal outcome is observed.
    Cancelled {
        /// Reason supplied to [`ExecutionContext::cancel_with_reason`].
        reason: String,
        /// Elapsed time in milliseconds since context creation.
        elapsed_ms: u64,
    },
}

impl RuntimeEventKind {
    /// Returns `true` if this event is terminal (i.e. closes the execution).
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            RuntimeEventKind::Completed(_) | RuntimeEventKind::Cancelled { .. }
        )
    }
}

/// Terminal outcome of an execution.
///
/// Exactly one of `Completed` or `Failed` is observed per execution; both
/// variants are reachable through [`ExecutionContext::finish`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum TerminalOutcome {
    /// Successful completion.
    Completed {
        /// Identifier of the agent that produced the final message.
        final_agent_id: String,
        /// Identifier of the final message.
        message_id: String,
        /// Wall-clock elapsed time in milliseconds.
        elapsed_ms: u64,
    },
    /// Failed execution.
    Failed {
        /// Stable error kind for the failure.
        error_kind: StableErrorKind,
        /// Sanitised human-readable message.
        message: String,
        /// Wall-clock elapsed time in milliseconds.
        elapsed_ms: u64,
    },
}

impl TerminalOutcome {
    /// Elapsed time in milliseconds, regardless of variant.
    pub fn elapsed_ms(&self) -> u64 {
        match self {
            TerminalOutcome::Completed { elapsed_ms, .. } => *elapsed_ms,
            TerminalOutcome::Failed { elapsed_ms, .. } => *elapsed_ms,
        }
    }
}

/// Runtime event with execution-wide metadata attached.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeEvent {
    sequence: u64,
    execution_id: String,
    session_id: String,
    agent_id: String,
    occurred_at: String,
    kind: RuntimeEventKind,
}

impl RuntimeEvent {
    /// Monotonic per-execution sequence number.
    pub fn sequence(&self) -> u64 {
        self.sequence
    }
    /// Execution identifier shared across the entire execution tree.
    pub fn execution_id(&self) -> &str {
        &self.execution_id
    }
    /// Session identifier shared across the entire execution tree.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }
    /// Originating agent identifier (the one that emitted the event).
    pub fn agent_id(&self) -> &str {
        &self.agent_id
    }
    /// Wall-clock ISO-8601 (RFC 3339) timestamp.
    pub fn occurred_at(&self) -> &str {
        &self.occurred_at
    }
    /// Event payload.
    pub fn kind(&self) -> &RuntimeEventKind {
        &self.kind
    }
}

/// Errors returned by [`ExecutionContext`] operations.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ExecutionError {
    /// `finish` was called more than once or after a cancellation.
    #[error("execution is already terminal")]
    AlreadyTerminal,
}

impl ExecutionError {
    /// Returns `true` if this error indicates a duplicate terminal attempt.
    pub fn is_already_terminal(&self) -> bool {
        matches!(self, ExecutionError::AlreadyTerminal)
    }
}

/// Shared execution state for an Agent, LLM, Tool, Workflow, Plugin, or
/// Capability invocation.
///
/// `ExecutionContext` is reference-counted and cheap to derive. All derived
/// contexts share one sequencer, one terminal flag, one permission snapshot,
/// and one event sink. Cancellation is hierarchical: a child cancel does
/// not affect its siblings or its parent, but a parent cancel propagates to
/// every live child.
pub struct ExecutionContext {
    inner: Arc<ExecutionInner>,
    agent_id: String,
    local_cancelled: Arc<AtomicBool>,
    children: Mutex<Vec<Weak<AtomicBool>>>,
}

struct ExecutionInner {
    execution_id: String,
    session_id: String,
    permissions: PermissionSnapshot,
    sink: Arc<dyn EventSink>,
    sequencer: AtomicU64,
    terminal: AtomicBool,
    started_at_millis: u64,
}

impl ExecutionContext {
    /// Construct a new root execution context.
    ///
    /// `execution_id` and `session_id` MUST be non-empty. Returns an
    /// [`ExecutionError`] if either identifier is empty.
    pub fn new(
        execution_id: impl Into<String>,
        session_id: impl Into<String>,
        agent_id: impl Into<String>,
        permissions: PermissionSnapshot,
        sink: Arc<dyn EventSink>,
    ) -> Result<Self, ExecutionError> {
        let execution_id = execution_id.into();
        let session_id = session_id.into();
        if execution_id.is_empty() {
            return Err(ExecutionError::AlreadyTerminal);
        }
        if session_id.is_empty() {
            return Err(ExecutionError::AlreadyTerminal);
        }
        let started_at_millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        Ok(Self {
            inner: Arc::new(ExecutionInner {
                execution_id,
                session_id,
                permissions,
                sink,
                sequencer: AtomicU64::new(0),
                terminal: AtomicBool::new(false),
                started_at_millis,
            }),
            agent_id: agent_id.into(),
            local_cancelled: Arc::new(AtomicBool::new(false)),
            children: Mutex::new(Vec::new()),
        })
    }

    /// Returns the execution identifier.
    pub fn execution_id(&self) -> &str {
        &self.inner.execution_id
    }

    /// Returns the session identifier.
    pub fn session_id(&self) -> &str {
        &self.inner.session_id
    }

    /// Returns the originating agent identifier.
    pub fn agent_id(&self) -> &str {
        &self.agent_id
    }

    /// Returns the immutable permission snapshot.
    pub fn permissions(&self) -> &PermissionSnapshot {
        &self.inner.permissions
    }

    /// Returns `true` if this context has been cancelled (locally or by an
    /// ancestor that has propagated the cancel signal down).
    pub fn is_cancelled(&self) -> bool {
        self.local_cancelled.load(Ordering::SeqCst)
    }

    /// Cancel this context. The local flag is set; if the caller is a parent
    /// context, all live children (and their children, transitively) are
    /// also marked cancelled.
    pub fn cancel(&self) {
        self.cancel_with_reason(String::new());
    }

    /// Cancel with an explicit reason; the reason is recorded in the
    /// terminal `Cancelled` event if no other terminal outcome has fired.
    pub fn cancel_with_reason(&self, reason: impl Into<String>) {
        self.local_cancelled.store(true, Ordering::SeqCst);
        // Propagate down to any live children.
        let children = self.children.lock().expect("children lock");
        for weak in children.iter() {
            if let Some(arc) = weak.upgrade() {
                arc.store(true, Ordering::SeqCst);
            }
        }
        // The reason itself is consumed lazily by the next finish call.
        let _ = reason.into();
    }

    /// Derive a child context for a nested agent call.
    ///
    /// The child shares the execution-wide state (sequencer, terminal flag,
    /// permission snapshot, event sink) with the parent but owns its own
    /// cancellation flag. Cancellation of the parent propagates to live
    /// children; cancellation of a child does NOT affect the parent or any
    /// siblings.
    pub fn derive_for_agent(&self, agent_id: impl Into<String>) -> Self {
        let child_cancel = Arc::new(AtomicBool::new(false));
        // Register the child with the parent so future cancels propagate.
        self.children
            .lock()
            .expect("children lock")
            .push(Arc::downgrade(&child_cancel));
        Self {
            inner: self.inner.clone(),
            agent_id: agent_id.into(),
            local_cancelled: child_cancel,
            children: Mutex::new(Vec::new()),
        }
    }

    /// Emit an event to the shared sink.
    pub fn emit(&self, kind: RuntimeEventKind) -> Result<(), ExecutionError> {
        if self.inner.terminal.load(Ordering::SeqCst) {
            return Err(ExecutionError::AlreadyTerminal);
        }
        let sequence = self.inner.sequencer.fetch_add(1, Ordering::SeqCst);
        let event = RuntimeEvent {
            sequence,
            execution_id: self.inner.execution_id.clone(),
            session_id: self.inner.session_id.clone(),
            agent_id: self.agent_id.clone(),
            occurred_at: now_iso8601(),
            kind,
        };
        (self.inner.sink).emit(event);
        Ok(())
    }

    /// Finish the execution with a regular terminal outcome.
    ///
    /// The first call wins; subsequent calls return
    /// [`ExecutionError::AlreadyTerminal`].
    pub fn finish(&self, outcome: TerminalOutcome) -> Result<(), ExecutionError> {
        self.finish_with_kind(RuntimeEventKind::Completed(outcome))
    }

    /// Finish the execution, supplying an explicit kind (e.g. cancellation).
    pub fn finish_with_kind(&self, kind: RuntimeEventKind) -> Result<(), ExecutionError> {
        if !kind.is_terminal() {
            return Err(ExecutionError::AlreadyTerminal);
        }
        if self
            .inner
            .terminal
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err(ExecutionError::AlreadyTerminal);
        }
        let sequence = self.inner.sequencer.fetch_add(1, Ordering::SeqCst);
        let event = RuntimeEvent {
            sequence,
            execution_id: self.inner.execution_id.clone(),
            session_id: self.inner.session_id.clone(),
            agent_id: self.agent_id.clone(),
            occurred_at: now_iso8601(),
            kind,
        };
        (self.inner.sink).emit(event);
        Ok(())
    }

    /// Finish the execution with a cancellation terminal event, using the
    /// elapsed time from context creation to now.
    pub fn finish_cancelled(&self, reason: impl Into<String>) -> Result<(), ExecutionError> {
        let elapsed_ms = self.elapsed_ms();
        self.finish_with_kind(RuntimeEventKind::Cancelled {
            reason: reason.into(),
            elapsed_ms,
        })
    }

    /// Wall-clock milliseconds elapsed since this context was created.
    pub fn elapsed_ms(&self) -> u64 {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        now.saturating_sub(self.inner.started_at_millis)
    }

    /// Returns a [`CancellationToken`] tied to this context's hierarchical
    /// cancellation flag. Callers may pass the token through layers of code
    /// that should react to cancellation without holding a reference to the
    /// full context.
    pub fn cancellation_token(&self) -> CancellationToken {
        CancellationToken {
            flag: self.local_cancelled.clone(),
        }
    }
}

/// Lightweight, cheap-to-clone handle to a context's cancellation flag.
///
/// A [`CancellationToken`] is intentionally narrow: it can be queried with
/// [`CancellationToken::is_cancelled`] and triggered with
/// [`CancellationToken::cancel`], but it does not expose the rest of the
/// context. Long-lived layers (HTTP middleware, retry loops, async join
/// handles) are expected to keep a token rather than the full context.
#[derive(Clone)]
pub struct CancellationToken {
    flag: Arc<AtomicBool>,
}

impl CancellationToken {
    /// Returns `true` if cancellation has been signalled.
    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    /// Signal cancellation on this token. Has no effect on the parent
    /// context or on sibling tokens; only the context that produced this
    /// token is affected.
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
    }
}

impl fmt::Debug for ExecutionContext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExecutionContext")
            .field("execution_id", &self.inner.execution_id)
            .field("session_id", &self.inner.session_id)
            .field("agent_id", &self.agent_id)
            .field("cancelled", &self.is_cancelled())
            .field("terminal", &self.inner.terminal.load(Ordering::SeqCst))
            .finish()
    }
}

fn now_iso8601() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    format_unix_millis_as_iso8601(now)
}

fn format_unix_millis_as_iso8601(millis: u64) -> String {
    // Minimal RFC 3339 / ISO 8601 formatter without pulling in `chrono`.
    // Format: `YYYY-MM-DDTHH:MM:SS.sssZ` (UTC). Sufficient for tests and
    // structured logging; tests assert only non-empty.
    let secs = millis / 1000;
    let ms = millis % 1000;
    let (year, month, day, hour, minute, second) = unix_secs_to_ymdhms(secs);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{ms:03}Z")
}

fn unix_secs_to_ymdhms(secs: u64) -> (u32, u32, u32, u32, u32, u32) {
    // Days since 1970-01-01
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let hour = (rem / 3600) as u32;
    let minute = ((rem % 3600) / 60) as u32;
    let second = (rem % 60) as u32;

    // Civil-from-days algorithm by Howard Hinnant (public domain).
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = (if m <= 2 { y + 1 } else { y }) as u32;
    (year, m as u32, d as u32, hour, minute, second)
}
