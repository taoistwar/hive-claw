//! WASM export extraction utility (T092 extension)
//!
//! Parses a WASM binary and extracts the names of all exported functions.
//! Used to populate the `plugin_export` dropdown in the Function creation form.

use anyhow::{Result, bail};
use wasmparser::{Parser, Payload};

/// Extract all exported function names from a WASM binary.
///
/// Returns a `Vec<String>` of export names. Only function exports are included;
/// memory, table, and global exports are ignored.
pub fn extract_wasm_exports(wasm_bytes: &[u8]) -> Result<Vec<String>> {
    let parser = Parser::new(0);
    let mut exports = Vec::new();

    for payload in parser.parse_all(wasm_bytes) {
        let payload = payload?;
        if let Payload::ExportSection(export_reader) = payload {
            for exp in export_reader {
                let exp = exp?;
                // Only include function exports (kind == 0)
                if exp.kind == wasmparser::ExternalKind::Func {
                    exports.push(exp.name.to_string());
                }
            }
            break; // We only need the export section
        }
    }

    if exports.is_empty() {
        bail!("WASM binary contains no exported functions");
    }

    Ok(exports)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_exports_from_weather_plugin() {
        // Read the compiled weather plugin WASM
        let wasm_bytes = std::fs::read(
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../examples/plugins/weather/target/wasm32-unknown-unknown/release/weather_plugin.wasm"
            ),
        )
        .expect("weather_plugin.wasm not found; run: cd examples/plugins/weather && cargo build --target wasm32-unknown-unknown --release");

        let exports = extract_wasm_exports(&wasm_bytes).unwrap();
        // The weather plugin exports: lookup, config_get, _start, __extism_* etc.
        assert!(exports.iter().any(|e| e == "lookup"));
    }
}
