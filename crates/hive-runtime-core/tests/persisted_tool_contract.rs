//! T016B [P] [Foundation-doc+Red] Persisted Tool: roundtrip + dependency
//! graph isolation (paired with `capability_contract.rs`).
//!
//! Source of truth: `specs/011-hivegui-standalone-mode/tasks.md` §T016B
//! (foundation cross-cutting rule: persisted tool kinds, XOR target,
//! required_capabilities, and storage-independent serialisation).
//!
//! Red gate: this test MUST fail to compile (or panic) against the
//! current stub crate. The future `hive_runtime_core::persisted_tool`
//! public boundary (introduced by T021) is required for Green.

use hive_runtime_core::persisted_tool::{
    PersistedTool, PersistedToolBuilder, PersistedToolError, PersistedToolKind,
    PersistedToolTarget, RequiredCapabilities,
};

#[test]
fn persisted_tool_function_wrap_target_is_a_function_identifier() {
    let tool = PersistedToolBuilder::new(PersistedToolKind::FunctionWrap)
        .target(PersistedToolTarget::function("fn.format_template"))
        .unwrap()
        .required_capabilities(RequiredCapabilities::empty())
        .unwrap()
        .build();
    assert_eq!(tool.target().as_str(), "fn.format_template");
    assert_eq!(tool.target().kind(), PersistedToolKind::FunctionWrap);
}

#[test]
fn persisted_tool_workflow_wrap_target_is_a_workflow_identifier() {
    let tool = PersistedToolBuilder::new(PersistedToolKind::WorkflowWrap)
        .target(PersistedToolTarget::workflow("wf.support_triage"))
        .unwrap()
        .required_capabilities(RequiredCapabilities::empty())
        .unwrap()
        .build();
    assert_eq!(tool.target().as_str(), "wf.support_triage");
    assert_eq!(tool.target().kind(), PersistedToolKind::WorkflowWrap);
}

#[test]
fn persisted_tool_xor_violation_is_stable() {
    // function-wrap + workflow target is rejected.
    let err = PersistedToolBuilder::new(PersistedToolKind::FunctionWrap)
        .target(PersistedToolTarget::workflow("wf.support_triage"))
        .expect_err("XOR violation is rejected at builder time");
    assert!(matches!(err, PersistedToolError::TargetKindMismatch { .. }));

    // workflow-wrap + function target is rejected.
    let err = PersistedToolBuilder::new(PersistedToolKind::WorkflowWrap)
        .target(PersistedToolTarget::function("fn.format_template"))
        .expect_err("XOR violation is rejected at builder time");
    assert!(matches!(err, PersistedToolError::TargetKindMismatch { .. }));
}

#[test]
fn persisted_tool_rejects_empty_target() {
    // An empty target string is not a valid identifier and MUST be
    // rejected at builder time, not silently accepted.
    let err = PersistedToolBuilder::new(PersistedToolKind::FunctionWrap)
        .target(PersistedToolTarget::function(""))
        .expect_err("empty target is rejected");
    assert!(matches!(err, PersistedToolError::EmptyTarget));
}

#[test]
fn persisted_tool_rejects_invalid_identifier_characters() {
    // Identifiers follow the stable convention:
    //   `<namespace>.<short_slug>` with a single `.` separator and only
    //   `[a-z0-9_]` characters. Anything else MUST be rejected. The
    //   empty case has its own dedicated test (`persisted_tool_rejects_empty_target`)
    //   and maps to [`PersistedToolError::EmptyTarget`]; here we only
    //   cover the non-empty "invalid characters" rejections so this
    //   test does not contradict the dedicated empty-target contract.
    for bad in &[
        "FormatTemplate", // uppercase
        "fn..format",     // empty slug
        "fn/format",      // slash
        "fn format",      // space
        "../fn.format",   // path traversal
        "fn.format.sub",  // extra dots
    ] {
        let err = PersistedToolBuilder::new(PersistedToolKind::FunctionWrap)
            .target(PersistedToolTarget::function(bad))
            .err()
            .unwrap_or_else(|| panic!("expected rejection for {bad:?}"));
        assert!(matches!(err, PersistedToolError::InvalidIdentifier { .. }));
    }
}

#[test]
fn persisted_tool_serialised_form_is_self_contained() {
    // The serialised form is the contract surface. It MUST be
    // parseable by `from_bytes` without any other crate / database
    // / network context, mirroring §T016B's "可独立序列化 roundtrip".
    let tool = PersistedToolBuilder::new(PersistedToolKind::FunctionWrap)
        .target(PersistedToolTarget::function("fn.format_template"))
        .unwrap()
        .required_capabilities(RequiredCapabilities::empty())
        .unwrap()
        .build();
    let bytes = tool.to_bytes().expect("serialise");
    let restored = PersistedTool::from_bytes(&bytes).expect("parse");
    assert_eq!(restored.to_bytes().expect("re-serialise"), bytes);
}

#[test]
fn persisted_tool_serialised_form_preserves_declared_order() {
    // Capability declaration order is part of the persisted contract.
    use hive_runtime_core::capability::CapabilityId;
    let mut a = RequiredCapabilities::empty();
    a.insert(CapabilityId::new("net.http").expect("valid"));
    a.insert(CapabilityId::new("fs.read").expect("valid"));

    let mut b = RequiredCapabilities::empty();
    b.insert(CapabilityId::new("fs.read").expect("valid"));
    b.insert(CapabilityId::new("net.http").expect("valid"));

    let tool_a = PersistedToolBuilder::new(PersistedToolKind::WorkflowWrap)
        .target(PersistedToolTarget::workflow("wf.support_triage"))
        .unwrap()
        .required_capabilities(a)
        .unwrap()
        .build();
    let tool_b = PersistedToolBuilder::new(PersistedToolKind::WorkflowWrap)
        .target(PersistedToolTarget::workflow("wf.support_triage"))
        .unwrap()
        .required_capabilities(b)
        .unwrap()
        .build();
    assert_ne!(tool_a.to_bytes().expect("a"), tool_b.to_bytes().expect("b"));
}

#[test]
fn persisted_tool_preserves_declared_capability_order() {
    use hive_runtime_core::capability::CapabilityId;

    let declared = ["network.http", "fs.read", "log.emit"];
    let mut capabilities = RequiredCapabilities::empty();
    for capability in declared {
        assert!(
            capabilities.insert(CapabilityId::new(capability).expect("valid Capability")),
            "fixture capabilities are unique"
        );
    }

    let tool = PersistedToolBuilder::new(PersistedToolKind::FunctionWrap)
        .target(PersistedToolTarget::function("fn.fetch_url"))
        .expect("matching target")
        .required_capabilities(capabilities)
        .expect("ordered unique capabilities")
        .build();
    let restored = PersistedTool::from_bytes(&tool.to_bytes().expect("serialize"))
        .expect("roundtrip persisted Tool");
    let actual = restored
        .required_capabilities()
        .iter()
        .map(|capability| capability.as_str())
        .collect::<Vec<_>>();

    assert_eq!(
        actual, declared,
        "PersistedTool required_capabilities must preserve the caller's stable input order"
    );
}

#[test]
fn persisted_tool_rejects_duplicate_and_unknown_capabilities_at_construction() {
    use hive_runtime_core::capability::{CapabilityId, CapabilitySet};

    let log_emit = CapabilityId::new("log.emit").expect("valid Capability");
    let mut known = CapabilitySet::empty();
    known.insert(log_emit.clone());

    let duplicate =
        RequiredCapabilities::from_ordered(vec![log_emit.clone(), log_emit.clone()], &known)
            .expect_err("duplicate Capability must fail");
    assert!(matches!(
        duplicate,
        PersistedToolError::DuplicateCapability { .. }
    ));

    let unknown = RequiredCapabilities::from_ordered(
        vec![CapabilityId::new("network.http").expect("valid but unavailable Capability")],
        &known,
    )
    .expect_err("unknown Capability must fail");
    assert!(matches!(
        unknown,
        PersistedToolError::UnknownCapability { .. }
    ));
}
