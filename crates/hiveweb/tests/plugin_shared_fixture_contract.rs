use hive_runtime_core::wasm::{WasmModuleShape, validate_wasm_shape};
use hiveweb::runtime::scan_wasm_imports;

const SHARED_SMOKE_WASM: &[u8] =
    include_bytes!("../../hivegui/tests/fixtures/plugins/shared-smoke/plugin.wasm");

const HOST_CALL_WITHOUT_ABI_EXPORT_WAT: &str = r#"
    (module
      (import "extism:host/user" "host_call"
        (func $host_call (param i64) (result i64)))
      (func (export "run") (result i32)
        i32.const 0))
"#;

#[test]
fn hiveweb_accepts_the_same_structurally_valid_smoke_fixture_as_hivegui() {
    let validation = validate_wasm_shape(SHARED_SMOKE_WASM, false);
    assert!(
        validation.is_ok(),
        "both hosts must consume one structurally valid fixture: {validation}"
    );
    scan_wasm_imports(SHARED_SMOKE_WASM)
        .expect("HiveWeb upload pre-validation must accept the shared fixture");
}

#[test]
fn hiveweb_upload_validation_rejects_a_missing_abi_export() {
    let wasm = wat::parse_str(HOST_CALL_WITHOUT_ABI_EXPORT_WAT).expect("WAT must compile");
    assert_eq!(
        validate_wasm_shape(&wasm, false).rejection_kind(),
        Some(WasmModuleShape::MissingAbiVersionExport)
    );

    let error = scan_wasm_imports(&wasm)
        .expect_err("HiveWeb must enforce the complete shared shape contract");
    assert!(
        error.to_string().contains("MissingAbiVersionExport")
            || error.to_string().contains("missing abi version export"),
        "the rejection must preserve the shared shape category: {error}"
    );
}
