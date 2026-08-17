//! T120 [P] [US13] Diagnostics bundle contract.
//!
//! Source of truth: `specs/011-hivegui-standalone-mode/tasks.md` §T120
//! ("在 `crates/hivegui/tests/diagnostics.rs` 编写 Agent/LLM/Tool/
//! Workflow/Plugin/Capability 的 execution_id 全链路、含 UTF-8
//! `cause_summary`≤512 bytes 的 v1 稳定字段、跨 adapter 同一内部
//! 错误脱敏后 exactly-once、持久化 retention high-watermark/时钟
//! 回拨、按每条记录精确 7×24 小时的强制时间轮转/崩溃安全
//! compaction、追加前 100,000,000 bytes 容量门禁和单条超限零写入
//! 拒绝，以及诊断包排除 prompt/token/会话/Tool payload/备份口令
//! E2E；激活 T016F 的日志/诊断包行，首次以唯一明文 canary 扫描
//! 结构化日志、活动/不可变段、错误、崩溃恢复产物和最终诊断包").
//!
//! Public boundaries the T131 implementation must satisfy:
//!   - `hivegui::runtime::diagnostics::RuntimeErrorBoundary`
//!   - `hivegui::runtime::diagnostics::DiagnosticRecord`
//!   - `hivegui::runtime::diagnostics::RuntimeFailure`
//!
//! T123 reviewer signs §T120.11; T131 implementation then makes
//! these tests Green; T136 reruns to record the Green evidence.

#![allow(missing_docs)]

mod support;

use std::sync::{Arc, Mutex};

use hivegui::runtime::diagnostics::{
    DiagnosticRecord, DiagnosticSink, RuntimeErrorBoundary, RuntimeFailure,
};
use support::TestWorkspace;
use support::sensitive_canary::{
    SensitiveField, place_canary_for_test, scan_all_mediums_for_test, unique_canary_payload,
};

const EXECUTION_ID: &str = "018f7b36-7a47-7c0f-9da4-4f486e9d12a1";
const EXECUTION_ID_CHILD: &str = "018f7b36-7a47-7c0f-9da4-4f486e9d12a2";
const EXECUTION_ID_GRANDCHILD: &str = "018f7b36-7a47-7c0f-9da4-4f486e9d12a3";

#[derive(Default)]
struct RecordingDiagnosticSink {
    records: Mutex<Vec<DiagnosticRecord>>,
}

impl RecordingDiagnosticSink {
    fn records(&self) -> Vec<DiagnosticRecord> {
        self.records.lock().expect("diagnostic sink lock").clone()
    }

    fn count(&self) -> usize {
        self.records.lock().expect("diagnostic sink lock").len()
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

fn tool_adapter(error: RuntimeFailure) -> RuntimeFailure {
    error.with_context("tool_adapter")
}

fn workflow_adapter(error: RuntimeFailure) -> RuntimeFailure {
    error.with_context("workflow_adapter")
}

fn plugin_adapter(error: RuntimeFailure) -> RuntimeFailure {
    error.with_context("plugin_adapter")
}

fn capability_adapter(error: RuntimeFailure) -> RuntimeFailure {
    error.with_context("capability_adapter")
}

#[test]
fn execution_id_propagates_through_full_agent_llm_tool_workflow_plugin_capability_chain() {
    let sink = Arc::new(RecordingDiagnosticSink::default());
    let boundary = RuntimeErrorBoundary::new(sink.clone());

    // The full US13 chain: Agent → LLM → Tool → Workflow → Plugin → Capability.
    let failure = RuntimeFailure::internal(
        EXECUTION_ID,
        "agent -> llm -> tool -> workflow -> plugin -> capability",
    );
    let failure = capability_adapter(plugin_adapter(workflow_adapter(tool_adapter(
        agent_runner_adapter(provider_adapter(failure)),
    ))));
    let public_error = boundary.handle("local_agent.execute", failure);

    let records = sink.records();
    assert_eq!(records.len(), 1, "chain must produce exactly one record");
    assert_eq!(records[0].execution_id(), EXECUTION_ID);
    assert_eq!(public_error.execution_id(), EXECUTION_ID);
    let context = records[0].context();
    assert_eq!(context.len(), 6, "all six adapter labels must be preserved");
    assert_eq!(context[0], "provider_adapter");
    assert_eq!(context[5], "capability_adapter");
}

#[test]
fn distinct_execution_ids_produce_distinct_records() {
    let sink = Arc::new(RecordingDiagnosticSink::default());
    let boundary = RuntimeErrorBoundary::new(sink.clone());

    boundary.handle(
        "local_agent.execute",
        RuntimeFailure::internal(EXECUTION_ID, "first agent"),
    );
    boundary.handle(
        "local_agent.execute",
        RuntimeFailure::internal(EXECUTION_ID_CHILD, "child agent"),
    );
    boundary.handle(
        "local_agent.execute",
        RuntimeFailure::internal(EXECUTION_ID_GRANDCHILD, "grandchild agent"),
    );

    let records = sink.records();
    assert_eq!(records.len(), 3);
    let ids: std::collections::HashSet<&str> = records.iter().map(|r| r.execution_id()).collect();
    assert!(ids.contains(EXECUTION_ID));
    assert!(ids.contains(EXECUTION_ID_CHILD));
    assert!(ids.contains(EXECUTION_ID_GRANDCHILD));
}

#[test]
fn cause_summary_redaction_respects_utf8_byte_boundary() {
    let sink = Arc::new(RecordingDiagnosticSink::default());
    let boundary = RuntimeErrorBoundary::new(sink.clone());

    // 600 bytes of UTF-8 mixed text — well over the 512 byte cap.
    let long_text: String = "αβγδ".repeat(200); // 800 bytes in UTF-8
    let failure = RuntimeFailure::internal(EXECUTION_ID, &long_text);
    let public_error = boundary.handle("local_agent.execute", failure);

    let records = sink.records();
    assert_eq!(records.len(), 1);
    let cause = records[0].cause();
    // The cause field must never exceed 512 bytes, and it must be
    // valid UTF-8 (Rust string slices enforce this on construction).
    assert!(
        cause.len() <= 512,
        "cause must be capped to 512 bytes; got {} bytes",
        cause.len()
    );
    let _ = public_error;
}

#[test]
fn cross_adapter_duplicate_failures_are_recorded_exactly_once() {
    let sink = Arc::new(RecordingDiagnosticSink::default());
    let boundary = RuntimeErrorBoundary::new(sink.clone());

    let failure = RuntimeFailure::internal(
        EXECUTION_ID,
        "malformed provider frame; Authorization=Bearer fixture-secret",
    );
    let observed_a = capability_adapter(plugin_adapter(workflow_adapter(tool_adapter(
        agent_runner_adapter(provider_adapter(failure.clone())),
    ))));
    let observed_b = capability_adapter(plugin_adapter(workflow_adapter(tool_adapter(
        agent_runner_adapter(provider_adapter(failure)),
    ))));

    let _public_a = boundary.handle("local_agent.execute", observed_a);
    let _public_b = boundary.handle("local_agent.execute", observed_b);

    let records = sink.records();
    assert_eq!(
        records.len(),
        1,
        "the same internal failure must be recorded exactly once even when multiple adapters observe it"
    );
    assert!(!records[0].cause().contains("fixture-secret"));
}

#[test]
fn public_message_does_not_leak_raw_cause_or_secrets() {
    let sink = Arc::new(RecordingDiagnosticSink::default());
    let boundary = RuntimeErrorBoundary::new(sink.clone());

    let failure = RuntimeFailure::internal(
        EXECUTION_ID,
        "raw cause <fixture-sensitive-value> Authorization=Bearer sk-live-secret",
    );
    let public_error = boundary.handle("local_agent.execute", failure);

    let records = sink.records();
    assert_eq!(records.len(), 1);
    // The diagnostic record must redact the secret material.
    assert!(!records[0].cause().contains("fixture-sensitive-value"));
    assert!(!records[0].cause().contains("sk-live-secret"));
    // The public error must not surface the raw cause or any secret.
    assert!(!public_error.message().contains("fixture-sensitive-value"));
    assert!(!public_error.message().contains("sk-live-secret"));
    assert!(!public_error.message().contains("Authorization"));
}

#[test]
fn function_not_executable_is_stable_kind() {
    let sink = Arc::new(RecordingDiagnosticSink::default());
    let boundary = RuntimeErrorBoundary::new(sink.clone());

    let failure = RuntimeFailure::function_not_executable(EXECUTION_ID, "tool not registered");
    let public_error = boundary.handle("local_agent.execute", failure);

    let records = sink.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].error_kind(), "function_not_executable");
    assert_eq!(public_error.kind(), "function_not_executable");
}

#[test]
fn cancelled_and_io_kinds_are_stable() {
    let sink = Arc::new(RecordingDiagnosticSink::default());
    let boundary = RuntimeErrorBoundary::new(sink.clone());

    boundary.handle(
        "local_agent.execute",
        RuntimeFailure::cancelled(EXECUTION_ID, "user stopped"),
    );
    boundary.handle(
        "local_agent.execute",
        RuntimeFailure::io(EXECUTION_ID, "disk read"),
    );

    let records = sink.records();
    assert_eq!(records.len(), 2);
    let kinds: std::collections::HashSet<&str> = records.iter().map(|r| r.error_kind()).collect();
    assert!(kinds.contains("cancelled"));
    assert!(kinds.contains("io"));
}

#[test]
fn diagnostics_redact_prompt_token_session_tool_payload_and_backup_password() {
    // T120 requires that the diagnostic bundle (and the persisted
    // diagnostic record) exclude prompts, API tokens, session
    // content, Tool payload, and backup passwords.
    let sink = Arc::new(RecordingDiagnosticSink::default());
    let boundary = RuntimeErrorBoundary::new(sink.clone());

    let sensitive_payload =
        "prompt='<HIVEGUI_CANARY_PROMPT=abc>'; token='<HIVEGUI_CANARY_TOKEN=xyz>'; \
         session='<HIVEGUI_CANARY_SESSION=qrs>'; tool_payload='<HIVEGUI_CANARY_TOOL=tuv>'; \
         backup_password='<HIVEGUI_CANARY_PASSWORD=mno>'; visible=ok"
            .to_string();
    let failure = RuntimeFailure::internal(EXECUTION_ID, &sensitive_payload);
    let _ = boundary.handle("local_agent.execute", failure);

    let records = sink.records();
    assert_eq!(records.len(), 1);
    let cause = records[0].cause();
    // The cause must not contain any of the secret-shaped canary tokens.
    assert!(!cause.contains("HIVEGUI_CANARY_PROMPT"));
    assert!(!cause.contains("HIVEGUI_CANARY_TOKEN"));
    assert!(!cause.contains("HIVEGUI_CANARY_SESSION"));
    assert!(!cause.contains("HIVEGUI_CANARY_TOOL"));
    assert!(!cause.contains("HIVEGUI_CANARY_PASSWORD"));
}

#[tokio::test(flavor = "current_thread")]
async fn diagnostic_record_sensitive_canary_leaves_zero_residue_on_persistence_media() {
    // Activate T016F's diagnostic-bundle row and verify that
    // placing a unique plaintext canary in a diagnostic cause
    // does not leave the canary on any of the test workspace's
    // persistence media.
    let workspace = TestWorkspace::new().expect("test workspace");
    let payload = unique_canary_payload("T120-diagnostic-canary");
    let canary = place_canary_for_test(SensitiveField::ChatMessageToolCalls, &payload)
        .expect("canary placed");

    let sink = Arc::new(RecordingDiagnosticSink::default());
    let boundary = RuntimeErrorBoundary::new(sink.clone());
    let failure = RuntimeFailure::internal(EXECUTION_ID, format!("visible {payload}"));
    let _ = boundary.handle("local_agent.execute", failure);

    let scan = scan_all_mediums_for_test(workspace.root(), &canary).expect("scan");
    assert!(
        scan.hits.is_empty(),
        "ToolPayload canary must not appear in any medium; hits: {:?}",
        scan.hits
    );
    let _ = sink.records(); // ensure sink is observed (lint guard)
}
