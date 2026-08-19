//! T025R ③ / T082 [US8] Plugin sandbox Red→Green contract.
//!
//! Source of truth: `specs/011-hivegui-standalone-mode/tasks.md` §T025R
//! 边界 ③ + `checklists/security.md` §③.6 5 步修复要求。
//!
//! Public boundaries the T079 implementation provides:
//!   - `hivegui::runtime::plugin_executor::PluginExecutor::execute`
//!   - `hivegui::runtime::plugin_executor::PluginExecutor::execute_with_capabilities`
//!
//! Red public boundaries this test must satisfy (T025R ③ 重新激活硬门禁):
//!   - `PluginExecutor::execute` must reject any WASI import
//!     (`wasi_snapshot_preview1::*` / `wasi::*`), regardless of which
//!     function the plugin tries to import. The instantiation itself
//!     must fail; the plugin must never be allowed to call any host
//!     function from the WASI surface.
//!   - Source-level: `plugin_executor.rs` must compile with
//!     `.with_wasi(false)`, never `.with_wasi(true)`.
//!   - Source-level: HiveGUI must keep Extism auto-registration
//!     (`http` / `register-http` / `register-filesystem`) disabled so
//!     plugins can never reach the host network or filesystem through
//!     Extism's pre-baked bindings.
//!   - The legitimate `host_call` host import (the only one the desktop
//!     executor registers) must still build and execute after the WASI
//!     lockdown.

#![allow(missing_docs)]

use std::time::Duration;

use hivegui::runtime::PluginExecutor;
use tempfile::TempDir;

fn write_wat(temp_dir: &TempDir, name: &str, source: &str) -> std::path::PathBuf {
    let path = temp_dir.path().join(name);
    let wasm = wat::parse_str(source).expect("compile WAT fixture");
    std::fs::write(&path, wasm).expect("write WASM fixture");
    path
}

#[tokio::test(flavor = "current_thread")]
async fn plugin_importing_wasi_fd_write_is_rejected() {
    let temp_dir = TempDir::new().expect("tempdir");
    let wasm = write_wat(
        &temp_dir,
        "wasi-fd-write.wasm",
        r#"
        (module
          (import "wasi_snapshot_preview1" "fd_write"
            (func $fd_write (param i32 i32 i32 i32) (result i32)))
          (func (export "evil") (result i32)
            i32.const 0
          )
        )
        "#,
    );

    let result = PluginExecutor::execute(&wasm, "evil", "{}").await;
    let error =
        result.expect_err("plugin importing wasi_snapshot_preview1::fd_write must be rejected");
    // Extism / Wasmtime surfaces the missing import at build time.
    assert!(
        error.to_lowercase().contains("import")
            || error.to_lowercase().contains("wasi")
            || error.contains("构建插件失败"),
        "rejection message should mention the missing WASI import, got: {error}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn plugin_importing_wasi_path_open_is_rejected() {
    let temp_dir = TempDir::new().expect("tempdir");
    let wasm = write_wat(
        &temp_dir,
        "wasi-path-open.wasm",
        r#"
        (module
          (import "wasi_snapshot_preview1" "path_open"
            (func $path_open
              (param i32 i32 i32 i32 i32 i64 i64 i32 i32) (result i32)))
          (func (export "escape") (result i32)
            i32.const 0
          )
        )
        "#,
    );

    let result =
        PluginExecutor::execute_with_timeout(&wasm, "escape", "{}", Duration::from_secs(1)).await;
    let error =
        result.expect_err("plugin importing wasi_snapshot_preview1::path_open must be rejected");
    assert!(
        error.to_lowercase().contains("import")
            || error.to_lowercase().contains("wasi")
            || error.contains("构建插件失败"),
        "rejection message should mention the missing WASI import, got: {error}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn plugin_importing_wasi_proc_exit_is_rejected() {
    let temp_dir = TempDir::new().expect("tempdir");
    let wasm = write_wat(
        &temp_dir,
        "wasi-proc-exit.wasm",
        r#"
        (module
          (import "wasi_snapshot_preview1" "proc_exit"
            (func $proc_exit (param i32)))
          (func (export "panic") (result i32)
            i32.const 0
          )
        )
        "#,
    );

    let result = PluginExecutor::execute_with_capabilities(
        &wasm,
        "panic",
        "{}",
        Duration::from_secs(1),
        Vec::new(),
    )
    .await;
    let error =
        result.expect_err("plugin importing wasi_snapshot_preview1::proc_exit must be rejected");
    assert!(
        error.to_lowercase().contains("import")
            || error.to_lowercase().contains("wasi")
            || error.contains("构建插件失败"),
        "rejection message should mention the missing WASI import, got: {error}"
    );
}

#[test]
fn plugin_executor_disables_wasi_in_source() {
    let source = include_str!("../src/runtime/plugin_executor.rs");
    assert!(
        source.contains(".with_wasi(false)"),
        "plugin_executor.rs must call .with_wasi(false) on the PluginBuilder"
    );
    assert!(
        !source.contains(".with_wasi(true)"),
        "plugin_executor.rs must not call .with_wasi(true); \
         WASI is part of the T025R ③ sandbox and must remain disabled"
    );
}

#[test]
fn plugin_executor_enforces_memory_fuel_and_output_limits() {
    // T079 资源限制必须接线（与 HiveWeb 一致）：
    //   - `with_memory_max(64KiB pages)` 由 MiB 换算，不得直接传 MiB/字节
    //   - `with_fuel_limit(DEFAULT_FUEL)` 指令预算
    //   - output 长度检查（Extism 无内置 output 上限，host 层兜底）
    let source = include_str!("../src/runtime/plugin_executor.rs");
    assert!(
        source.contains(".with_memory_max("),
        "plugin_executor.rs must set the Extism memory cap via with_memory_max"
    );
    assert!(
        source.contains("memory_pages"),
        "plugin_executor.rs must convert MiB → 64 KiB pages before with_memory_max"
    );
    assert!(
        source.contains(".with_fuel_limit("),
        "plugin_executor.rs must set the Extism fuel budget via with_fuel_limit"
    );
    assert!(
        source.contains("DEFAULT_FUEL"),
        "plugin_executor.rs must apply the shared fuel budget default"
    );
    assert!(
        source.contains("output.len() as u64 > output_bytes"),
        "plugin_executor.rs must enforce the output cap after the call"
    );
}

#[test]
fn plugin_executor_keeps_only_host_call_as_a_host_import() {
    // The executor must register exactly one host import — `host_call` —
    // and must not auto-register any other extism:host/* functions.
    let source = include_str!("../src/runtime/plugin_executor.rs");
    let with_function_count = source.matches(".with_function(").count();
    assert_eq!(
        with_function_count, 1,
        "plugin_executor.rs must register exactly one host function in total"
    );
    // The single host import is `host_call`, sourced from the shared
    // `hive-runtime-core::wasm::HOST_CALL_IMPORT` constant (which resolves
    // to `"host_call"`). The identifier may be referenced via the constant,
    // so we assert the shared constant is imported and used once, rather
    // than a raw string literal.
    assert!(
        source.contains("HOST_CALL_IMPORT"),
        "plugin_executor.rs must register the shared `host_call` host function"
    );
    assert!(
        source.contains("use hive_runtime_core::wasm::HOST_CALL_IMPORT"),
        "plugin_executor.rs must source `host_call` from the shared wasm contract"
    );
}

#[test]
fn hivegui_cargo_manifest_keeps_extism_auto_registration_disabled() {
    // The extism crate's auto-registered host functions (network + filesystem)
    // are gated behind the `http`, `register-http` and `register-filesystem`
    // features. HiveGUI resolves extism through the workspace, so the
    // authoritative declaration lives in the workspace `Cargo.toml` and
    // must keep `default-features = false` plus all three auto-registration
    // features turned off.
    let workspace_manifest = include_str!("../../../Cargo.toml");
    let extism_line = workspace_manifest
        .lines()
        .find(|line| line.trim_start().starts_with("extism =") && line.contains("workspace"))
        .or_else(|| {
            // Fallback to the in-workspace declaration if HiveGUI ever
            // inlines extism in `crates/hivegui/Cargo.toml` directly.
            workspace_manifest
                .lines()
                .find(|line| line.trim_start().starts_with("extism ="))
        })
        .expect("workspace Cargo.toml must declare extism");
    assert!(
        extism_line.contains("default-features = false")
            || extism_line.contains("default-features=false"),
        "extism must be declared with `default-features = false` in the workspace Cargo.toml, got: {extism_line}"
    );
    assert!(
        !extism_line.contains("\"http\""),
        "hivegui must not enable the Extism `http` feature, got: {extism_line}"
    );
    assert!(
        !extism_line.contains("\"register-http\""),
        "hivegui must not enable the Extism `register-http` feature, got: {extism_line}"
    );
    assert!(
        !extism_line.contains("\"register-filesystem\""),
        "hivegui must not enable the Extism `register-filesystem` feature, got: {extism_line}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn plugin_with_only_host_call_can_still_be_built_after_wasi_lockdown() {
    // Companion test to `desktop_host_call::plugin_importing_host_call_can_be_built_by_the_desktop_executor`.
    // After the T025R ③ lockdown, the legitimate `host_call` import must
    // still build; only WASI imports are forbidden.
    let temp_dir = TempDir::new().expect("tempdir");
    let wasm = write_wat(
        &temp_dir,
        "host-call-only.wasm",
        r#"
        (module
          (type $host-call-type (func (param i64) (result i64)))
          (import "extism:host/user" "host_call" (func $host_call (type $host-call-type)))
          (func (export "noop") (result i32)
            i32.const 0
          )
        )
        "#,
    );

    let result = PluginExecutor::execute_with_capabilities(
        &wasm,
        "noop",
        "{}",
        Duration::from_secs(1),
        Vec::new(),
    )
    .await;

    if let Err(error) = result {
        assert!(
            !error.contains("host_call has not been defined") && !error.contains("unknown import"),
            "host_call ABI must still be registered after WASI lockdown: {error}"
        );
    }
}
