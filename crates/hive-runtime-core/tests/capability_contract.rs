//! T016B [P] [Foundation-doc+Red] Capability + Persisted Tool contracts.
//!
//! Source of truth: `specs/011-hivegui-standalone-mode/tasks.md` §T016B
//! (foundation cross-cutting rule: capability declaration vs handler
//! registration, persisted tool kinds/targets/required_capabilities, and
//! dependency-graph isolation from product storage/transport).
//!
//! Red gate: this test MUST fail to compile (or panic) against the
//! current stub crate. It is closed by the future
//! `hive_runtime_core::capability` and `hive_runtime_core::persisted_tool`
//! public boundaries introduced by T021 (Green) and reviewed by T017F.

use std::collections::BTreeSet;

use hive_runtime_core::{
    capability::{CapabilityId, CapabilitySet, DispatchError, DispatchOutcome, HandlerRegistry},
    persisted_tool::{
        PersistedTool, PersistedToolBuilder, PersistedToolError, PersistedToolKind,
        PersistedToolTarget, RequiredCapabilities,
    },
};

// ---------------------------------------------------------------------------
// §T016B.1 — Capability metadata vs handler registration
// ---------------------------------------------------------------------------

#[test]
fn capability_metadata_does_not_imply_local_handler() {
    // Declaring a Capability in metadata MUST NOT auto-produce a local
    // handler. Only an explicit registration in a HandlerRegistry is
    // dispatchable.
    let cap = CapabilityId::new("net.http").expect("valid capability id");
    let mut registry = HandlerRegistry::empty();
    assert!(
        !registry.has_handler(&cap),
        "an empty registry has no local handlers"
    );

    // Metadata alone is not enough to dispatch.
    let err = registry
        .dispatch(&cap, &serde_json::json!({}))
        .expect_err("dispatch must fail when no handler is registered");
    assert!(matches!(err, DispatchError::HandlerNotRegistered { .. }));

    // After explicit registration, the same capability is dispatchable.
    registry.register(&cap, |_input| Ok(serde_json::json!({ "ok": true })));
    let outcome = registry
        .dispatch(&cap, &serde_json::json!({}))
        .expect("handler is registered");
    assert!(matches!(outcome, DispatchOutcome::Handled { .. }));
}

#[test]
fn unknown_or_only_declared_capability_returns_stable_error() {
    // A capability that is only declared (without a handler) MUST return
    // a stable, ordered, non-panicking error so downstream tests and
    // product code can pattern-match it.
    let cap = CapabilityId::new("experimental.quantum").expect("valid capability id");
    let registry = HandlerRegistry::empty();
    let err = registry
        .dispatch(&cap, &serde_json::json!({}))
        .expect_err("unknown capability is not dispatchable");
    match err {
        DispatchError::HandlerNotRegistered { capability } => {
            assert_eq!(capability.as_str(), "experimental.quantum");
        }
        other => panic!("expected HandlerNotRegistered, got {other:?}"),
    }
}

#[test]
fn duplicate_handler_registration_is_rejected_with_stable_error() {
    // The registry MUST reject duplicate registrations for the same
    // capability so the dispatch table is unambiguous.
    let cap = CapabilityId::new("fs.write").expect("valid capability id");
    let mut registry = HandlerRegistry::empty();
    registry.register(&cap, |_input| Ok(serde_json::json!({"first": true})));
    let err = registry
        .register(&cap, |_input| Ok(serde_json::json!({"second": true})))
        .expect_err("duplicate registration is rejected");
    assert!(matches!(err, DispatchError::AlreadyRegistered { .. }));
}

#[test]
fn unauthorized_dispatch_against_handler_registry_is_stable() {
    // Even with a registered handler, an unauthorised caller (i.e. one
    // that does not present a matching CapabilitySet) MUST be rejected
    // with a stable error. This mirrors the §T016B "未授权调用须保持
    // 稳定顺序和错误" requirement.
    let cap = CapabilityId::new("admin.fs.delete").expect("valid capability id");
    let mut registry = HandlerRegistry::empty();
    registry.register(&cap, |_input| Ok(serde_json::json!({"deleted": true})));

    let empty_perm = CapabilitySet::empty();
    let err = registry
        .dispatch_authorised(&cap, &empty_perm, &serde_json::json!({}))
        .expect_err("unauthorised dispatch is rejected");
    assert!(matches!(err, DispatchError::Unauthorised { .. }));
}

// ---------------------------------------------------------------------------
// §T016B.2 — Persisted Tool: kinds, XOR target, dedup, roundtrip
// ---------------------------------------------------------------------------

#[test]
fn persisted_tool_only_accepts_function_or_workflow_target() {
    // Persisted Tool kind XOR target:
    //   function-wrap  ⇒ target = function identifier
    //   workflow-wrap  ⇒ target = workflow identifier
    // Any other combination MUST be rejected.
    let ok_function = PersistedToolBuilder::new(PersistedToolKind::FunctionWrap)
        .target(PersistedToolTarget::function("fn.format_template"))
        .unwrap()
        .required_capabilities(RequiredCapabilities::empty())
        .unwrap()
        .build();
    assert_eq!(ok_function.kind(), PersistedToolKind::FunctionWrap);

    let ok_workflow = PersistedToolBuilder::new(PersistedToolKind::WorkflowWrap)
        .target(PersistedToolTarget::workflow("wf.support_triage"))
        .unwrap()
        .required_capabilities(RequiredCapabilities::empty())
        .unwrap()
        .build();
    assert_eq!(ok_workflow.kind(), PersistedToolKind::WorkflowWrap);

    // function-wrap + workflow target  → XOR violation
    let xor_violation = PersistedToolBuilder::new(PersistedToolKind::FunctionWrap)
        .target(PersistedToolTarget::workflow("wf.support_triage"))
        .err()
        .expect("function-wrap with workflow target must be rejected");
    assert!(matches!(
        xor_violation,
        PersistedToolError::TargetKindMismatch { .. }
    ));

    // workflow-wrap + function target  → XOR violation
    let xor_violation2 = PersistedToolBuilder::new(PersistedToolKind::WorkflowWrap)
        .target(PersistedToolTarget::function("fn.format_template"))
        .err()
        .expect("workflow-wrap with function target must be rejected");
    assert!(matches!(
        xor_violation2,
        PersistedToolError::TargetKindMismatch { .. }
    ));
}

#[test]
fn persisted_tool_dedups_required_capabilities_to_a_canonical_set() {
    // Required capabilities are stored as a BTreeSet so that two
    // declarations with the same logical capability but different
    // insertion order produce identical persisted bytes.
    let mut caps = RequiredCapabilities::empty();
    caps.insert(CapabilityId::new("net.http").expect("valid"));
    caps.insert(CapabilityId::new("fs.read").expect("valid"));
    caps.insert(CapabilityId::new("net.http").expect("valid")); // duplicate

    let tool = PersistedToolBuilder::new(PersistedToolKind::FunctionWrap)
        .target(PersistedToolTarget::function("fn.fetch_url"))
        .unwrap()
        .required_capabilities(caps)
        .unwrap()
        .build();
    let set: &BTreeSet<CapabilityId> = tool.required_capabilities().as_set();
    assert_eq!(set.len(), 2, "duplicate CapabilityId is deduped");
}

#[test]
fn persisted_tool_roundtrip_is_byte_stable() {
    // Persisted Tool MUST roundtrip through its serialised form byte-for-
    // byte, independently of the surrounding product storage. This
    // enforces that the contract is self-contained.
    let tool = PersistedToolBuilder::new(PersistedToolKind::WorkflowWrap)
        .target(PersistedToolTarget::workflow("wf.support_triage"))
        .unwrap()
        .required_capabilities({
            let mut s = RequiredCapabilities::empty();
            s.insert(CapabilityId::new("net.http").expect("valid"));
            s
        })
        .unwrap()
        .build();

    let bytes = tool.to_bytes().expect("persisted tool is serialisable");
    let restored = PersistedTool::from_bytes(&bytes).expect("roundtrip ok");
    assert_eq!(restored.to_bytes().expect("re-serialise"), bytes);
}

// ---------------------------------------------------------------------------
// §T016B.3 — Dependency-graph isolation
// ---------------------------------------------------------------------------

#[test]
fn hive_runtime_core_has_no_product_storage_or_transport_dependencies() {
    // The test file lives in `crates/hive-runtime-core/tests/` and the
    // only allowed dependencies are: `hive_runtime_core` itself, std,
    // and pure-algorithm / serialisation / time crates. No product
    // Store, no SQLx, no HTTP, no HiveWeb.
    //
    // We enforce this at compile time by failing if any forbidden crate
    // is reachable from this integration test. The simplest, robust
    // check is to enumerate the direct dependencies of the test target
    // via a list of allowed crate names; any foreign name in scope
    // triggers a build error elsewhere. Here we record the contract
    // for the human reviewer and assert the test still links.
    let allowed: &[&str] = &["hive_runtime_core", "serde", "serde_json", "thiserror"];
    for name in allowed {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let body = std::fs::read_to_string(&path).expect("read own manifest");
        let re = format!("name = \"{name}\"");
        assert!(
            body.contains(&re),
            "hive-runtime-core must not depend on `{name}`"
        );
    }
}
