//! T067 [P] [US7] Runtime capability catalog contract.
//!
//! Source of truth: `specs/011-hivegui-standalone-mode/tasks.md` §T067
//! ("在 `crates/hivegui/tests/runtime_capability_catalog.rs` 编写数据
//! 库元数据与真实 handler 分离、未知/未授权/参数错误顺序和脱敏事件
//! 测试").
//!
//! Red public boundaries (T070 will add these):
//!   - `hivegui::runtime::capability_adapter::CapabilityAdapter`
//!   - `hivegui::runtime::capability_adapter::HandlerRegistry`
//!   - `hivegui::runtime::capability_adapter::AdapterErrorCode`
//!   - `hivegui::runtime::capability_adapter::DispatchAuditEvent`
//!   - `hivegui::runtime::capability_adapter::CapabilityDispatchError`
//!
//! T068 reviewer signs §T067.11; T070 implementation then makes these
//! tests Green; T071 reruns to record the Green evidence.

use std::sync::{Arc, Mutex};

use hivegui::datasource::{Store, entity_store::Capability};
use hivegui::runtime::capability_adapter::{
    AdapterErrorCode, CapabilityAdapter, CapabilityDispatchError, DispatchAuditEvent,
    HandlerRegistry,
};
use hivegui::runtime::{DesktopHostDispatcher, diagnostics::DiagnosticSink};
use serde_json::Value;

#[derive(Default)]
struct CollectingSink {
    events: Mutex<Vec<DispatchAuditEvent>>,
}

impl DiagnosticSink for CollectingSink {
    fn record(&self, _diagnostic: hivegui::runtime::diagnostics::DiagnosticRecord) {}
}

impl hivegui::runtime::capability_adapter::AuditSink for CollectingSink {
    fn record_event(&self, event: DispatchAuditEvent) {
        self.events.lock().expect("audit lock").push(event);
    }
}

fn collect_audit_events() -> Arc<CollectingSink> {
    Arc::new(CollectingSink::default())
}

fn parse_reply(envelope: &str) -> Value {
    serde_json::from_str(envelope).expect("reply must be valid JSON")
}

// ===========================================================================
// §T067.0 — Store 启动时注册运行能力目录并保留用户自定义 Capability
// ===========================================================================
//
// 该子断言已在 US7 之前就绪,保留在 T067 顶部以防 runtime adapter 重构
// 失去对 Store 自定义 capability 行的兜底语义。T070 implementation 不
// 得破坏这条 Green 行。

#[tokio::test]
async fn store_registers_runtime_capability_catalog_and_preserves_custom_entries() {
    let db_dir = tempfile::tempdir().expect("create temporary HiveGUI data directory");
    let store = Store::new(db_dir.path())
        .await
        .expect("create HiveGUI store");

    Capability::create(
        store.pool(),
        "net".to_string(),
        "user-defined network group".to_string(),
        false,
        None,
    )
    .await
    .expect("create custom capability");
    drop(store);

    let reopened = Store::new(db_dir.path())
        .await
        .expect("reopen HiveGUI store");
    let network_http = Capability::get(reopened.pool(), "network.http")
        .await
        .expect("query standard capability")
        .expect("network.http is registered at startup");

    assert!(network_http.is_dangerous);
    assert!(
        Capability::get(reopened.pool(), "net")
            .await
            .expect("query custom capability")
            .is_some(),
        "startup catalog registration must preserve user-defined capabilities"
    );
}

// ===========================================================================
// §T067.1 — 数据库元数据与真实 handler 分离
// ===========================================================================
//
// A capability may exist in the `capabilities` table as a metadata row
// (description + is_dangerous) without a registered local handler. Such
// rows must NOT silently become dispatchable. T070 will introduce a
// `CapabilityAdapter` whose `HandlerRegistry` is the single source of
// truth for *what is actually executable*. The adapter's stable error
// code (`AdapterErrorCode::MetadataOnly` /
// `AdapterErrorCode::HandlerMissing`) must surface unchanged through
// `DesktopHostDispatcher::dispatch`.

#[tokio::test]
async fn metadata_only_capability_is_rejected_with_stable_error_code() {
    // Register only the metadata row; do NOT register a handler.
    let registry = HandlerRegistry::for_test_with_metadata_only(["db.execute"]);
    let adapter = CapabilityAdapter::new(registry);
    let events = collect_audit_events();
    let dispatcher = DesktopHostDispatcher::with_capability_adapter(adapter, events.clone());

    let reply = dispatcher
        .dispatch(
            r#"{"capability":"db.execute","args":{"query":"select 1"}}"#,
            &["db.execute".to_string()],
        )
        .await;
    let json = parse_reply(&reply);
    assert_eq!(
        json["ok"],
        Value::Bool(false),
        "metadata-only capability must not execute: {json}"
    );
    let code = json["code"]
        .as_u64()
        .expect("dispatcher must emit a stable numeric code");
    assert_eq!(
        AdapterErrorCode::from_wire(code),
        AdapterErrorCode::MetadataOnly,
        "metadata-only capability must surface the MetadataOnly stable code; got wire {code}"
    );

    // Audit events must record one metadata-only attempt, no handler
    // execution, and no secrets in the message.
    let events = events.events.lock().expect("audit lock").clone();
    assert_eq!(
        events.len(),
        1,
        "exactly one audit event expected; got {events:?}"
    );
    assert_eq!(events[0].capability(), "db.execute");
    assert!(!events[0].handler_invoked());
    assert_eq!(events[0].outcome(), "metadata_only");
    assert!(!events[0].message().contains("Bearer"));
    assert!(!events[0].message().contains("Authorization"));
}

#[tokio::test]
async fn capability_with_handler_is_dispatched_via_handler() {
    // The same capability with a real local handler must NOT return
    // the MetadataOnly code, even if a metadata row exists.
    let registry = HandlerRegistry::for_test_with_handler("time.now", |_args: &Value| {
        Ok(serde_json::json!({"unix": 1_700_000_000_i64}))
    });
    let adapter = CapabilityAdapter::new(registry);
    let events = collect_audit_events();
    let dispatcher = DesktopHostDispatcher::with_capability_adapter(adapter, events.clone());

    let reply = dispatcher
        .dispatch(
            r#"{"capability":"time.now","args":{}}"#,
            &["time.now".to_string()],
        )
        .await;
    let json = parse_reply(&reply);
    assert_eq!(
        json["ok"],
        Value::Bool(true),
        "handler-backed dispatch must succeed: {json}"
    );
    assert_eq!(json["data"]["unix"], 1_700_000_000_i64);

    let events = events.events.lock().expect("audit lock").clone();
    assert_eq!(events.len(), 1);
    assert!(events[0].handler_invoked());
    assert_eq!(events[0].outcome(), "ok");
}

#[tokio::test]
async fn metadata_only_capability_does_not_bypass_authorization_check() {
    // Even when no handler is registered, the permission snapshot is
    // still consulted: an unauthorized caller must see the unauthorized
    // error before the metadata-only error.
    let registry = HandlerRegistry::for_test_with_metadata_only(["db.execute"]);
    let adapter = CapabilityAdapter::new(registry);
    let events = collect_audit_events();
    let dispatcher = DesktopHostDispatcher::with_capability_adapter(adapter, events.clone());

    let reply = dispatcher
        .dispatch(
            r#"{"capability":"db.execute","args":{"query":"select 1"}}"#,
            &[],
        )
        .await;
    let json = parse_reply(&reply);
    let code = json["code"].as_u64().expect("stable numeric code");
    assert_eq!(
        AdapterErrorCode::from_wire(code),
        AdapterErrorCode::Unauthorized,
        "unauthorized must take precedence over metadata-only"
    );
}

// ===========================================================================
// §T067.2 — 未知/未授权/参数错误顺序
// ===========================================================================
//
// The dispatcher's order of checks is part of the contract. T070 will
// lock the order as: parse → unknown → unauthorized → handler-existence
// → argument-shape. Each test below exercises one boundary and proves
// that a "later" failure does not mask an "earlier" failure.

#[tokio::test]
async fn unknown_capability_takes_precedence_over_argument_errors() {
    let registry =
        HandlerRegistry::for_test_with_handler("time.now", |_| Ok(serde_json::json!({"unix": 0})));
    let adapter = CapabilityAdapter::new(registry);
    let events = collect_audit_events();
    let dispatcher = DesktopHostDispatcher::with_capability_adapter(adapter, events.clone());

    // "unknown.capability" is both unknown AND has a malformed args
    // shape (numeric instead of object). Unknown must win.
    let reply = dispatcher
        .dispatch(
            r#"{"capability":"unknown.capability","args":42}"#,
            &["unknown.capability".to_string()],
        )
        .await;
    let json = parse_reply(&reply);
    let code = json["code"].as_u64().expect("stable numeric code");
    assert_eq!(
        AdapterErrorCode::from_wire(code),
        AdapterErrorCode::Unknown,
        "unknown must precede argument validation"
    );
}

#[tokio::test]
async fn unauthorized_takes_precedence_over_handler_argument_errors() {
    let registry =
        HandlerRegistry::for_test_with_handler("time.now", |_| Ok(serde_json::json!({"unix": 0})));
    let adapter = CapabilityAdapter::new(registry);
    let events = collect_audit_events();
    let dispatcher = DesktopHostDispatcher::with_capability_adapter(adapter, events.clone());

    // time.now is known and has a handler, but the caller did not
    // authorize it. The args object is also malformed (must be {}
    // since time.now expects no args). Unauthorized must win.
    let reply = dispatcher
        .dispatch(r#"{"capability":"time.now","args":{"not":"empty"}}"#, &[])
        .await;
    let json = parse_reply(&reply);
    let code = json["code"].as_u64().expect("stable numeric code");
    assert_eq!(
        AdapterErrorCode::from_wire(code),
        AdapterErrorCode::Unauthorized,
        "unauthorized must precede handler argument validation"
    );
}

#[tokio::test]
async fn metadata_only_takes_precedence_over_handler_argument_errors() {
    let registry = HandlerRegistry::for_test_with_metadata_only(["time.now"]);
    let adapter = CapabilityAdapter::new(registry);
    let events = collect_audit_events();
    let dispatcher = DesktopHostDispatcher::with_capability_adapter(adapter, events.clone());

    // Caller has the right name in `allowed_capabilities`, but the
    // metadata row is registered with no handler. Even with malformed
    // args, the metadata-only error must come first.
    let reply = dispatcher
        .dispatch(
            r#"{"capability":"time.now","args":{"nope":1}}"#,
            &["time.now".to_string()],
        )
        .await;
    let json = parse_reply(&reply);
    let code = json["code"].as_u64().expect("stable numeric code");
    assert_eq!(
        AdapterErrorCode::from_wire(code),
        AdapterErrorCode::MetadataOnly,
        "metadata-only must precede handler argument validation"
    );
}

#[tokio::test]
async fn parse_error_surfaces_first_when_envelope_is_not_json() {
    let registry =
        HandlerRegistry::for_test_with_handler("time.now", |_| Ok(serde_json::json!({"unix": 0})));
    let adapter = CapabilityAdapter::new(registry);
    let events = collect_audit_events();
    let dispatcher = DesktopHostDispatcher::with_capability_adapter(adapter, events.clone());

    let reply = dispatcher
        .dispatch("not-a-json-envelope", &["time.now".to_string()])
        .await;
    let json = parse_reply(&reply);
    let code = json["code"].as_u64().expect("stable numeric code");
    assert_eq!(
        AdapterErrorCode::from_wire(code),
        AdapterErrorCode::InvalidEnvelope,
        "parse error must be the first observable failure"
    );
}

#[tokio::test]
async fn error_order_summary_matrix() {
    // The order must be stable across multiple dispatchers / inputs.
    // T070 implementation must keep this matrix intact.
    use AdapterErrorCode as C;
    let cases: Vec<(&str, Vec<String>, C)> = vec![
        // (envelope, allowed, expected_code)
        (
            "not-a-json-envelope",
            vec!["time.now".to_string()],
            C::InvalidEnvelope,
        ),
        (
            r#"{"capability":"unknown.capability","args":{}}"#,
            vec!["unknown.capability".to_string()],
            C::Unknown,
        ),
        (
            r#"{"capability":"time.now","args":{}}"#,
            vec![],
            C::Unauthorized,
        ),
        (
            r#"{"capability":"time.now","args":{"bad":"shape"}}"#,
            vec!["time.now".to_string()],
            C::MetadataOnly,
        ),
    ];

    // Use a metadata-only registry so the handler-existence check
    // (MetadataOnly) precedes the argument-shape check. The matrix
    // exercises: parse → unknown → unauthorized → handler-existence.
    let registry = HandlerRegistry::for_test_with_metadata_only(["time.now"]);
    let adapter = CapabilityAdapter::new(registry);
    let events = collect_audit_events();
    let dispatcher = DesktopHostDispatcher::with_capability_adapter(adapter, events.clone());

    for (envelope, allowed, expected) in cases {
        let reply = dispatcher.dispatch(envelope, &allowed).await;
        let json = parse_reply(&reply);
        let code = json["code"].as_u64().expect("stable numeric code");
        assert_eq!(
            AdapterErrorCode::from_wire(code),
            expected,
            "envelope={envelope:?} allowed={allowed:?} must surface {expected:?}; got reply={json}"
        );
    }
}

// ===========================================================================
// §T067.3 — 脱敏事件
// ===========================================================================
//
// The dispatcher must record an audit event for every call. The event
// must carry the capability name, the redacted outcome and a message
// that does NOT contain Authorization headers, Bearer tokens or any
// other secret-shaped material that may have leaked from the args.

#[tokio::test]
async fn audit_event_message_is_sanitized_for_authorization_header() {
    let registry =
        HandlerRegistry::for_test_with_handler("time.now", |_| Ok(serde_json::json!({"unix": 0})));
    let adapter = CapabilityAdapter::new(registry);
    let events = collect_audit_events();
    let dispatcher = DesktopHostDispatcher::with_capability_adapter(adapter, events.clone());

    let reply = dispatcher
        .dispatch(
            r#"{"capability":"time.now","args":{"Authorization":"Bearer super-secret-token","nested":{"api_key":"abc123"}}}"#,
            &["time.now".to_string()],
        )
        .await;
    let _ = parse_reply(&reply);
    let events = events.events.lock().expect("audit lock").clone();
    assert_eq!(events.len(), 1);
    let message = events[0].message();
    assert!(
        !message.contains("super-secret-token"),
        "raw bearer token must not appear in the audit message: {message}"
    );
    assert!(
        !message.contains("abc123"),
        "raw api_key value must not appear in the audit message: {message}"
    );
    assert!(
        message.contains("<redacted>") || message.contains("Authorization"),
        "audit message must indicate redaction, not silently drop context: {message}"
    );
}

#[tokio::test]
async fn audit_event_includes_handler_invoked_flag_and_correlation_id() {
    let registry =
        HandlerRegistry::for_test_with_handler("time.now", |_| Ok(serde_json::json!({"unix": 0})));
    let adapter = CapabilityAdapter::new(registry);
    let events = collect_audit_events();
    let dispatcher = DesktopHostDispatcher::with_capability_adapter(adapter, events.clone());

    let reply = dispatcher
        .dispatch(
            r#"{"capability":"time.now","args":{},"correlation_id":"exec-12345"}"#,
            &["time.now".to_string()],
        )
        .await;
    let _ = parse_reply(&reply);
    let events = events.events.lock().expect("audit lock").clone();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].capability(), "time.now");
    assert!(events[0].handler_invoked());
    assert_eq!(events[0].correlation_id(), "exec-12345");
    assert_eq!(events[0].outcome(), "ok");
}

#[tokio::test]
async fn audit_event_records_metadata_only_outcome_without_calling_handler() {
    let registry = HandlerRegistry::for_test_with_metadata_only(["time.now"]);
    let adapter = CapabilityAdapter::new(registry);
    let events = collect_audit_events();
    let dispatcher = DesktopHostDispatcher::with_capability_adapter(adapter, events.clone());

    let reply = dispatcher
        .dispatch(
            r#"{"capability":"time.now","args":{}}"#,
            &["time.now".to_string()],
        )
        .await;
    let json = parse_reply(&reply);
    assert_eq!(json["ok"], Value::Bool(false));
    let events = events.events.lock().expect("audit lock").clone();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].capability(), "time.now");
    assert!(!events[0].handler_invoked());
    assert_eq!(events[0].outcome(), "metadata_only");
}

#[tokio::test]
async fn audit_event_records_handler_failure_with_redacted_message() {
    let registry = HandlerRegistry::for_test_with_handler("time.now", |_| {
        Err("upstream HTTP failed: Authorization=Bearer leaked-token".to_string())
    });
    let adapter = CapabilityAdapter::new(registry);
    let events = collect_audit_events();
    let dispatcher = DesktopHostDispatcher::with_capability_adapter(adapter, events.clone());

    let reply = dispatcher
        .dispatch(
            r#"{"capability":"time.now","args":{}}"#,
            &["time.now".to_string()],
        )
        .await;
    let json = parse_reply(&reply);
    assert_eq!(json["ok"], Value::Bool(false));
    let events = events.events.lock().expect("audit lock").clone();
    assert_eq!(events.len(), 1);
    assert!(events[0].handler_invoked());
    assert_eq!(events[0].outcome(), "handler_error");
    let message = events[0].message();
    assert!(
        !message.contains("leaked-token"),
        "raw token must be redacted: {message}"
    );
}

// ===========================================================================
// §T067.4 — CapabilityAdapter 与 DiagnosticSink 协作
// ===========================================================================
//
// Each audit event must also be delivered to the typed DiagnosticSink
// (Foundation boundary) so the activity log keeps a single record per
// dispatcher call. T070 implementation must keep this one-to-one
// correspondence.

#[tokio::test]
async fn audit_event_is_delivered_to_diagnostic_sink_exactly_once() {
    #[derive(Default)]
    struct LocalSink {
        diagnostics: Mutex<Vec<String>>,
    }
    impl hivegui::runtime::diagnostics::DiagnosticSink for LocalSink {
        fn record(&self, record: hivegui::runtime::diagnostics::DiagnosticRecord) {
            self.diagnostics
                .lock()
                .expect("diagnostic lock")
                .push(record.cause().to_string());
        }
    }

    let local_sink: Arc<LocalSink> = Arc::new(LocalSink::default());
    let events = collect_audit_events();
    let registry =
        HandlerRegistry::for_test_with_handler("time.now", |_| Ok(serde_json::json!({"unix": 0})));
    let adapter = CapabilityAdapter::new(registry);
    let dispatcher = DesktopHostDispatcher::with_capability_adapter(adapter, events.clone())
        .with_diagnostic_sink(local_sink.clone() as Arc<dyn DiagnosticSink>);

    let reply = dispatcher
        .dispatch(
            r#"{"capability":"time.now","args":{}}"#,
            &["time.now".to_string()],
        )
        .await;
    let _ = parse_reply(&reply);

    let diagnostics = local_sink
        .diagnostics
        .lock()
        .expect("diagnostic lock")
        .clone();
    let audits = events.events.lock().expect("audit lock").clone();
    assert_eq!(
        diagnostics.len(),
        audits.len(),
        "diagnostic sink and audit sink must record the same number of events: diagnostics={diagnostics:?} audits={audits:?}"
    );
    assert_eq!(diagnostics.len(), 1);
    assert!(
        !diagnostics[0].contains("Authorization") && !diagnostics[0].contains("Bearer"),
        "diagnostic record must be redacted: {}",
        diagnostics[0]
    );
}

#[tokio::test]
async fn capability_dispatch_error_carries_stable_wire_code() {
    // T070 must introduce a typed error that maps to the stable wire
    // code. UI / log layers consume the typed error; the dispatcher
    // emits the matching JSON envelope.
    let registry = HandlerRegistry::for_test_with_metadata_only(["time.now"]);
    let adapter = CapabilityAdapter::new(registry);
    let events = collect_audit_events();
    let dispatcher = DesktopHostDispatcher::with_capability_adapter(adapter, events.clone());

    let typed: CapabilityDispatchError = dispatcher
        .dispatch_typed(
            r#"{"capability":"time.now","args":{}}"#,
            &["time.now".to_string()],
        )
        .await
        .expect_err("metadata-only dispatch must return Err");
    assert_eq!(typed.wire_code(), AdapterErrorCode::MetadataOnly.wire());
    assert_eq!(typed.capability(), "time.now");
}
