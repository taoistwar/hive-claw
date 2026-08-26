//! Helpers for constructing small, structurally valid Extism v1 test Plugins.

/// Compile a WAT module with the canonical ABI-version export.
///
/// `extra_imports` is inserted before any function definitions; `functions`
/// contains the test-owned business exports. The compatibility probe writes
/// `hive-extism/v1` through Extism's stable memory API, matching the calling
/// convention produced by `extism_pdk::plugin_fn` without requiring a second
/// Rust/WASM build for every resource-limit probe.
pub fn compile_v1(extra_imports: &str, functions: &str) -> Vec<u8> {
    let source = format!(
        r#"
        (module
          (import "extism:host/env" "alloc"
            (func $abi_alloc (param i64) (result i64)))
          (import "extism:host/env" "store_u8"
            (func $abi_store_u8 (param i64 i32)))
          (import "extism:host/env" "output_set"
            (func $abi_output_set (param i64 i64)))
          {extra_imports}

          (func (export "_hive_plugin_abi_version") (result i32)
            (local $abi_ptr i64)
            (local.set $abi_ptr (call $abi_alloc (i64.const 14)))
            (call $abi_store_u8 (i64.add (local.get $abi_ptr) (i64.const 0)) (i32.const 104))
            (call $abi_store_u8 (i64.add (local.get $abi_ptr) (i64.const 1)) (i32.const 105))
            (call $abi_store_u8 (i64.add (local.get $abi_ptr) (i64.const 2)) (i32.const 118))
            (call $abi_store_u8 (i64.add (local.get $abi_ptr) (i64.const 3)) (i32.const 101))
            (call $abi_store_u8 (i64.add (local.get $abi_ptr) (i64.const 4)) (i32.const 45))
            (call $abi_store_u8 (i64.add (local.get $abi_ptr) (i64.const 5)) (i32.const 101))
            (call $abi_store_u8 (i64.add (local.get $abi_ptr) (i64.const 6)) (i32.const 120))
            (call $abi_store_u8 (i64.add (local.get $abi_ptr) (i64.const 7)) (i32.const 116))
            (call $abi_store_u8 (i64.add (local.get $abi_ptr) (i64.const 8)) (i32.const 105))
            (call $abi_store_u8 (i64.add (local.get $abi_ptr) (i64.const 9)) (i32.const 115))
            (call $abi_store_u8 (i64.add (local.get $abi_ptr) (i64.const 10)) (i32.const 109))
            (call $abi_store_u8 (i64.add (local.get $abi_ptr) (i64.const 11)) (i32.const 47))
            (call $abi_store_u8 (i64.add (local.get $abi_ptr) (i64.const 12)) (i32.const 118))
            (call $abi_store_u8 (i64.add (local.get $abi_ptr) (i64.const 13)) (i32.const 49))
            (call $abi_output_set (local.get $abi_ptr) (i64.const 14))
            i32.const 0)

          {functions}
        )
        "#
    );

    wat::parse_str(source).expect("compile Extism v1 WAT fixture")
}
