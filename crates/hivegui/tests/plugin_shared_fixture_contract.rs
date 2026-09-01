use std::{path::Path, time::Duration};

use hive_runtime_core::{
    abi::HIVE_EXTISM_ABI_V1,
    wasm::{ABI_VERSION_EXPORT, WasmModuleShape, validate_wasm_shape},
};
use hivegui::runtime::PluginExecutor;

const SHARED_SMOKE_WASM: &[u8] = include_bytes!("fixtures/plugins/shared-smoke/plugin.wasm");
const SHARED_SMOKE_WASM_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/plugins/shared-smoke/plugin.wasm"
);

const HOST_CALL_WITHOUT_ABI_EXPORT_WAT: &str = r#"
    (module
      (import "extism:host/env" "input_offset"
        (func $input_offset (result i64)))
      (import "extism:host/env" "length"
        (func $length (param i64) (result i64)))
      (import "extism:host/env" "output_set"
        (func $output_set (param i64 i64)))
      (import "extism:host/user" "host_call"
        (func $host_call (param i64) (result i64)))

      (func (export "run") (result i32)
        (local $reply i64)
        (local.set $reply (call $host_call (call $input_offset)))
        (call $output_set
          (local.get $reply)
          (call $length (local.get $reply)))
        i32.const 0))
"#;

#[tokio::test]
async fn shared_smoke_fixture_exports_the_v1_version_through_hivegui() {
    let validation = validate_wasm_shape(SHARED_SMOKE_WASM, false);
    assert!(
        validation.is_ok(),
        "HiveGUI and HiveWeb must consume one structurally valid fixture: {validation}"
    );

    let version = PluginExecutor::execute_with_capabilities(
        Path::new(SHARED_SMOKE_WASM_PATH),
        ABI_VERSION_EXPORT,
        "{}",
        Duration::from_secs(5),
        Vec::new(),
    )
    .await
    .expect("HiveGUI must be able to query the shared fixture ABI version");
    assert_eq!(version, HIVE_EXTISM_ABI_V1);
}

#[tokio::test]
async fn shared_smoke_fixture_has_the_exact_hivegui_echo_reply() {
    let reply = PluginExecutor::execute_with_capabilities(
        Path::new(SHARED_SMOKE_WASM_PATH),
        "echo",
        r#""hello""#,
        Duration::from_secs(5),
        vec!["log.emit".to_owned()],
    )
    .await
    .expect("the shared smoke fixture must execute through HiveGUI");

    assert_eq!(reply, r#"{"echo":"hello","logged":true}"#);
}

#[tokio::test]
async fn hivegui_rejects_a_plugin_without_the_abi_export_before_target_execution() {
    let wasm = wat::parse_str(HOST_CALL_WITHOUT_ABI_EXPORT_WAT).expect("WAT must compile");
    assert_eq!(
        validate_wasm_shape(&wasm, false).rejection_kind(),
        Some(WasmModuleShape::MissingAbiVersionExport)
    );

    let temp_dir = tempfile::tempdir().expect("create fixture directory");
    let wasm_path = temp_dir.path().join("missing-abi-export.wasm");
    std::fs::write(&wasm_path, wasm).expect("write negative fixture");

    let error = PluginExecutor::execute_with_capabilities(
        &wasm_path,
        "run",
        r#"{"capability":"log.emit","args":{"level":"info","message":"fixture"}}"#,
        Duration::from_secs(1),
        vec!["log.emit".to_owned()],
    )
    .await
    .expect_err("shape validation must run before the requested export");
    assert!(
        error.contains("MissingAbiVersionExport") || error.contains("missing abi version export"),
        "the rejection must preserve the shared shape category: {error}"
    );
}

#[tokio::test]
async fn hivegui_rejects_an_unsupported_abi_version_before_business_execution() {
    let mut wasm = SHARED_SMOKE_WASM.to_vec();
    let expected = HIVE_EXTISM_ABI_V1.as_bytes();
    let unsupported = b"hive-extism/v2";
    assert_eq!(expected.len(), unsupported.len());

    let matches = wasm
        .windows(expected.len())
        .enumerate()
        .filter_map(|(offset, bytes)| (bytes == expected).then_some(offset))
        .collect::<Vec<_>>();
    assert_eq!(
        matches.len(),
        1,
        "the generated fixture must carry exactly one canonical ABI value"
    );
    wasm[matches[0]..matches[0] + unsupported.len()].copy_from_slice(unsupported);
    assert!(
        validate_wasm_shape(&wasm, false).is_ok(),
        "the negative fixture must remain structurally valid"
    );

    let temp_dir = tempfile::tempdir().expect("create fixture directory");
    let wasm_path = temp_dir.path().join("unsupported-abi-version.wasm");
    std::fs::write(&wasm_path, wasm).expect("write negative fixture");

    let error = PluginExecutor::execute_with_capabilities(
        &wasm_path,
        "echo",
        r#""hello""#,
        Duration::from_secs(5),
        vec!["log.emit".to_owned()],
    )
    .await
    .expect_err("ABI compatibility must be bound before the business export");
    assert!(
        error.contains("UnsupportedAbiVersion") || error.contains("unsupported abi version"),
        "the rejection must preserve the shared shape category: {error}"
    );
}
