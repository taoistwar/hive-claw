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
    collections::BTreeSet,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use hivegui::runtime::execution::{
    CancelHandle, CancellationLayer, FoundationRuntimeComposition, LayerCancellationAdapter,
    LayerCancellationFuture, LayerExecutionRequest, LayeredCancellationRuntime,
    LocalExecutionAdapter, LocalExecutionError, LocalExecutionFuture, LocalExecutionOutcome,
    LocalExecutionRequest,
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

#[derive(Debug, Default)]
struct RecordingLayerAdapter {
    started: Arc<Mutex<Vec<(String, CancellationLayer)>>>,
}

impl RecordingLayerAdapter {
    fn started_layers(&self, execution_id: &str) -> BTreeSet<CancellationLayer> {
        self.started
            .lock()
            .expect("started poisoned")
            .iter()
            .filter(|(id, _)| id == execution_id)
            .map(|(_, layer)| *layer)
            .collect()
    }
}

impl LayerCancellationAdapter for RecordingLayerAdapter {
    fn execute_layer(
        &self,
        request: LayerExecutionRequest,
        cancel: CancelHandle,
    ) -> LayerCancellationFuture {
        self.started
            .lock()
            .expect("started poisoned")
            .push((request.execution_id().to_string(), request.layer()));
        Box::pin(async move {
            if request.execution_id() == "00000000-0000-4000-8000-000000000002" {
                tokio::time::sleep(Duration::from_millis(25)).await;
                return Ok(LocalExecutionOutcome::Completed);
            }
            if request.layer() == CancellationLayer::Tool {
                return Ok(LocalExecutionOutcome::Completed);
            }
            if request.layer() == CancellationLayer::Plugin {
                std::future::pending::<()>().await;
                unreachable!("Plugin future is force-terminated by the runtime")
            }
            while !cancel.is_cancelled() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            Ok(LocalExecutionOutcome::Cancelled)
        })
    }
}

async fn wait_until_started(adapter: &RecordingLayerAdapter, execution_id: &str, expected: usize) {
    tokio::time::timeout(Duration::from_secs(1), async {
        while adapter.started_layers(execution_id).len() < expected {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("all requested layers must start");
}

#[tokio::test(flavor = "current_thread")]
async fn stopping_one_agent_propagates_to_every_layer_and_closes_new_scheduling() {
    let execution_id = "00000000-0000-4000-8000-000000000001";
    let session_id = "10000000-0000-4000-8000-000000000001";
    let planned = [
        CancellationLayer::Agent,
        CancellationLayer::ChildAgent,
        CancellationLayer::Llm,
        CancellationLayer::Tool,
        CancellationLayer::Workflow,
        CancellationLayer::Plugin,
    ];
    let adapter = Arc::new(RecordingLayerAdapter::default());
    let runtime = LayeredCancellationRuntime::new(adapter.clone());
    runtime
        .start_execution(execution_id, session_id, &planned)
        .expect("start layered execution");
    for (layer, external_side_effect) in [
        (CancellationLayer::Agent, false),
        (CancellationLayer::ChildAgent, false),
        (CancellationLayer::Llm, false),
        (CancellationLayer::Tool, true),
        (CancellationLayer::Plugin, false),
    ] {
        runtime
            .dispatch_step(execution_id, layer, external_side_effect)
            .await
            .expect("dispatch planned layer");
    }
    wait_until_started(&adapter, execution_id, 5).await;
    tokio::time::sleep(Duration::from_millis(25)).await;

    let stopped_at = Instant::now();
    runtime
        .cancel_execution(execution_id)
        .expect("cancel known layered execution");
    let rejected = runtime
        .dispatch_step(execution_id, CancellationLayer::Workflow, false)
        .await
        .expect_err("no new work may be scheduled after Stop");
    assert_eq!(rejected.field(), "execution_id");
    assert_eq!(rejected.reason(), "scheduling_closed");
    let summary = runtime
        .wait_for_terminal(execution_id)
        .await
        .expect("cancelled summary");
    assert!(
        stopped_at.elapsed() <= Duration::from_millis(2_250),
        "non-cooperative Plugin must be force-terminated within 2s + slack"
    );
    assert_eq!(summary.status(), "cancelled");
    assert_eq!(
        summary.completed_layers(),
        &[CancellationLayer::Tool],
        "the completed external Tool side effect is retained"
    );
    assert_eq!(summary.not_started_layers(), &[CancellationLayer::Workflow]);
    assert_eq!(
        summary
            .interrupted_layers()
            .iter()
            .copied()
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([
            CancellationLayer::Agent,
            CancellationLayer::ChildAgent,
            CancellationLayer::Llm,
            CancellationLayer::Plugin,
        ])
    );
    assert_eq!(
        summary.side_effect_notice(),
        Some("completed external side effects are not rolled back")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn cancelling_one_execution_does_not_stop_another_session() {
    let adapter = Arc::new(RecordingLayerAdapter::default());
    let runtime = LayeredCancellationRuntime::new(adapter.clone());
    let first = "00000000-0000-4000-8000-000000000001";
    let second = "00000000-0000-4000-8000-000000000002";
    runtime
        .start_execution(
            first,
            "10000000-0000-4000-8000-000000000001",
            &[CancellationLayer::Plugin],
        )
        .expect("start first session");
    runtime
        .start_execution(
            second,
            "10000000-0000-4000-8000-000000000002",
            &[CancellationLayer::Llm],
        )
        .expect("start second session");
    runtime
        .dispatch_step(first, CancellationLayer::Plugin, false)
        .await
        .expect("dispatch first Plugin");
    runtime
        .dispatch_step(second, CancellationLayer::Llm, false)
        .await
        .expect("dispatch second LLM");
    wait_until_started(&adapter, first, 1).await;
    wait_until_started(&adapter, second, 1).await;
    runtime.cancel_execution(first).expect("cancel first only");
    let second_summary = runtime
        .wait_for_terminal(second)
        .await
        .expect("second session terminal");
    assert_eq!(second_summary.status(), "completed");
    assert_eq!(
        runtime
            .wait_for_terminal(first)
            .await
            .expect("first cancelled")
            .status(),
        "cancelled"
    );
}
