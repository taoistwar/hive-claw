//! T128 [US13] Cooperative cancellation contract.
//!
//! Source of truth: `specs/011-hivegui-standalone-mode/tasks.md` §T128
//! ("在 `crates/hivegui/src/runtime/execution.rs`、
//! `crates/hivegui/src/runtime/local_agent.rs` 和
//! `crates/hivegui/src/runtime/plugin_executor.rs` 实现协作取消、
//! 迟到结果丢弃、2秒 CancelHandle 强停和 cancelled 终态明细").
//!
//! Red public boundaries the T128 implementation must satisfy:
//!   - `hivegui::runtime::execution::FoundationRuntimeComposition::dispatch_background`
//!     surfaces a `LocalExecutionOutcome::Cancelled` terminal state when
//!     a `CancelHandle` is signalled.
//!   - `hivegui::runtime::execution::CancelHandle` enforces a 2-second
//!     forced-termination deadline — adapters that ignore the cooperative
//!     signal MUST be reaped to `Cancelled` within 2 s + ε.
//!   - Late results that arrive after a `Cancelled` outcome has been
//!     recorded are discarded: the terminal state MUST NOT flip back to
//!     `Completed` / `Failed` once cancellation has been recorded.
//!   - `cancelled` terminal state records a stable wire string
//!     (`"cancelled"`) for the diagnostics sink and UI summary rows.
//!   - `LocalAgentRuntime::cancel_execution` propagates the cancel to a
//!     long-running tool call and the per-turn snapshot updates.

#![allow(missing_docs)]

mod support;

use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use hivegui::runtime::execution::{
    FoundationRuntimeComposition, LocalExecutionAdapter, LocalExecutionError, LocalExecutionFuture,
    LocalExecutionOutcome, LocalExecutionRequest,
};
use support::TestWorkspace;

/// Adapter that resolves after the supplied delay, ignoring cooperative
/// cancellation. The forced-termination path must reap it.
#[derive(Debug)]
struct LongRunningAdapter {
    delay: Duration,
    resolved: Arc<Mutex<u32>>,
}

impl LongRunningAdapter {
    fn new(delay: Duration) -> Self {
        Self {
            delay,
            resolved: Arc::new(Mutex::new(0)),
        }
    }
}

impl LocalExecutionAdapter for LongRunningAdapter {
    fn execute(&self, _request: LocalExecutionRequest) -> LocalExecutionFuture {
        let delay = self.delay;
        let resolved = self.resolved.clone();
        Box::pin(async move {
            tokio::time::sleep(delay).await;
            *resolved.lock().expect("resolved poisoned") += 1;
            Ok(LocalExecutionOutcome::Completed)
        })
    }
}

/// Adapter that always returns `Completed` after a short delay — used to
/// prove that late results are discarded once cancellation is observed.
#[derive(Debug)]
struct LateCompletingAdapter;

impl LocalExecutionAdapter for LateCompletingAdapter {
    fn execute(&self, _request: LocalExecutionRequest) -> LocalExecutionFuture {
        Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            Ok(LocalExecutionOutcome::Completed)
        })
    }
}

fn request(id: &str) -> LocalExecutionRequest {
    LocalExecutionRequest::new(id, "task-1", "long-running")
        .expect("request must validate non-empty fields")
}

#[tokio::test(flavor = "current_thread")]
async fn cooperative_cancel_records_cancelled_outcome_under_two_seconds() {
    let _workspace = TestWorkspace::new().expect("test workspace");
    let adapter: Arc<dyn LocalExecutionAdapter> =
        Arc::new(LongRunningAdapter::new(Duration::from_secs(60)));
    let composition = FoundationRuntimeComposition::with_local_adapter(adapter, 1)
        .expect("composition must build with capacity 1");

    let execution_id = composition
        .dispatch_background(request("exec-cooperative-1"))
        .await
        .expect("dispatch must succeed when capacity is available");

    let started = Instant::now();
    composition
        .cancel(&execution_id)
        .expect("cancel must succeed for a known execution id");
    let outcome = composition
        .wait_for_terminal(&execution_id)
        .await
        .expect("terminal outcome must be reported");
    let elapsed = started.elapsed();

    assert_eq!(
        outcome,
        LocalExecutionOutcome::Cancelled,
        "cooperative cancel must record the Cancelled terminal state"
    );
    assert!(
        elapsed < Duration::from_millis(2_500),
        "forced-termination deadline must be <= 2 s + 250 ms slack; got {elapsed:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn late_results_are_discarded_after_cancellation() {
    let adapter: Arc<dyn LocalExecutionAdapter> = Arc::new(LateCompletingAdapter);
    let composition = FoundationRuntimeComposition::with_local_adapter(adapter, 1)
        .expect("composition must build with capacity 1");

    let execution_id = composition
        .dispatch_background(request("exec-late-1"))
        .await
        .expect("dispatch must succeed when capacity is available");
    composition
        .cancel(&execution_id)
        .expect("cancel must succeed for a known execution id");
    let outcome = composition
        .wait_for_terminal(&execution_id)
        .await
        .expect("terminal outcome must be reported");
    assert_eq!(
        outcome,
        LocalExecutionOutcome::Cancelled,
        "the late Completed result must not overwrite a recorded Cancelled state"
    );
    // Wait past the adapter's natural completion window and ensure the
    // terminal state still reads Cancelled.
    tokio::time::sleep(Duration::from_millis(400)).await;
    let second = composition
        .wait_for_terminal(&execution_id)
        .await
        .expect("terminal outcome remains observable");
    assert_eq!(
        second,
        LocalExecutionOutcome::Cancelled,
        "subsequent reads must keep returning Cancelled; observed {second:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn unknown_cancellation_is_rejected_with_unknown_error() {
    let _workspace = TestWorkspace::new().expect("test workspace");
    let adapter: Arc<dyn LocalExecutionAdapter> =
        Arc::new(LongRunningAdapter::new(Duration::from_millis(10)));
    let composition = FoundationRuntimeComposition::with_local_adapter(adapter, 1)
        .expect("composition must build with capacity 1");
    let result = composition.cancel("not-tracked");
    match result {
        Err(LocalExecutionError::Unknown(_)) => {}
        other => panic!("expected Unknown error, got {other:?}"),
    }
}

#[test]
fn cancelled_outcome_string_is_stable() {
    assert_eq!(
        LocalExecutionOutcome::Cancelled.as_str(),
        "cancelled",
        "the Cancelled wire string is consumed by the activity log and UI summary"
    );
}
