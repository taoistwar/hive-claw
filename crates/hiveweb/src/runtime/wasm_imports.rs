//! WASM binary parser: scan imports section (FR-005)
//!
//! Parses the WASM binary format to extract import names, used during
//! plugin upload to verify that all host function imports are registered.

use crate::runtime::capability::CAPABILITIES;
use crate::utils::error::AppError;

/// 返回所有已注册的 capability 名（即合法的 host import 名）
pub fn registered_imports() -> Vec<&'static str> {
    CAPABILITIES.iter().map(|c| c.name).collect()
}

/// 扫描 WASM 的 imports 段，验证所有 host import 都在已注册列表中。
/// 
/// WASM binary format:
/// - magic: \0asm (4 bytes)
/// - version: 1 (4 bytes)  
/// - sections...
/// 
/// Import section id = 2
/// Each import: module_len + module + name_len + name + import_kind
pub fn scan_wasm_imports(bytes: &[u8], allowed: &[&str]) -> Result<(), AppError> {
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
            let section_start = pos;
            let (num_imports, new_pos) = read_unsigned_leb128(bytes, pos)
                .ok_or_else(|| AppError::BadRequest("WASM 格式错误：无法读取 import count".into()))?;
            pos = new_pos;

            for _ in 0..num_imports {
                let (module, new_pos) = read_name(bytes, pos)
                    .ok_or_else(|| AppError::BadRequest("WASM 格式错误：无法读取 module name".into()))?;
                pos = new_pos;

                let (name, new_pos) = read_name(bytes, pos)
                    .ok_or_else(|| AppError::BadRequest("WASM 格式错误：无法读取 field name".into()))?;
                pos = new_pos;

                // Skip import kind byte
                pos += 1;

                let full_import = format!("{}.{}", module, name);
                found_imports.push(full_import);
            }

            // 跳过 section 剩余部分（理论上不应该有）
            let section_end = section_start + section_size as usize;
            if pos < section_end {
                pos = section_end;
            }
            break;
        } else {
            // Skip this section
            pos += section_size as usize;
        }
    }

    // 验证所有 import 都在 allowed 列表中
    for import in &found_imports {
        if !allowed.contains(&import.as_str()) {
            return Err(AppError::BadRequest(format!(
                "WASM 包含未注册的 host import: {}（宿主仅支持: {}）",
                import,
                allowed.join(", ")
            )));
        }
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
            0x00, 0x61, 0x73, 0x6d,
            0x01, 0x00, 0x00, 0x00,
            0x01, 0x00,
            0x0a, 0x00,
        ];
        let allowed = vec!["extism.time.now"];
        assert!(scan_wasm_imports(&wasm, &allowed).is_ok());
    }

    #[test]
    fn test_wasm_with_allowed_import() {
        let mut wasm = vec![
            0x00, 0x61, 0x73, 0x6d,
            0x01, 0x00, 0x00, 0x00,
            0x02,
        ];

        let mut import_data = Vec::new();
        import_data.push(1);

        import_data.extend_from_slice(&[6]);
        import_data.extend_from_slice(b"extism");
        import_data.extend_from_slice(&[8]);
        import_data.extend_from_slice(b"time.now");
        import_data.push(0);

        let section_size = import_data.len() as u32;
        wasm.push((section_size & 0x7F) as u8);
        wasm.extend_from_slice(&import_data);

        let allowed = vec!["extism.time.now"];
        assert!(scan_wasm_imports(&wasm, &allowed).is_ok());
    }

    #[test]
    fn test_wasm_with_unallowed_import() {
        let mut wasm = vec![
            0x00, 0x61, 0x73, 0x6d,
            0x01, 0x00, 0x00, 0x00,
            0x02,
        ];

        let mut import_data = Vec::new();
        import_data.push(1);

        import_data.extend_from_slice(&[6]);
        import_data.extend_from_slice(b"extism");
        import_data.extend_from_slice(&[12]);
        import_data.extend_from_slice(b"unknown.func");
        import_data.push(0);

        let section_size = import_data.len() as u32;
        wasm.push((section_size & 0x7F) as u8);
        wasm.extend_from_slice(&import_data);

        let allowed = vec!["extism.time.now"];
        let result = scan_wasm_imports(&wasm, &allowed);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("未注册的 host import"));
    }
}
