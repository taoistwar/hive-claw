use anyhow::{Result, bail};
use hive_runtime_core::wasm::ABI_VERSION_EXPORT;
use wasmparser::{ExternalKind, Parser, Payload};

/// Returns the names of all function exports declared by a WASM module.
pub fn extract_wasm_exports(wasm_bytes: &[u8]) -> Result<Vec<String>> {
    let mut exports = Vec::new();

    for payload in Parser::new(0).parse_all(wasm_bytes) {
        if let Payload::ExportSection(reader) = payload? {
            for export in reader {
                let export = export?;
                if export.kind == ExternalKind::Func && export.name != ABI_VERSION_EXPORT {
                    exports.push(export.name.to_string());
                }
            }
            break;
        }
    }

    if exports.is_empty() {
        bail!("WASM 模块没有导出的函数");
    }

    Ok(exports)
}
