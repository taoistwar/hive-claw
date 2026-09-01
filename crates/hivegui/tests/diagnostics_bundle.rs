//! T131 [US13] Diagnostic bundle contract.
//!
//! Source of truth: `specs/011-hivegui-standalone-mode/tasks.md` §T131
//! ("在 `crates/hivegui/src/runtime/diagnostics.rs` 和
//! `crates/hivegui/src/logging.rs` 复用 T027 已 Green 的持久日志
//! 边界，实现按 execution_id 汇集 Agent/LLM/Tool/Workflow/Plugin/
//! Capability 事件与导出脱敏诊断包").
//!
//! Red public boundaries the T131 implementation must satisfy:
//!   - `hivegui::runtime::diagnostics::ExecutionEventCollector` collects
//!     events keyed by `execution_id` and exposes a `for_execution`
//!     accessor.
//!   - `hivegui::runtime::diagnostics::DiagnosticBundle::export_redacted`
//!     writes a JSON bundle that excludes the unique plaintext canary
//!     payload placed by the test, and includes the event records.
//!   - The bundle excludes prompt / token / conversation / tool
//!     payload / device key / backup passphrase material.

#![allow(missing_docs)]

mod support;

use std::sync::Arc;

use hivegui::runtime::diagnostics::{
    DiagnosticBundle, ExecutionEventCollector, RedactedBundle, RedactionConfig,
};
use support::TestWorkspace;
use support::sensitive_canary::{SensitiveField, place_canary_for_test, scan_all_mediums_for_test};

#[test]
fn execution_event_collector_groups_by_execution_id() {
    let collector = ExecutionEventCollector::new();
    let execution_id = "018f7b36-7a47-7c0f-9da4-4f486e9d12a1";
    collector.record_agent_event(execution_id, "agent.started", "started turn");
    collector.record_tool_event(execution_id, "format_template", "rendered");
    collector.record_workflow_event(execution_id, "workflow.step", "node=1");
    collector.record_plugin_event(execution_id, "plugin.invoke", "ok");
    collector.record_capability_event(execution_id, "capability.check", "allowed");
    let events = collector.for_execution(execution_id);
    assert_eq!(
        events.len(),
        5,
        "all five events must be grouped by execution_id"
    );
}

#[test]
fn export_redacted_bundle_excludes_sensitive_canary() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let canary = place_canary_for_test(SensitiveField::ChatMessageContent, "T131-canary-prompt")
        .expect("canary placed");
    let collector = Arc::new(ExecutionEventCollector::new());
    collector.record_agent_event(
        "018f7b36-7a47-7c0f-9da4-4f486e9d12a1",
        "agent.started",
        "started",
    );
    let bundle = DiagnosticBundle::new(collector);
    let target = workspace
        .root()
        .join(format!("diagnostic-{}.json", uuid::Uuid::new_v4()));
    let redacted: RedactedBundle = bundle
        .export_redacted(&target, &RedactionConfig::default())
        .expect("export must succeed");
    let json = std::fs::read_to_string(redacted.path()).expect("read bundle");
    assert!(
        !json.contains(&canary.plaintext_token()),
        "bundle must not contain the prompt canary"
    );
    let scan = scan_all_mediums_for_test(workspace.root(), &canary).expect("scan");
    // No medium at all should carry the canary.
    assert!(
        scan.hits.is_empty(),
        "no medium may carry the canary; hits: {:?}",
        scan.hits
    );
}

#[test]
fn redaction_config_disables_prompt_field_explicitly() {
    let config = RedactionConfig::default();
    assert!(
        config.redact_prompt,
        "prompt redaction must be enabled by default"
    );
    assert!(
        config.redact_token,
        "token redaction must be enabled by default"
    );
    assert!(
        config.redact_tool_payload,
        "tool payload redaction must be enabled by default"
    );
    assert!(
        config.redact_conversation,
        "conversation redaction must be enabled by default"
    );
    assert!(
        config.redact_device_key,
        "device key redaction must be enabled by default"
    );
    assert!(
        config.redact_backup_passphrase,
        "backup passphrase redaction must be enabled by default"
    );
}
