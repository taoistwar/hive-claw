//! WASM structural validation for Plugin upload (FR-005).
//!
//! HiveWeb delegates the complete import/export shape contract to the shared
//! `hive-runtime-core::wasm` validator so both hosts reject the same artifact
//! categories before instantiation.

use crate::utils::error::AppError;
use hive_runtime_core::wasm::{WasmValidationError, validate_wasm_shape};

/// Validate a Plugin artifact against the shared, WASI-denied WASM shape.
///
/// This upload boundary checks the Extism host fingerprint, host import
/// allowlist, WASI denial, and the `_hive_plugin_abi_version` function export.
/// The export's returned value is checked by the invoker after instantiation
/// and before any business export is called.
pub fn scan_wasm_imports(bytes: &[u8]) -> Result<(), AppError> {
    let validation = validate_wasm_shape(bytes, false);
    match validation.rejection_kind() {
        None => Ok(()),
        Some(kind) => Err(map_validation_error(kind, validation)),
    }
}

fn map_validation_error(
    kind: hive_runtime_core::wasm::WasmModuleShape,
    validation: WasmValidationError,
) -> AppError {
    AppError::BadRequest(format!("WASM 结构校验失败（{kind:?}）：{validation}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hive_runtime_core::wasm::ABI_VERSION_EXPORT;

    fn wasm_with_import_and_abi_export(module: &str, name: &str) -> Vec<u8> {
        wat::parse_str(format!(
            r#"(module
                (import "{module}" "{name}" (func $host (param i64) (result i64)))
                (func (export "{ABI_VERSION_EXPORT}") (result i32)
                  i32.const 0))"#
        ))
        .expect("WAT must compile")
    }

    #[test]
    fn test_minimal_wasm_no_imports_is_rejected() {
        let wasm = wat::parse_str("(module)").expect("WAT must compile");
        let error = scan_wasm_imports(&wasm).expect_err("missing host surface must fail closed");
        assert!(error.to_string().contains("MissingExtismFingerprint"));
    }

    #[test]
    fn test_wasm_with_host_call_import_is_allowed() {
        let wasm = wasm_with_import_and_abi_export("extism:host/user", "host_call");
        assert!(scan_wasm_imports(&wasm).is_ok());
    }

    #[test]
    fn test_wasm_with_pdk_env_import_is_allowed() {
        let wasm = wasm_with_import_and_abi_export("extism:host/env", "input_offset");
        assert!(scan_wasm_imports(&wasm).is_ok());
    }

    #[test]
    fn test_wasm_without_abi_export_is_rejected() {
        let wasm = wat::parse_str(
            r#"(module
                (import "extism:host/user" "host_call"
                  (func $host_call (param i64) (result i64))))"#,
        )
        .expect("WAT must compile");

        let error = scan_wasm_imports(&wasm).expect_err("missing ABI export must fail closed");
        assert!(error.to_string().contains("MissingAbiVersionExport"));
    }

    #[test]
    fn test_wasm_with_unknown_user_host_function_is_rejected() {
        let wasm = wasm_with_import_and_abi_export("extism:host/user", "other_func");
        let error = scan_wasm_imports(&wasm).expect_err("unknown host function must be rejected");
        assert!(error.to_string().contains("UnknownHostCallImport"));
    }

    #[test]
    fn test_wasm_with_disallowed_host_import_is_rejected() {
        let wasm = wasm_with_import_and_abi_export("extism", "time.now");
        let error = scan_wasm_imports(&wasm).expect_err("disallowed host import must be rejected");
        assert!(error.to_string().contains("DisallowedHostImport"));
    }

    #[test]
    fn test_wasm_with_wasi_import_is_rejected() {
        let wasm = wat::parse_str(format!(
            r#"(module
                (import "extism:host/user" "host_call"
                  (func $host_call (param i64) (result i64)))
                (import "wasi_snapshot_preview1" "random_get"
                  (func $random_get (param i32 i32) (result i32)))
                (func (export "{ABI_VERSION_EXPORT}") (result i32)
                  i32.const 0))"#
        ))
        .expect("WAT must compile");

        let error = scan_wasm_imports(&wasm).expect_err("WASI import must be rejected");
        assert!(error.to_string().contains("WasiImportPresent"));
    }
}
