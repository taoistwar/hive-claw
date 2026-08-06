use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
};

use hive_runtime_core::{
    abi::StableErrorKind,
    execution::{
        EventSink, ExecutionContext, ExecutionPhase, PermissionSnapshot, RuntimeEvent,
        RuntimeEventKind, TerminalOutcome,
    },
};

const EXECUTION_ID: &str = "018f7b36-7a47-7c0f-9da4-4f486e9d12a1";
const SESSION_ID: &str = "018f7b36-7a47-7c0f-9da4-4f486e9d12a2";

#[derive(Default)]
struct RecordingEventSink {
    events: Mutex<Vec<RuntimeEvent>>,
}

impl RecordingEventSink {
    fn events(&self) -> Vec<RuntimeEvent> {
        self.events.lock().expect("event sink lock").clone()
    }
}

impl EventSink for RecordingEventSink {
    fn emit(&self, event: RuntimeEvent) {
        self.events.lock().expect("event sink lock").push(event);
    }
}

fn context(sink: Arc<RecordingEventSink>) -> ExecutionContext {
    ExecutionContext::new(
        EXECUTION_ID,
        SESSION_ID,
        "agent-root",
        PermissionSnapshot::new(BTreeSet::from(["network.http".to_owned()])),
        sink,
    )
    .expect("valid execution context")
}

fn completed_outcome() -> TerminalOutcome {
    TerminalOutcome::Completed {
        final_agent_id: "agent-root".to_owned(),
        message_id: "message-1".to_owned(),
        elapsed_ms: 12,
    }
}

#[test]
fn events_share_execution_metadata_and_one_monotonic_sequence() {
    let sink = Arc::new(RecordingEventSink::default());
    let root = context(sink.clone());
    let child = root.derive_for_agent("agent-child");

    root.emit(RuntimeEventKind::Status {
        phase: ExecutionPhase::Running,
    })
    .expect("emit running status");
    child
        .emit(RuntimeEventKind::Token {
            text: "hello".to_owned(),
            agent_id: "agent-child".to_owned(),
        })
        .expect("emit child token");
    root.finish(completed_outcome())
        .expect("emit completed terminal event");

    let events = sink.events();
    assert_eq!(events.len(), 3);
    let sequences = events
        .iter()
        .map(RuntimeEvent::sequence)
        .collect::<Vec<_>>();
    assert!(
        sequences.windows(2).all(|pair| pair[0] < pair[1]),
        "derived contexts must share one strictly increasing per-execution sequencer"
    );
    assert!(
        events
            .iter()
            .all(|event| event.execution_id() == EXECUTION_ID)
    );
    assert!(events.iter().all(|event| event.session_id() == SESSION_ID));
    assert!(
        events.iter().all(|event| !event.occurred_at().is_empty()),
        "every emitted event must carry an occurred_at value"
    );
}

#[test]
fn all_derived_contexts_share_one_terminal_gate() {
    let sink = Arc::new(RecordingEventSink::default());
    let root = context(sink.clone());
    let child = root.derive_for_agent("agent-child");

    root.finish(completed_outcome())
        .expect("the first terminal outcome wins");

    let duplicate = child
        .finish(TerminalOutcome::Failed {
            error_kind: StableErrorKind::Internal,
            message: "must not replace completed".to_owned(),
            elapsed_ms: 13,
        })
        .expect_err("a second terminal outcome must be rejected");
    assert!(duplicate.is_already_terminal());

    let terminal_count = sink
        .events()
        .iter()
        .filter(|event| event.kind().is_terminal())
        .count();
    assert_eq!(terminal_count, 1);
}

#[test]
fn permission_snapshot_is_immutable_and_propagates_to_children() {
    let mut granted = BTreeSet::from(["network.http".to_owned()]);
    let snapshot = PermissionSnapshot::new(granted.clone());
    let sink = Arc::new(RecordingEventSink::default());
    let root = ExecutionContext::new(EXECUTION_ID, SESSION_ID, "agent-root", snapshot, sink)
        .expect("valid execution context");

    granted.clear();
    let child = root.derive_for_agent("agent-child");

    assert!(root.permissions().allows("network.http"));
    assert!(child.permissions().allows("network.http"));
    assert!(!root.permissions().allows("filesystem.read"));
    assert!(!child.permissions().allows("filesystem.read"));
}

#[test]
fn child_cancellation_is_isolated_but_parent_cancellation_propagates() {
    let sink = Arc::new(RecordingEventSink::default());
    let root = context(sink);
    let first_child = root.derive_for_agent("agent-first");
    let second_child = root.derive_for_agent("agent-second");

    first_child.cancel();
    assert!(first_child.is_cancelled());
    assert!(!root.is_cancelled());
    assert!(!second_child.is_cancelled());

    root.cancel();
    assert!(root.is_cancelled());
    assert!(second_child.is_cancelled());
}
