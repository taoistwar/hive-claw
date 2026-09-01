use std::collections::BTreeSet;

use hive_runtime_core::{
    abi::{HIVE_EXTISM_ABI_V1, HostCallReply, HostCallRequest, StableErrorKind},
    plugin::PluginManifestV1,
};

fn manifest_issue_codes(bytes: &[u8]) -> BTreeSet<String> {
    PluginManifestV1::parse_and_validate(bytes, &BTreeSet::new())
        .expect_err("malformed manifest must be rejected")
        .issues()
        .iter()
        .map(|issue| issue.code().to_owned())
        .collect()
}

#[test]
fn manifest_accepts_the_exact_v1_contract_and_normalizes_capabilities() {
    let host_capabilities = BTreeSet::from(["network.http".to_owned()]);
    let manifest = PluginManifestV1::parse_and_validate(
        br#"{
            "abi_version": "hive-extism/v1",
            "required_capabilities": [" network.http "],
            "exports": [{"name":"lookup","input":"json","output":"json"}],
            "future_metadata": {"preserved": true}
        }"#,
        &host_capabilities,
    )
    .expect("the documented v1 manifest must be compatible");

    assert_eq!(HIVE_EXTISM_ABI_V1, "hive-extism/v1");
    assert_eq!(manifest.abi_version(), HIVE_EXTISM_ABI_V1);
    assert_eq!(manifest.required_capabilities(), ["network.http"]);
    assert_eq!(manifest.exports().len(), 1);
    assert_eq!(manifest.exports()[0].name(), "lookup");
    assert_eq!(manifest.exports()[0].input(), "json");
    assert_eq!(manifest.exports()[0].output(), "json");

    let reencoded = String::from_utf8(manifest.encode_json())
        .expect("a validated manifest must encode as UTF-8 JSON");
    assert!(
        reencoded.contains("\"future_metadata\""),
        "unknown manifest fields must be retained when the v1 manifest is encoded again"
    );
    assert!(
        reencoded.contains("\"preserved\":true"),
        "unknown manifest field values must survive re-encoding"
    );

    PluginManifestV1::parse_and_validate(reencoded.as_bytes(), &host_capabilities)
        .expect("the re-encoded manifest must remain v1-compatible");
}

#[test]
fn manifest_reports_all_compatibility_issues_before_import() {
    let host_capabilities = BTreeSet::from(["network.http".to_owned()]);
    let error = PluginManifestV1::parse_and_validate(
        br#"{
            "abi_version": "hive-extism/v2",
            "required_capabilities": [
                "network.http",
                " network.http ",
                "   ",
                "storage.read"
            ],
            "exports": [{"name":"lookup","input":"json","output":"json"}]
        }"#,
        &host_capabilities,
    )
    .expect_err("an incompatible manifest must be rejected before file or database writes");

    let issue_codes = error
        .issues()
        .iter()
        .map(|issue| issue.code())
        .collect::<BTreeSet<_>>();

    assert_eq!(
        issue_codes,
        BTreeSet::from([
            "unsupported_abi",
            "capability_duplicate",
            "capability_blank",
            "capability_unavailable",
        ]),
        "validation must accumulate every independently detectable compatibility issue"
    );
}

#[test]
fn manifest_rejects_malformed_json_wrong_field_types_and_non_object_roots() {
    for (manifest, expected_issue) in [
        (br#"{"#.as_slice(), "manifest_json_invalid"),
        (br#"[]"#.as_slice(), "manifest_not_object"),
        (
            br#"{
                "abi_version": 1,
                "required_capabilities": [],
                "exports": [{"name":"lookup","input":"json","output":"json"}]
            }"#
            .as_slice(),
            "abi_version_type",
        ),
        (
            br#"{
                "abi_version": "hive-extism/v1",
                "required_capabilities": "network.http",
                "exports": [{"name":"lookup","input":"json","output":"json"}]
            }"#
            .as_slice(),
            "required_capabilities_type",
        ),
        (
            br#"{
                "abi_version": "hive-extism/v1",
                "required_capabilities": [7],
                "exports": [{"name":"lookup","input":"json","output":"json"}]
            }"#
            .as_slice(),
            "capability_type",
        ),
        (
            br#"{
                "abi_version": "hive-extism/v1",
                "required_capabilities": [],
                "exports": {"name":"lookup","input":"json","output":"json"}
            }"#
            .as_slice(),
            "exports_type",
        ),
    ] {
        assert_eq!(
            manifest_issue_codes(manifest),
            BTreeSet::from([expected_issue.to_owned()]),
            "manifest structural failure must have one stable, actionable issue"
        );
    }
}

#[test]
fn manifest_rejects_invalid_export_shapes_encodings_and_duplicate_names() {
    for (exports, expected_issue) in [
        (r#"[]"#, "exports_empty"),
        (r#"[7]"#, "export_type"),
        (
            r#"[{"name":7,"input":"json","output":"json"}]"#,
            "export_name_type",
        ),
        (
            r#"[{"name":"   ","input":"json","output":"json"}]"#,
            "export_name_blank",
        ),
        (
            r#"[{"name":"lookup","input":{},"output":"json"}]"#,
            "export_input_type",
        ),
        (
            r#"[{"name":"lookup","input":"bytes","output":"json"}]"#,
            "export_input_unsupported",
        ),
        (
            r#"[{"name":"lookup","input":"json","output":false}]"#,
            "export_output_type",
        ),
        (
            r#"[{"name":"lookup","input":"json","output":"bytes"}]"#,
            "export_output_unsupported",
        ),
        (
            r#"[
                {"name":"lookup","input":"json","output":"json"},
                {"name":" lookup ","input":"json","output":"json"}
            ]"#,
            "export_duplicate",
        ),
    ] {
        let manifest = format!(
            r#"{{
                "abi_version": "hive-extism/v1",
                "required_capabilities": [],
                "exports": {exports}
            }}"#
        );
        assert_eq!(
            manifest_issue_codes(manifest.as_bytes()),
            BTreeSet::from([expected_issue.to_owned()]),
            "invalid export schema must be rejected before artifact or database writes"
        );
    }
}

#[test]
fn host_call_request_and_reply_use_the_v1_wire_envelopes() {
    let request = HostCallRequest::decode(br#"{"capability":"network.http","args":{}}"#)
        .expect("decode the documented host_call request");
    assert_eq!(request.capability(), "network.http");
    assert_eq!(request.args_json(), b"{}");

    let success = HostCallReply::success_json(br#"{"status":200}"#)
        .expect("accept valid JSON data")
        .encode_json();
    assert_eq!(
        success, br#"{"ok":true,"data":{"status":200}}"#,
        "success replies must contain ok=true and data"
    );

    let failure = HostCallReply::failure(
        StableErrorKind::CapabilityDenied,
        "capability is not granted to this Agent",
    )
    .encode_json();
    assert_eq!(
        failure,
        br#"{"ok":false,"code":4030,"message":"capability is not granted to this Agent"}"#
    );
    assert!(
        !failure
            .windows(b"\"data\"".len())
            .any(|part| part == b"\"data\""),
        "failure replies must never carry data"
    );
}

#[test]
fn stable_error_kinds_preserve_host_codes_and_placeholder_rejection() {
    let host_error_codes = [
        (StableErrorKind::InvalidArgs, "invalid_args", 4001),
        (StableErrorKind::CapabilityDenied, "capability_denied", 4030),
        (
            StableErrorKind::CapabilityUnknown,
            "capability_unknown",
            4045,
        ),
        (
            StableErrorKind::CapabilityTimeout,
            "capability_timeout",
            4081,
        ),
        (StableErrorKind::Cancelled, "cancelled", 4990),
        (StableErrorKind::Internal, "internal", 5000),
        (StableErrorKind::PluginTimeout, "plugin_timeout", 5004),
        (
            StableErrorKind::PluginMemoryLimit,
            "plugin_memory_limit",
            5011,
        ),
        (
            StableErrorKind::PluginOutputLimit,
            "plugin_output_limit",
            5012,
        ),
        (StableErrorKind::WasiDenied, "wasi_denied", 5013),
    ];

    for (kind, stable_name, numeric_code) in host_error_codes {
        assert_eq!(kind.as_str(), stable_name);
        assert_eq!(kind.host_call_code(), Some(numeric_code));
    }

    assert_eq!(
        StableErrorKind::FunctionNotExecutable.as_str(),
        "function_not_executable"
    );
    assert_eq!(
        StableErrorKind::FunctionNotExecutable.host_call_code(),
        None,
        "Placeholder rejection is a stable runtime error and must happen before host_call dispatch"
    );
}
