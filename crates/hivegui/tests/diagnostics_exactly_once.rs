use std::sync::{Arc, Mutex};

use hivegui::runtime::diagnostics::{
    DiagnosticRecord, DiagnosticSink, RuntimeErrorBoundary, RuntimeFailure,
};

const EXECUTION_ID: &str = "018f7b36-7a47-7c0f-9da4-4f486e9d12a1";
const SECRET: &str = "<fixture-sensitive-value>";

#[derive(Default)]
struct RecordingDiagnosticSink {
    records: Mutex<Vec<DiagnosticRecord>>,
}

impl RecordingDiagnosticSink {
    fn records(&self) -> Vec<DiagnosticRecord> {
        self.records.lock().expect("diagnostic sink lock").clone()
    }
}

impl DiagnosticSink for RecordingDiagnosticSink {
    fn record(&self, diagnostic: DiagnosticRecord) {
        self.records
            .lock()
            .expect("diagnostic sink lock")
            .push(diagnostic);
    }
}

fn provider_adapter(error: RuntimeFailure) -> RuntimeFailure {
    error.with_context("provider_adapter")
}

fn agent_runner_adapter(error: RuntimeFailure) -> RuntimeFailure {
    error.with_context("agent_runner_adapter")
}

#[test]
fn one_internal_error_crossing_adapters_is_logged_only_once_at_the_handling_boundary() {
    let sink = Arc::new(RecordingDiagnosticSink::default());
    let boundary = RuntimeErrorBoundary::new(sink.clone());
    let failure = RuntimeFailure::internal(
        EXECUTION_ID,
        format!("malformed provider frame; Authorization=Bearer {SECRET}"),
    );
    let failure = agent_runner_adapter(provider_adapter(failure));
    let second_observer = failure.clone();

    assert!(
        sink.records().is_empty(),
        "adapters may add context but must not log an error they do not handle"
    );

    let user_error = boundary.handle("local_agent.execute", failure);
    let event_error = boundary.handle("local_agent.execute", second_observer);

    let records = sink.records();
    assert_eq!(
        records.len(),
        1,
        "the same internal failure must be recorded exactly once even when multiple adapters observe it"
    );
    assert_eq!(records[0].execution_id(), EXECUTION_ID);
    assert_eq!(records[0].operation(), "local_agent.execute");
    assert_eq!(records[0].error_kind(), "internal");
    assert!(records[0].cause().contains("malformed provider frame"));
    assert!(!records[0].cause().contains(SECRET));

    for public_error in [user_error, event_error] {
        assert_eq!(public_error.execution_id(), EXECUTION_ID);
        assert_eq!(public_error.kind(), "internal");
        assert!(!public_error.message().contains("malformed provider frame"));
        assert!(!public_error.message().contains(SECRET));
    }
}
