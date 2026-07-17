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
