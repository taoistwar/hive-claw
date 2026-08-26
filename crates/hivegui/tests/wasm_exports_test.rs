use std::collections::BTreeSet;

use hive_runtime_core::wasm::ABI_VERSION_EXPORT;
use hivegui::datasource::wasm_exports::extract_wasm_exports;

#[test]
fn extracts_only_exported_wasm_functions() {
    let wasm = [
        0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, // magic + version
        0x01, 0x04, 0x01, 0x60, 0x00, 0x00, // type: () -> ()
        0x03, 0x02, 0x01, 0x00, // one function using type 0
        0x07, 0x0a, 0x01, 0x06, b'l', b'o', b'o', b'k', b'u', b'p', 0x00, 0x00, 0x0a, 0x04, 0x01,
        0x02, 0x00, 0x0b, // empty function body
    ];

    assert_eq!(extract_wasm_exports(&wasm).unwrap(), vec!["lookup"]);
}

#[test]
fn rejects_modules_without_exported_functions() {
    let wasm = [
        0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, // magic + version
    ];

    assert!(extract_wasm_exports(&wasm).is_err());
}

#[test]
fn shared_fixture_hides_the_reserved_abi_export_from_business_functions() {
    let wasm = include_bytes!("fixtures/plugins/shared-smoke/plugin.wasm");
    let exports = extract_wasm_exports(wasm).expect("shared fixture exports");
    let export_set = exports.iter().map(String::as_str).collect::<BTreeSet<_>>();

    assert_eq!(
        export_set,
        ["echo", "fs_roundtrip", "full_demo", "http_get", "ping"]
            .into_iter()
            .collect()
    );
    assert!(!exports.iter().any(|name| name == ABI_VERSION_EXPORT));
}
