//! WASM binary parser: scan imports section (FR-005)
//!
//! Parses the WASM binary format to extract import names, used during
//! plugin upload to verify the import surface against the shared
//! `hive-runtime-core::wasm` structural contract (T080 / §4 隔离):
//! exactly one user host function (`host_call`), no WASI, no stray host.

use crate::utils::error::AppError;
use hive_runtime_core::wasm::{EXTISM_HOST_CALL_MODULE, HOST_CALL_IMPORT, WasmModuleShape};

/// WASI 的 `wasi_snapshot_preview1` module 名。能力-only Plugin 一律拒绝。
/// 该拒绝在共享 `hive-runtime-core::wasm` 契约中对应
/// [`WasmModuleShape::WasiImportPresent`]（§4 隔离：PluginBuilder 必须
/// `with_wasi(false)`，Plugin 不得直接触碰文件系统或网络）。
const WASI_MODULE: &str = "wasi_snapshot_preview1";

/// Extism 宿主挂载唯一用户 host function 的完整 import 路径：
/// `extism:host/user.host_call`（`with_function(HOST_CALL_IMPORT, …)` 固定
/// 挂到 `EXTISM_HOST_CALL_MODULE` 命名空间）。
fn extism_host_call_import() -> String {
    format!("{EXTISM_HOST_CALL_MODULE}.{HOST_CALL_IMPORT}")
}

/// 扫描 WASM 的 imports 段，对照共享结构契约验证 import surface。
///
/// 允许的 import 仅两类：
/// 1. `extism:host/user.host_call` —— 唯一用户 host function（宿主仅注册
///    [`HOST_CALL_IMPORT`]）；其余 `extism:host/user.*` 视为
///    [`WasmModuleShape::UnknownHostCallImport`]。
/// 2. `extism:host/env.*` —— Extism PDK 基础设施（input_offset/length/
///    output_set 等），随 SDK 固定，不暴露业务能力。
///
/// 拒绝：`wasi_snapshot_preview1`（[`WasmModuleShape::WasiImportPresent`]）、
/// 任何其它 host import（[`WasmModuleShape::DisallowedHostImport`]）。
///
/// 注意：此签名不再接收 capability 白名单——capability 通过 `host_call`
/// JSON envelope 传递，而非 WASM import 名；旧的白名单模型对合法 Extism
/// 插件从不命中（所有 import 都在 `extism:host/` 下），属空转校验。
///
/// WASM binary format:
/// - magic: \0asm (4 bytes)
/// - version: 1 (4 bytes)  
/// - sections...
///
/// Import section id = 2
/// Each import: module_len + module + name_len + name + import_kind
pub fn scan_wasm_imports(bytes: &[u8]) -> Result<(), AppError> {
    // Skip magic + version (8 bytes)
    if bytes.len() < 8 {
        return Err(AppError::BadRequest("WASM 文件太小，无法解析".into()));
    }

    let mut pos = 8;
    let mut found_imports = Vec::new();

    while pos < bytes.len() {
        let section_id = bytes[pos];
        pos += 1;

        let (section_size, new_pos) = read_unsigned_leb128(bytes, pos)
            .ok_or_else(|| AppError::BadRequest("WASM 格式错误：无法读取 section size".into()))?;
        pos = new_pos;

        if section_id == 2 {
            // Import section
            let (num_imports, new_pos) = read_unsigned_leb128(bytes, pos).ok_or_else(|| {
                AppError::BadRequest("WASM 格式错误：无法读取 import count".into())
            })?;
            pos = new_pos;

            for _ in 0..num_imports {
                let (module, new_pos) = read_name(bytes, pos).ok_or_else(|| {
                    AppError::BadRequest("WASM 格式错误：无法读取 module name".into())
                })?;
                pos = new_pos;

                // §4 隔离：能力-only Plugin 必须 `with_wasi(false)`，任何
                // `wasi_snapshot_preview1` import 都在上传预校验阶段拒绝，
                // 与共享 `WasmModuleShape::WasiImportPresent` 分类对齐。
                if module == WASI_MODULE {
                    return Err(AppError::BadRequest(format!(
                        "WASM 包含被禁止的 WASI import（{}，对应共享契约 {:?}），能力-only Plugin 不得直接访问文件系统或网络",
                        WASI_MODULE,
                        WasmModuleShape::WasiImportPresent
                    )));
                }

                let (name, new_pos) = read_name(bytes, pos).ok_or_else(|| {
                    AppError::BadRequest("WASM 格式错误：无法读取 field name".into())
                })?;
                pos = new_pos;

                // Read and skip import descriptor
                if pos >= bytes.len() {
                    return Err(AppError::BadRequest(
                        "WASM 格式错误：import descriptor 越界".into(),
                    ));
                }
                let import_kind = bytes[pos];
                pos += 1;
                // Skip the rest of the import descriptor based on kind
                pos = skip_import_desc(bytes, pos, import_kind)?;

                let full_import = format!("{}.{}", module, name);
                found_imports.push(full_import);
            }
            break;
        } else {
            // Skip this section
            pos += section_size as usize;
        }
    }

    // 验证 import surface，对照共享 `WasmModuleShape` 结构契约：
    // 唯一用户 host function 是 `extism:host/user.host_call`；Extism PDK 的
    // `extism:host/env.*` 基础设施始终允许；其余一律拒绝。
    let host_call_import = extism_host_call_import();
    for import in &found_imports {
        if import == &host_call_import {
            continue; // 唯一用户 host function（宿主已注册 HOST_CALL_IMPORT）
        }
        if import.starts_with("extism:host/env.") {
            continue; // Extism PDK 基础设施（随 SDK 固定，不暴露业务能力）
        }
        if import.starts_with("extism:host/") {
            return Err(AppError::BadRequest(format!(
                "WASM 包含未知的用户 host function（{:?}）：宿主仅注册 {}",
                WasmModuleShape::UnknownHostCallImport,
                HOST_CALL_IMPORT
            )));
        }
        return Err(AppError::BadRequest(format!(
            "WASM 包含不允许的 host import（{:?}）",
            WasmModuleShape::DisallowedHostImport
        )));
    }

    Ok(())
}

/// 读取 LEB128 编码的无符号整数
fn read_unsigned_leb128(bytes: &[u8], mut pos: usize) -> Option<(u32, usize)> {
    let mut result: u32 = 0;
    let mut shift = 0;

    loop {
        if pos >= bytes.len() {
            return None;
        }
        let byte = bytes[pos];
        pos += 1;
        result |= ((byte & 0x7F) as u32) << shift;
        if byte & 0x80 == 0 {
            return Some((result, pos));
        }
        shift += 7;
        if shift >= 32 {
            return None;
        }
    }
}

/// 根据 import kind 跳过 import descriptor 的剩余字节
/// WASM MVP import descriptor 格式:
/// - 0x00 (func):   typeidx (LEB128 u32)
/// - 0x01 (table):  reftype (LEB128 s33) + limits
/// - 0x02 (memory): limits
/// - 0x03 (global): valtype (1 byte) + mut (1 byte)
fn skip_import_desc(bytes: &[u8], pos: usize, kind: u8) -> Result<usize, AppError> {
    let mkerr = |name: &str| {
        AppError::BadRequest(format!("WASM 格式错误：无法读取 import descriptor {name}"))
    };

    match kind {
        0x00 => {
            // Function import: skip typeidx (LEB128 u32)
            let (_, p) = read_unsigned_leb128(bytes, pos).ok_or_else(|| mkerr("typeidx"))?;
            Ok(p)
        }
        0x01 => {
            // Table import: skip reftype (LEB128 s33) + limits
            let (_, p) = read_unsigned_leb128(bytes, pos).ok_or_else(|| mkerr("table reftype"))?;
            skip_limits(bytes, p)
        }
        0x02 => {
            // Memory import: skip limits
            skip_limits(bytes, pos)
        }
        0x03 => {
            // Global import: skip valtype (1 byte) + mut (1 byte)
            if pos + 2 > bytes.len() {
                return Err(mkerr("globaltype"));
            }
            Ok(pos + 2)
        }
        _ => {
            // Unknown import kind — cannot determine descriptor size.
            // This should not happen for valid WASM MVP modules.
            Err(AppError::BadRequest(format!(
                "WASM 格式错误：未知 import kind 0x{kind:02x}"
            )))
        }
    }
}

/// 跳过 WASM limits 结构: flags(1 byte) + min(LEB128) + max(LEB128, 如果 has_max)
fn skip_limits(bytes: &[u8], pos: usize) -> Result<usize, AppError> {
    if pos >= bytes.len() {
        return Err(AppError::BadRequest("WASM 格式错误：limits 越界".into()));
    }
    let flags = bytes[pos];
    let (_, p) = read_unsigned_leb128(bytes, pos + 1)
        .ok_or_else(|| AppError::BadRequest("WASM 格式错误：无法读取 limits min".into()))?;
    if flags & 0x01 != 0 {
        // has max
        let (_, p2) = read_unsigned_leb128(bytes, p)
            .ok_or_else(|| AppError::BadRequest("WASM 格式错误：无法读取 limits max".into()))?;
        return Ok(p2);
    }
    Ok(p)
}

/// 读取 WASM name 字段（length-prefixed UTF-8）
fn read_name(bytes: &[u8], pos: usize) -> Option<(String, usize)> {
    let (len, new_pos) = read_unsigned_leb128(bytes, pos)?;
    let end = new_pos + len as usize;
    if end > bytes.len() {
        return None;
    }
    let name = String::from_utf8(bytes[new_pos..end].to_vec()).ok()?;
    Some((name, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_minimal_wasm_no_imports() {
        let wasm = vec![
            0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x0a, 0x00,
        ];
        assert!(scan_wasm_imports(&wasm).is_ok());
    }

    fn write_leb128_u32(value: u32) -> Vec<u8> {
        let mut v = value;
        let mut buf = Vec::new();
        loop {
            let mut byte = (v & 0x7F) as u8;
            v >>= 7;
            if v != 0 {
                byte |= 0x80;
            }
            buf.push(byte);
            if v == 0 {
                break;
            }
        }
        buf
    }

    /// Build a minimal WASM binary with a single function import of
    /// `(module, name)`.
    fn wasm_with_import(module: &str, name: &str) -> Vec<u8> {
        let mut wasm = vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x02];

        let mut import_data = Vec::new();
        import_data.push(1); // num_imports
        import_data.push(module.len() as u8);
        import_data.extend_from_slice(module.as_bytes());
        import_data.push(name.len() as u8);
        import_data.extend_from_slice(name.as_bytes());
        import_data.push(0x00); // function kind
        import_data.push(0x00); // typeidx = 0

        let section_size = import_data.len() as u32;
        wasm.extend_from_slice(&write_leb128_u32(section_size));
        wasm.extend_from_slice(&import_data);
        wasm
    }

    #[test]
    fn test_wasm_with_host_call_import_is_allowed() {
        // 唯一用户 host function：extism:host/user.host_call
        let wasm = wasm_with_import("extism:host/user", "host_call");
        assert!(scan_wasm_imports(&wasm).is_ok());
    }

    #[test]
    fn test_wasm_with_pdk_env_import_is_allowed() {
        // Extism PDK 基础设施：extism:host/env.*
        let wasm = wasm_with_import("extism:host/env", "input_offset");
        assert!(scan_wasm_imports(&wasm).is_ok());
    }

    #[test]
    fn test_wasm_with_unknown_user_host_function_is_rejected() {
        // 宿主仅注册 host_call；extism:host/user.* 其它函数一律拒绝。
        let wasm = wasm_with_import("extism:host/user", "other_func");
        let result = scan_wasm_imports(&wasm);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("UnknownHostCallImport"),
            "stray user host function must map to the shared UnknownHostCallImport category"
        );
    }

    #[test]
    fn test_wasm_with_disallowed_host_import_is_rejected() {
        // 非 extism:host/* 的裸 host import（旧 capability-名白名单模型）→ 拒绝。
        let wasm = wasm_with_import("extism", "time.now");
        let result = scan_wasm_imports(&wasm);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("DisallowedHostImport"),
            "non-Extism host import must map to the shared DisallowedHostImport category"
        );
    }

    #[test]
    fn test_wasm_with_wasi_import_is_rejected() {
        let wasm = wat::parse_str(
            r#"(module
                (import "wasi_snapshot_preview1" "random_get"
                  (func $random_get (param i32 i32) (result i32))))"#,
        )
        .expect("WAT must compile");

        let result = scan_wasm_imports(&wasm);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("wasi_snapshot_preview1"),
            "WASI import must be rejected at upload pre-validation"
        );
    }
}
