//! File system tools: `read_file`, `write_file`, `edit_file`, `list_dir`.
//! Port of `nanobot.agent.tools.filesystem`.
//!
//! # Scope
//!
//! The Python source (~900 lines) supports PDF/Office documents, image
//! MIME handling, curly-quote preservation, reindentation, and similarity
//! hints. This Rust port covers the core text-file behaviors and leaves
//! the advanced niceties as `TODO` — they can be layered back without
//! changing the tool surface.
//!
//! # Safety
//!
//! All four tools share [`FsTool`] for path resolution + allowed-dir
//! enforcement, so workspace restriction is honored consistently.

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use once_cell::sync::Lazy;
use serde_json::{Value, json};

use super::base::{Tool, ToolExecError};
use super::file_state::{FileStateStore, FileStates, current_file_states};
use super::sandbox::{PathError, resolve_path};

static IGNORE_DIRS: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    HashSet::from([
        ".git",
        "node_modules",
        "__pycache__",
        ".venv",
        "venv",
        "dist",
        "build",
        ".tox",
        ".mypy_cache",
        ".pytest_cache",
        ".ruff_cache",
        ".coverage",
        "htmlcov",
        "target",
    ])
});

/// Shared base for all filesystem tools — holds path policy and helpers.
#[derive(Clone)]
pub struct FsTool {
    pub workspace: Option<PathBuf>,
    pub allowed_dir: Option<PathBuf>,
    pub extra_allowed_dirs: Vec<PathBuf>,
    explicit_file_states: Option<Arc<FileStates>>,
    fallback_file_states: Arc<FileStates>,
}

impl FsTool {
    pub fn new(
        workspace: Option<PathBuf>,
        allowed_dir: Option<PathBuf>,
        extra_allowed_dirs: Vec<PathBuf>,
    ) -> Self {
        Self {
            workspace,
            allowed_dir,
            extra_allowed_dirs,
            explicit_file_states: None,
            fallback_file_states: Arc::new(FileStates::new()),
        }
    }

    pub fn with_file_states(mut self, states: Option<Arc<FileStates>>) -> Self {
        self.explicit_file_states = states;
        self
    }

    fn file_states(&self) -> Arc<FileStates> {
        if let Some(explicit) = &self.explicit_file_states {
            return explicit.clone();
        }
        current_file_states(&self.fallback_file_states)
    }

    pub fn resolve(&self, path: &str) -> Result<PathBuf, PathError> {
        resolve_path(
            path,
            self.workspace.as_deref(),
            self.allowed_dir.as_deref(),
            &self.extra_allowed_dirs,
        )
    }
}

fn string_param<'a>(params: &'a Value, key: &str) -> Option<&'a str> {
    params.get(key).and_then(|v| v.as_str())
}

fn bool_param(params: &Value, key: &str, default: bool) -> bool {
    params.get(key).and_then(|v| v.as_bool()).unwrap_or(default)
}

fn usize_param(params: &Value, key: &str, default: usize) -> usize {
    params
        .get(key)
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .unwrap_or(default)
}

// ---------------------------------------------------------------------------
// read_file
// ---------------------------------------------------------------------------

const READ_MAX_CHARS: usize = 128_000;
const READ_DEFAULT_LIMIT: usize = 2000;

pub struct ReadFileTool(pub FsTool);

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }
    fn description(&self) -> &str {
        "Read a text file. Output format: LINE_NUM| CONTENT. Use offset and limit for large files. Reads exceeding ~128K chars are truncated."
    }
    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "path":{"type":"string","description":"The file path to read"},
                "offset":{"type":"integer","description":"Line number to start reading from (1-indexed, default 1)","minimum":1,"default":1},
                "limit":{"type":"integer","description":"Maximum number of lines to read (default 2000)","minimum":1},
            },
            "required":["path"],
        })
    }
    fn read_only(&self) -> bool {
        true
    }
    fn scopes(&self) -> &[&str] {
        &["core", "subagent", "memory"]
    }

    async fn execute(&self, params: Value) -> Result<Value, ToolExecError> {
        let Some(path) = string_param(&params, "path") else {
            return Ok(Value::String("Error reading file: Unknown path".into()));
        };
        let offset = usize_param(&params, "offset", 1).max(1);
        let limit = params
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize);
        let fp = match self.0.resolve(path) {
            Ok(p) => p,
            Err(e) => return Ok(Value::String(format!("Error: {e}"))),
        };
        if !fp.exists() {
            return Ok(Value::String(format!("Error: File not found: {path}")));
        }
        if !fp.is_file() {
            return Ok(Value::String(format!("Error: Not a file: {path}")));
        }
        let file_states = self.0.file_states();
        if file_states.is_unchanged(&fp, offset, limit) {
            return Ok(Value::String(format!(
                "[File unchanged since last read: {path}]"
            )));
        }
        let raw = match fs::read(&fp) {
            Ok(r) => r,
            Err(e) => return Ok(Value::String(format!("Error reading file: {e}"))),
        };
        if raw.is_empty() {
            return Ok(Value::String(format!("(Empty file: {path})")));
        }
        let text = match String::from_utf8(raw) {
            Ok(s) => s.replace("\r\n", "\n"),
            Err(_) => {
                return Ok(Value::String(format!(
                    "Error: Cannot read binary file {path}. Only UTF-8 text is supported by this tool.",
                )));
            }
        };
        let all_lines: Vec<&str> = text.split('\n').collect();
        let total = all_lines.len();
        if offset > total {
            return Ok(Value::String(format!(
                "Error: offset {offset} is beyond end of file ({total} lines)"
            )));
        }
        let start = offset - 1;
        let end = (start + limit.unwrap_or(READ_DEFAULT_LIMIT)).min(total);
        let mut numbered: Vec<String> = all_lines[start..end]
            .iter()
            .enumerate()
            .map(|(i, l)| format!("{}| {}", start + i + 1, l))
            .collect();
        let mut result = numbered.join("\n");
        if result.len() > READ_MAX_CHARS {
            let mut trimmed: Vec<String> = Vec::new();
            let mut chars = 0;
            for line in numbered.drain(..) {
                chars += line.len() + 1;
                if chars > READ_MAX_CHARS {
                    break;
                }
                trimmed.push(line);
            }
            let actual_end = start + trimmed.len();
            result = trimmed.join("\n");
            result.push_str(&format!(
                "\n\n(Showing lines {}-{} of {}. Use offset={} to continue.)",
                offset,
                actual_end,
                total,
                actual_end + 1
            ));
        } else if end < total {
            result.push_str(&format!(
                "\n\n(Showing lines {}-{} of {}. Use offset={} to continue.)",
                offset,
                end,
                total,
                end + 1
            ));
        } else {
            result.push_str(&format!("\n\n(End of file — {total} lines total)"));
        }
        file_states.record_read(&fp, offset, limit);
        Ok(Value::String(result))
    }
}

// ---------------------------------------------------------------------------
// write_file
// ---------------------------------------------------------------------------

pub struct WriteFileTool(pub FsTool);

#[async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &str {
        "write_file"
    }
    fn description(&self) -> &str {
        "Write content to a file. Overwrites if the file already exists; creates parent directories as needed. For partial edits, prefer edit_file instead."
    }
    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "path":{"type":"string","description":"The file path to write to"},
                "content":{"type":"string","description":"The content to write"},
            },
            "required":["path","content"],
        })
    }
    fn scopes(&self) -> &[&str] {
        &["core", "subagent", "memory"]
    }
    async fn execute(&self, params: Value) -> Result<Value, ToolExecError> {
        let Some(path) = string_param(&params, "path") else {
            return Ok(Value::String("Error writing file: Unknown path".into()));
        };
        let Some(content) = string_param(&params, "content") else {
            return Ok(Value::String("Error writing file: Unknown content".into()));
        };
        let fp = match self.0.resolve(path) {
            Ok(p) => p,
            Err(e) => return Ok(Value::String(format!("Error: {e}"))),
        };
        if let Some(parent) = fp.parent() {
            if let Err(e) = fs::create_dir_all(parent) {
                return Ok(Value::String(format!("Error writing file: {e}")));
            }
        }
        if let Err(e) = fs::write(&fp, content) {
            return Ok(Value::String(format!("Error writing file: {e}")));
        }
        self.0.file_states().record_write(&fp);
        Ok(Value::String(format!(
            "Successfully wrote {} characters to {}",
            content.len(),
            fp.display()
        )))
    }
}

// ---------------------------------------------------------------------------
// edit_file
// ---------------------------------------------------------------------------

const EDIT_MAX_FILE_SIZE: u64 = 1024 * 1024 * 1024;

pub struct EditFileTool(pub FsTool);

#[async_trait]
impl Tool for EditFileTool {
    fn name(&self) -> &str {
        "edit_file"
    }
    fn description(&self) -> &str {
        "Edit a file by replacing old_text with new_text. Use read_file first to verify content. Set replace_all=true to replace all occurrences."
    }
    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "path":{"type":"string","description":"The file path to edit"},
                "old_text":{"type":"string","description":"Exact text to find (empty to create a new file)"},
                "new_text":{"type":"string","description":"Replacement text (empty to delete)"},
                "replace_all":{"type":"boolean","description":"Replace all occurrences (default false)","default":false},
            },
            "required":["path","old_text","new_text"],
        })
    }
    fn scopes(&self) -> &[&str] {
        &["core", "subagent", "memory"]
    }
    async fn execute(&self, params: Value) -> Result<Value, ToolExecError> {
        let Some(path) = string_param(&params, "path") else {
            return Ok(Value::String("Error editing file: Unknown path".into()));
        };
        let old_text = string_param(&params, "old_text").unwrap_or("");
        let new_text = string_param(&params, "new_text").unwrap_or("");
        let replace_all = bool_param(&params, "replace_all", false);

        let fp = match self.0.resolve(path) {
            Ok(p) => p,
            Err(e) => return Ok(Value::String(format!("Error: {e}"))),
        };

        if !fp.exists() {
            if old_text.is_empty() {
                if let Some(parent) = fp.parent() {
                    let _ = fs::create_dir_all(parent);
                }
                if let Err(e) = fs::write(&fp, new_text) {
                    return Ok(Value::String(format!("Error editing file: {e}")));
                }
                self.0.file_states().record_write(&fp);
                return Ok(Value::String(format!(
                    "Successfully created {}",
                    fp.display()
                )));
            }
            return Ok(Value::String(format!("Error: File not found: {path}")));
        }

        let size = fs::metadata(&fp).map(|m| m.len()).unwrap_or(0);
        if size > EDIT_MAX_FILE_SIZE {
            return Ok(Value::String(format!(
                "Error: File too large to edit ({} bytes). Maximum is 1 GiB.",
                size
            )));
        }

        if old_text.is_empty() {
            let raw = fs::read_to_string(&fp).unwrap_or_default();
            if !raw.trim().is_empty() {
                return Ok(Value::String(format!(
                    "Error: Cannot create file — {path} already exists and is not empty."
                )));
            }
            if let Err(e) = fs::write(&fp, new_text) {
                return Ok(Value::String(format!("Error editing file: {e}")));
            }
            self.0.file_states().record_write(&fp);
            return Ok(Value::String(format!(
                "Successfully edited {}",
                fp.display()
            )));
        }

        let file_states = self.0.file_states();
        let warning = file_states.check_read(&fp);
        let raw = match fs::read(&fp) {
            Ok(r) => r,
            Err(e) => return Ok(Value::String(format!("Error editing file: {e}"))),
        };
        let uses_crlf = raw.windows(2).any(|w| w == b"\r\n");
        let content = match String::from_utf8(raw) {
            Ok(s) => s.replace("\r\n", "\n"),
            Err(_) => {
                return Ok(Value::String(format!(
                    "Error: Cannot edit non-UTF-8 file {path}."
                )));
            }
        };
        let norm_old = old_text.replace("\r\n", "\n");
        let matches = find_all(&content, &norm_old);
        if matches.is_empty() {
            return Ok(Value::String(format!(
                "Error: old_text not found in {path}. Re-read the file and copy the exact text."
            )));
        }
        if matches.len() > 1 && !replace_all {
            let preview: Vec<String> = matches
                .iter()
                .take(3)
                .map(|idx| {
                    let line = content[..*idx].matches('\n').count() + 1;
                    format!("line {line}")
                })
                .collect();
            let mut hint = preview.join(", ");
            if matches.len() > 3 {
                hint.push_str(", ...");
            }
            return Ok(Value::String(format!(
                "Warning: old_text appears {} times at {}. Provide more context to make it unique, or set replace_all=true.",
                matches.len(),
                hint
            )));
        }
        let norm_new = new_text.replace("\r\n", "\n");

        let mut new_content = content.clone();
        let selected: Vec<usize> = if replace_all {
            matches
        } else {
            vec![matches[0]]
        };
        for idx in selected.into_iter().rev() {
            let end = idx + norm_old.len();
            let mut real_end = end;
            if norm_new.is_empty()
                && !norm_old.ends_with('\n')
                && new_content.as_bytes().get(real_end) == Some(&b'\n')
            {
                real_end += 1;
            }
            new_content.replace_range(idx..real_end, &norm_new);
        }

        let to_write = if uses_crlf {
            new_content.replace('\n', "\r\n")
        } else {
            new_content
        };
        if let Err(e) = fs::write(&fp, to_write) {
            return Ok(Value::String(format!("Error editing file: {e}")));
        }
        file_states.record_write(&fp);
        let msg = match warning {
            Some(w) => format!("{w}\nSuccessfully edited {}", fp.display()),
            None => format!("Successfully edited {}", fp.display()),
        };
        Ok(Value::String(msg))
    }
}

fn find_all(haystack: &str, needle: &str) -> Vec<usize> {
    if needle.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut start = 0;
    while let Some(idx) = haystack[start..].find(needle) {
        let absolute = start + idx;
        out.push(absolute);
        start = absolute + needle.len().max(1);
    }
    out
}

// ---------------------------------------------------------------------------
// list_dir
// ---------------------------------------------------------------------------

pub struct ListDirTool(pub FsTool);

#[async_trait]
impl Tool for ListDirTool {
    fn name(&self) -> &str {
        "list_dir"
    }
    fn description(&self) -> &str {
        "List the contents of a directory. Set recursive=true to explore nested structure. Common noise dirs (.git, node_modules, ...) are auto-ignored."
    }
    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "path":{"type":"string","description":"The directory path to list"},
                "recursive":{"type":"boolean","description":"Recursively list all files (default false)","default":false},
                "max_entries":{"type":"integer","description":"Maximum entries to return (default 200)","minimum":1,"default":200},
            },
            "required":["path"],
        })
    }
    fn read_only(&self) -> bool {
        true
    }
    fn scopes(&self) -> &[&str] {
        &["core", "subagent"]
    }
    async fn execute(&self, params: Value) -> Result<Value, ToolExecError> {
        let Some(path) = string_param(&params, "path") else {
            return Ok(Value::String("Error: Unknown path".into()));
        };
        let recursive = bool_param(&params, "recursive", false);
        let cap = usize_param(&params, "max_entries", 200).max(1);

        let dp = match self.0.resolve(path) {
            Ok(p) => p,
            Err(e) => return Ok(Value::String(format!("Error: {e}"))),
        };
        if !dp.exists() {
            return Ok(Value::String(format!("Error: Directory not found: {path}")));
        }
        if !dp.is_dir() {
            return Ok(Value::String(format!("Error: Not a directory: {path}")));
        }

        let mut items: Vec<String> = Vec::new();
        let mut total: usize = 0;
        if recursive {
            for entry in walkdir::WalkDir::new(&dp).sort_by_file_name().into_iter() {
                let Ok(entry) = entry else { continue };
                if entry.path() == dp {
                    continue;
                }
                let path = entry.path();
                if path
                    .components()
                    .any(|c| matches!(c, std::path::Component::Normal(name) if IGNORE_DIRS.contains(&name.to_string_lossy().as_ref())))
                {
                    continue;
                }
                total += 1;
                if items.len() < cap {
                    let rel = path.strip_prefix(&dp).unwrap_or(path);
                    let s = rel.display().to_string();
                    items.push(if entry.file_type().is_dir() {
                        format!("{s}/")
                    } else {
                        s
                    });
                }
            }
        } else {
            let mut entries: Vec<_> = match fs::read_dir(&dp) {
                Ok(rd) => rd.flatten().collect(),
                Err(e) => return Ok(Value::String(format!("Error: {e}"))),
            };
            entries.sort_by_key(|e| e.file_name());
            for entry in entries {
                let name = entry.file_name();
                let name_s = name.to_string_lossy();
                if IGNORE_DIRS.contains(name_s.as_ref()) {
                    continue;
                }
                total += 1;
                if items.len() < cap {
                    let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                    let prefix = if is_dir { "[dir] " } else { "[file] " };
                    items.push(format!("{prefix}{name_s}"));
                }
            }
        }
        if items.is_empty() && total == 0 {
            return Ok(Value::String(format!("Directory {path} is empty")));
        }
        let mut result = items.join("\n");
        if total > cap {
            result.push_str(&format!(
                "\n\n(truncated, showing first {cap} of {total} entries)"
            ));
        }
        Ok(Value::String(result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env::temp_dir;
    use std::path::Path;

    fn make_fs(ws: &Path) -> FsTool {
        FsTool::new(Some(ws.to_path_buf()), Some(ws.to_path_buf()), Vec::new())
    }

    #[tokio::test]
    async fn write_then_read_roundtrip() {
        let ws = temp_dir().join("rustbot_fs_wr");
        let _ = fs::remove_dir_all(&ws);
        fs::create_dir_all(&ws).unwrap();
        let tool = WriteFileTool(make_fs(&ws));
        let res = tool
            .execute(json!({"path":"a.txt","content":"hello\nworld"}))
            .await
            .unwrap();
        assert!(res.as_str().unwrap().contains("Successfully wrote"));

        let reader = ReadFileTool(make_fs(&ws));
        let v = reader
            .execute(json!({"path":"a.txt","offset":1,"limit":10}))
            .await
            .unwrap();
        let out = v.as_str().unwrap();
        assert!(out.contains("1| hello"));
        assert!(out.contains("2| world"));
    }

    #[tokio::test]
    async fn edit_replaces_unique_match() {
        let ws = temp_dir().join("rustbot_fs_ed");
        let _ = fs::remove_dir_all(&ws);
        fs::create_dir_all(&ws).unwrap();
        fs::write(ws.join("x.txt"), "foo bar baz").unwrap();
        let fs = make_fs(&ws);
        fs.file_states().record_read(ws.join("x.txt"), 1, None);
        let tool = EditFileTool(fs);
        let res = tool
            .execute(json!({"path":"x.txt","old_text":"bar","new_text":"qux"}))
            .await
            .unwrap();
        assert!(res.as_str().unwrap().contains("Successfully edited"));
        let content = fs::read_to_string(ws.join("x.txt")).unwrap();
        assert_eq!(content, "foo qux baz");
    }

    #[tokio::test]
    async fn list_dir_reports_entries() {
        let ws = temp_dir().join("rustbot_fs_ls");
        let _ = fs::remove_dir_all(&ws);
        fs::create_dir_all(&ws).unwrap();
        fs::write(ws.join("a.txt"), "").unwrap();
        fs::create_dir_all(ws.join("sub")).unwrap();
        let tool = ListDirTool(make_fs(&ws));
        let res = tool.execute(json!({"path":"."})).await.unwrap();
        let out = res.as_str().unwrap();
        assert!(out.contains("a.txt"));
        assert!(out.contains("sub"));
    }
}
