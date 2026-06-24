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
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{Value, json};

use super::base::{Tool, ToolExecError};
use super::file_state::{FileStates, current_file_states};
use super::sandbox::{PathError, resolve_path};

static BLOCKED_DEVICE_PATHS: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    HashSet::from([
        "/dev/zero",
        "/dev/random",
        "/dev/urandom",
        "/dev/full",
        "/dev/stdin",
        "/dev/stdout",
        "/dev/stderr",
        "/dev/tty",
        "/dev/console",
        "/dev/fd/0",
        "/dev/fd/1",
        "/dev/fd/2",
    ])
});

static PROC_FD_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^/proc/\d+/fd/[012]$").unwrap());

static PROC_SELF_FD_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^/proc/self/fd/[012]$").unwrap());

fn is_blocked_device<P: AsRef<Path>>(path: P) -> bool {
    let raw = path.as_ref().to_string_lossy();

    let resolved = fs::canonicalize(path.as_ref())
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| raw.to_string());

    if BLOCKED_DEVICE_PATHS.contains(raw.as_ref())
        || BLOCKED_DEVICE_PATHS.contains(resolved.as_str())
    {
        return true;
    }
    if PROC_FD_RE.is_match(&raw) || PROC_SELF_FD_RE.is_match(&raw) {
        return true;
    }
    if PROC_FD_RE.is_match(&resolved) || PROC_SELF_FD_RE.is_match(&resolved) {
        return true;
    }
    if resolved.starts_with("/dev/") {
        return true;
    }
    false
}

fn detect_image_mime(data: &[u8]) -> Option<&'static str> {
    if data.len() >= 8 && data[..8] == [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A] {
        return Some("image/png");
    }
    if data.len() >= 3 && data[..3] == [0xFF, 0xD8, 0xFF] {
        return Some("image/jpeg");
    }
    if data.len() >= 6 && (&data[..6] == b"GIF87a" || &data[..6] == b"GIF89a") {
        return Some("image/gif");
    }
    if data.len() >= 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    None
}

fn mime_from_extension(path: &str) -> Option<&'static str> {
    let lower = path.to_lowercase();
    if lower.ends_with(".png") {
        Some("image/png")
    } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        Some("image/jpeg")
    } else if lower.ends_with(".gif") {
        Some("image/gif")
    } else if lower.ends_with(".webp") {
        Some("image/webp")
    } else {
        None
    }
}

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
const MAX_PDF_PAGES: usize = 20;
const MARKDOWN_EXTS: &[&str] = &[".md", ".mdx", ".markdown"];

pub struct ReadFileTool(pub FsTool);

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }
    fn description(&self) -> String {
        "Read a file (text, image, or document). Text output format: LINE_NUM|CONTENT. Images return visual content for analysis. Supports PDF, DOCX, XLSX, PPTX documents. Use offset and limit for large text files. Reads exceeding ~128K chars are truncated.".into()
    }
    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "path":{"type":"string","description":"The file path to read"},
                "offset":{"type":"integer","description":"Line number to start reading from (1-indexed, default 1)","minimum":1,"default":1},
                "limit":{"type":"integer","description":"Maximum number of lines to read (default 2000)","minimum":1},
                "pages":{"type":"string","description":"Page range for PDF files, e.g. '1-5' (default: all, max 20 pages)"},
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
        let pages = params.get("pages").and_then(|v| v.as_str());
        let fp = match self.0.resolve(path) {
            Ok(p) => p,
            Err(e) => return Ok(Value::String(format!("Error: {e}"))),
        };

        if is_blocked_device(&fp) {
            return Ok(Value::String(format!(
                "Error: Reading {} is blocked (device path that could hang or produce infinite output).",
                fp.display()
            )));
        }

        if !fp.exists() {
            return Ok(Value::String(format!("Error: File not found: {path}")));
        }
        if !fp.is_file() {
            return Ok(Value::String(format!("Error: Not a file: {path}")));
        }

        let ext = fp
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();

        if ext == "pdf" {
            return Ok(Value::String(read_pdf(&fp, pages)));
        }
        if matches!(ext.as_str(), "docx" | "xlsx" | "pptx") {
            return Ok(Value::String(read_office_doc(&fp, &ext)));
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

        let mime = detect_image_mime(&raw).or_else(|| mime_from_extension(path));
        if let Some(m) = mime {
            if m.starts_with("image/") {
                return Ok(Value::String(build_image_response(&raw, m, &fp, path)));
            }
        }

        let text = match String::from_utf8(raw.clone()) {
            Ok(s) => s.replace("\r\n", "\n"),
            Err(_) => {
                let mime = detect_image_mime(&raw).or_else(|| mime_from_extension(path));
                if let Some(m) = mime {
                    if m.starts_with("image/") {
                        return Ok(Value::String(build_image_response(&raw, m, &fp, path)));
                    }
                }
                return Ok(Value::String(format!(
                    "Error: Cannot read binary file {path} (MIME: {}). Only UTF-8 text and images are supported.",
                    mime.unwrap_or("unknown")
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

fn read_pdf(fp: &Path, pages: Option<&str>) -> String {
    // TODO: Full PDF extraction requires a library like `lopdf` or `pdf-extract`.
    // Implementation plan:
    // 1. Add `pdf-extract` or `lopdf` crate to Cargo.toml.
    // 2. Parse page range string (e.g. "1-5") into 0-based start/end.
    // 3. Use the library to open the PDF and extract text per page.
    // 4. Respect MAX_PDF_PAGES limit (default 20 pages).
    // 5. Format output as "--- Page N ---\n{text}" per page.
    // 6. Handle continuation hints when document has more pages.
    // 7. Truncate at READ_MAX_CHARS with a suffix note.
    //
    // For now, a simple heuristic: try reading raw bytes and looking for
    // stream markers — this only works for very simple PDFs.
    // A proper implementation needs a PDF parsing library.
    let _ = fp;
    let _ = pages;
    let _ = MAX_PDF_PAGES;
    "Error: PDF reading requires a PDF parsing library (e.g. pdf-extract or lopdf). Add the crate to Cargo.toml and implement read_pdf.".to_string()
}

fn read_office_doc(fp: &Path, ext: &str) -> String {
    // TODO: Office document extraction requires parsing ZIP-based OOXML formats.
    // Implementation plan:
    // 1. Add `docx` crate for .docx, `calamine` for .xlsx, or write custom
    //    ZIP + XML parsing for all three formats.
    // 2. .docx: extract word/document.xml text nodes.
    // 3. .xlsx: extract sheet data from xl/worksheets/*.xml.
    // 4. .pptx: extract slide text from ppt/slides/*.xml.
    // 5. Handle errors gracefully and return user-friendly messages.
    // 6. Truncate at READ_MAX_CHARS.
    //
    // For now, return a helpful error message.
    let _ = fp;
    format!(
        "Error: {} reading requires an Office document parsing library. Add the appropriate crate (e.g. docx for .docx, calamine for .xlsx) to Cargo.toml and implement read_office_doc.",
        ext.to_uppercase()
    )
}

fn build_image_response(raw: &[u8], mime: &str, fp: &Path, path: &str) -> String {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    let b64 = STANDARD.encode(raw);
    format!(
        "[IMAGE:{mime};base64,{b64}|path={}|label=(Image file: {})]",
        fp.display(),
        path
    )
}

// ---------------------------------------------------------------------------
// edit_file helpers: quote preservation, reindent, match finding, similarity
// ---------------------------------------------------------------------------

struct QuoteTable;

impl QuoteTable {
    fn normalize(s: &str) -> String {
        s.chars()
            .map(|c| match c {
                '\u{2018}' | '\u{2019}' => '\'',
                '\u{201c}' | '\u{201d}' => '"',
                _ => c,
            })
            .collect()
    }

    fn has_curly_double(s: &str) -> bool {
        s.contains('\u{201c}') || s.contains('\u{201d}')
    }

    fn has_curly_single(s: &str) -> bool {
        s.contains('\u{2018}') || s.contains('\u{2019}')
    }

    fn to_curly_double(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        let mut opening = true;
        for ch in s.chars() {
            if ch == '"' {
                out.push(if opening { '\u{201c}' } else { '\u{201d}' });
                opening = !opening;
            } else {
                out.push(ch);
            }
        }
        out
    }

    fn to_curly_single(s: &str) -> String {
        let chars: Vec<char> = s.chars().collect();
        let mut out = String::with_capacity(s.len());
        let mut opening = true;
        for (i, &ch) in chars.iter().enumerate() {
            if ch != '\'' {
                out.push(ch);
                continue;
            }
            let prev = if i > 0 { chars[i - 1] } else { ' ' };
            let next = if i + 1 < chars.len() {
                chars[i + 1]
            } else {
                ' '
            };
            if prev.is_alphanumeric() && next.is_alphanumeric() {
                out.push('\u{2019}');
                continue;
            }
            out.push(if opening { '\u{2018}' } else { '\u{2019}' });
            opening = !opening;
        }
        out
    }
}

fn preserve_quote_style(old_text: &str, actual_text: &str, new_text: &str) -> String {
    if QuoteTable::normalize(old_text.trim()) != QuoteTable::normalize(actual_text.trim())
        || old_text == actual_text
    {
        return new_text.to_string();
    }
    let mut styled = new_text.to_string();
    if QuoteTable::has_curly_double(actual_text) && styled.contains('"') {
        styled = QuoteTable::to_curly_double(&styled);
    }
    if QuoteTable::has_curly_single(actual_text) && styled.contains('\'') {
        styled = QuoteTable::to_curly_single(&styled);
    }
    styled
}

fn leading_ws(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
        i += 1;
    }
    &line[..i]
}

fn reindent_like_match(old_text: &str, actual_text: &str, new_text: &str) -> String {
    let old_lines: Vec<&str> = old_text.split('\n').collect();
    let actual_lines: Vec<&str> = actual_text.split('\n').collect();
    if old_lines.len() != actual_lines.len() {
        return new_text.to_string();
    }

    let comparable: Vec<(&&str, &&str)> = old_lines
        .iter()
        .zip(actual_lines.iter())
        .filter(|(o, a)| !o.trim().is_empty() && !a.trim().is_empty())
        .collect();

    if comparable.is_empty()
        || comparable
            .iter()
            .any(|(o, a)| QuoteTable::normalize(o.trim()) != QuoteTable::normalize(a.trim()))
    {
        return new_text.to_string();
    }

    let old_ws = leading_ws(comparable[0].0);
    let actual_ws = leading_ws(comparable[0].1);
    if actual_ws == old_ws {
        return new_text.to_string();
    }

    let delta = if !old_ws.is_empty() {
        if !actual_ws.starts_with(old_ws) {
            return new_text.to_string();
        }
        &actual_ws[old_ws.len()..]
    } else {
        actual_ws
    };

    if delta.is_empty() {
        return new_text.to_string();
    }

    new_text
        .split('\n')
        .map(|line| {
            if line.is_empty() {
                line.to_string()
            } else {
                format!("{}{}", delta, line)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(Debug, Clone)]
struct MatchSpan {
    start: usize,
    end: usize,
    text: String,
    line: usize,
}

fn find_exact_matches(content: &str, old_text: &str) -> Vec<MatchSpan> {
    let mut matches = Vec::new();
    let mut start = 0;
    while let Some(idx) = content[start..].find(old_text) {
        let absolute = start + idx;
        let end = absolute + old_text.len();
        matches.push(MatchSpan {
            start: absolute,
            end,
            text: content[absolute..end].to_string(),
            line: content[..absolute].matches('\n').count() + 1,
        });
        start = end.max(absolute + 1);
    }
    matches
}

fn find_trim_matches(content: &str, old_text: &str, normalize_quotes: bool) -> Vec<MatchSpan> {
    let old_lines: Vec<String> = old_text
        .lines()
        .map(|l| {
            let s = l.trim().to_string();
            if normalize_quotes {
                QuoteTable::normalize(&s)
            } else {
                s
            }
        })
        .collect();
    if old_lines.is_empty() {
        return Vec::new();
    }

    let content_lines: Vec<&str> = content.lines().collect();
    if content_lines.len() < old_lines.len() {
        return Vec::new();
    }

    let mut offsets = Vec::with_capacity(content_lines.len() + 1);
    let mut pos = 0usize;
    for line in content.lines() {
        offsets.push(pos);
        pos += line.len() + 1;
    }
    offsets.push(pos);

    let stripped_old = &old_lines;
    let mut matches = Vec::new();
    let window_size = stripped_old.len();

    for i in 0..=(content_lines.len() - window_size) {
        let window: Vec<String> = content_lines[i..i + window_size]
            .iter()
            .map(|l| {
                let s = l.trim().to_string();
                if normalize_quotes {
                    QuoteTable::normalize(&s)
                } else {
                    s
                }
            })
            .collect();
        if &window != stripped_old {
            continue;
        }

        let start = offsets[i];
        let mut end = offsets[i + window_size];
        if content_lines[i + window_size - 1].ends_with('\n')
            || (i + window_size < content_lines.len())
        {
            end = end.saturating_sub(1);
        }
        matches.push(MatchSpan {
            start,
            end,
            text: content[start..end].to_string(),
            line: i + 1,
        });
    }
    matches
}

fn find_quote_matches(content: &str, old_text: &str) -> Vec<MatchSpan> {
    let norm_content = QuoteTable::normalize(content);
    let norm_old = QuoteTable::normalize(old_text);
    let mut matches = Vec::new();
    let mut start = 0;
    while let Some(idx) = norm_content[start..].find(&norm_old) {
        let absolute = start + idx;
        let end = absolute + old_text.len();
        matches.push(MatchSpan {
            start: absolute,
            end,
            text: content[absolute..end.min(content.len())].to_string(),
            line: content[..absolute].matches('\n').count() + 1,
        });
        start = absolute + norm_old.len().max(1);
    }
    matches
}

fn find_all_matches(content: &str, old_text: &str) -> Vec<MatchSpan> {
    let mut matches = find_exact_matches(content, old_text);
    if !matches.is_empty() {
        return matches;
    }
    matches = find_trim_matches(content, old_text, false);
    if !matches.is_empty() {
        return matches;
    }
    matches = find_trim_matches(content, old_text, true);
    if !matches.is_empty() {
        return matches;
    }
    find_quote_matches(content, old_text)
}

fn collapse_internal_whitespace(text: &str) -> String {
    text.lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect::<Vec<_>>()
        .join("\n")
}

fn diagnose_near_match(old_text: &str, actual_text: &str) -> Vec<String> {
    let mut hints = Vec::new();
    if old_text.to_lowercase() == actual_text.to_lowercase() && old_text != actual_text {
        hints.push("letter case differs".to_string());
    }
    if collapse_internal_whitespace(old_text) == collapse_internal_whitespace(actual_text)
        && old_text != actual_text
    {
        hints.push("whitespace differs".to_string());
    }
    if old_text.trim_end_matches('\n') == actual_text.trim_end_matches('\n')
        && old_text != actual_text
    {
        hints.push("trailing newline differs".to_string());
    }
    if QuoteTable::normalize(old_text) == QuoteTable::normalize(actual_text)
        && old_text != actual_text
    {
        hints.push("quote style differs".to_string());
    }
    hints
}

fn best_window(old_text: &str, content: &str) -> (f64, usize, Vec<String>, Vec<String>) {
    let lines: Vec<&str> = content.lines().collect();
    let old_lines: Vec<&str> = old_text.lines().collect();
    let window = old_lines.len().max(1);

    let mut best_ratio = -1.0f64;
    let mut best_start = 0;
    let mut best_window_lines: Vec<String> = Vec::new();

    for i in 0..lines.len().saturating_sub(window).saturating_add(1) {
        let current: Vec<&str> = lines[i..(i + window).min(lines.len())].to_vec();
        let ratio = similarity_ratio(&old_lines, &current);
        if ratio > best_ratio {
            best_ratio = ratio;
            best_start = i;
            best_window_lines = current.iter().map(|s| s.to_string()).collect();
        }
    }

    let actual_text = best_window_lines.join("\n");
    let hints = diagnose_near_match(old_text, &actual_text);
    (best_ratio, best_start + 1, best_window_lines, hints)
}

fn similarity_ratio(a: &[&str], b: &[&str]) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let matching = a.iter().zip(b.iter()).filter(|(x, y)| x == y).count();
    let max_len = a.len().max(b.len());
    if max_len == 0 {
        return 1.0;
    }
    matching as f64 / max_len as f64
}

fn strip_trailing_ws(text: &str) -> String {
    text.lines()
        .map(|line| line.trim_end())
        .collect::<Vec<_>>()
        .join("\n")
}

fn file_not_found_msg(path: &str, fp: &Path) -> String {
    let parent = fp.parent();
    let mut suggestions: Vec<String> = Vec::new();
    if let Some(p) = parent {
        if p.is_dir() {
            if let Ok(entries) = fs::read_dir(p) {
                let mut siblings: Vec<String> = Vec::new();
                for entry in entries.flatten() {
                    if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                        if let Some(name) = entry.file_name().to_str() {
                            siblings.push(name.to_string());
                        }
                    }
                }
                let close = fuzzy_close_matches(
                    fp.file_name().and_then(|n| n.to_str()).unwrap_or(""),
                    &siblings,
                    3,
                );
                suggestions = close
                    .into_iter()
                    .map(|s| p.join(s).display().to_string())
                    .collect();
            }
        }
    }
    let mut parts = vec![format!("Error: File not found: {}", path)];
    if !suggestions.is_empty() {
        parts.push(format!("Did you mean: {}?", suggestions.join(", ")));
    }
    parts.join("\n")
}

fn fuzzy_close_matches(target: &str, candidates: &[String], n: usize) -> Vec<String> {
    let mut scored: Vec<(f64, &String)> = candidates
        .iter()
        .map(|c| (similarity_str(target, c), c))
        .filter(|(s, _)| *s >= 0.6)
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.into_iter().take(n).map(|(_, c)| c.clone()).collect()
}

fn similarity_str(a: &str, b: &str) -> f64 {
    if a == b {
        return 1.0;
    }
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let max_len = a_chars.len().max(b_chars.len());
    if max_len == 0 {
        return 1.0;
    }
    let mut matches = 0usize;
    let mut i = 0;
    let mut j = 0;
    while i < a_chars.len() && j < b_chars.len() {
        if a_chars[i] == b_chars[j] {
            matches += 1;
            i += 1;
            j += 1;
        } else {
            j += 1;
        }
    }
    matches as f64 / max_len as f64
}

fn not_found_msg(old_text: &str, content: &str, path: &str) -> String {
    let (best_ratio, best_start, best_window_lines, hints) = best_window(old_text, content);
    if best_ratio > 0.5 {
        let diff = unified_diff(
            &old_text.lines().collect::<Vec<_>>(),
            &best_window_lines
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>(),
            "old_text (provided)",
            &format!("{} (actual, line {})", path, best_start),
        );
        let hint_text = if hints.is_empty() {
            String::new()
        } else {
            format!("\nPossible cause: {}.", hints.join(", "))
        };
        return format!(
            "Error: old_text not found in {}.{hint_text}\nBest match ({:.0}% similar) at line {}:\n{}",
            path,
            best_ratio * 100.0,
            best_start,
            diff
        );
    }

    if hints.is_empty() {
        format!(
            "Error: old_text not found in {}. No similar text found. Verify the file content.",
            path
        )
    } else {
        format!(
            "Error: old_text not found in {}. Possible cause: {}. Copy the exact text from read_file and try again.",
            path,
            hints.join(", ")
        )
    }
}

fn unified_diff(old: &[&str], new: &[&str], from: &str, to: &str) -> String {
    let mut out = Vec::new();
    out.push(format!("--- {}", from));
    out.push(format!("+++ {}", to));

    let mut i = 0;
    let mut j = 0;
    let mut hunk_old: Vec<&str> = Vec::new();
    let mut hunk_new: Vec<&str> = Vec::new();
    let mut old_start = 1;
    let mut new_start = 1;

    while i < old.len() || j < new.len() {
        if i < old.len() && j < new.len() && old[i] == new[j] {
            if !hunk_old.is_empty() || !hunk_new.is_empty() {
                out.push(format!(
                    "@@ -{},{} +{},{} @@",
                    old_start,
                    hunk_old.len(),
                    new_start,
                    hunk_new.len()
                ));
                for line in &hunk_old {
                    out.push(format!("-{}", line));
                }
                for line in &hunk_new {
                    out.push(format!("+{}", line));
                }
                hunk_old.clear();
                hunk_new.clear();
            }
            out.push(format!(" {}", old[i]));
            old_start = i + 2;
            new_start = j + 2;
            i += 1;
            j += 1;
        } else if i < old.len() && (j == new.len() || old.len() - i <= new.len() - j) {
            hunk_old.push(old[i]);
            i += 1;
        } else {
            hunk_new.push(new[j]);
            j += 1;
        }
    }

    if !hunk_old.is_empty() || !hunk_new.is_empty() {
        out.push(format!(
            "@@ -{},{} +{},{} @@",
            old_start,
            hunk_old.len(),
            new_start,
            hunk_new.len()
        ));
        for line in &hunk_old {
            out.push(format!("-{}", line));
        }
        for line in &hunk_new {
            out.push(format!("+{}", line));
        }
    }

    if out.len() <= 2 {
        return "(no diff)".to_string();
    }
    out.join("\n")
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
    fn description(&self) -> String {
        "Write content to a file. Overwrites if the file already exists; creates parent directories as needed. For partial edits, prefer edit_file instead.".into()
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
    fn description(&self) -> String {
        "Edit a file by replacing old_text with new_text. Use read_file first to verify content. Set replace_all=true to replace all occurrences.".into()
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

        if path.ends_with(".ipynb") {
            return Ok(Value::String(
                "Error: This is a Jupyter notebook. Use the notebook_edit tool instead of edit_file.".into()
            ));
        }

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
            return Ok(Value::String(file_not_found_msg(path, &fp)));
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
        let norm_new = new_text.replace("\r\n", "\n");

        let is_markdown = fp
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| {
                let lower = e.to_lowercase();
                MARKDOWN_EXTS
                    .iter()
                    .any(|ext| lower == *ext.trim_start_matches('.'))
            })
            .unwrap_or(false);
        let norm_new = if is_markdown {
            norm_new
        } else {
            strip_trailing_ws(&norm_new)
        };

        let matches = find_all_matches(&content, &norm_old);
        if matches.is_empty() {
            return Ok(Value::String(not_found_msg(&norm_old, &content, path)));
        }
        let count = matches.len();
        if count > 1 && !replace_all {
            let line_numbers: Vec<String> = matches
                .iter()
                .take(3)
                .map(|m| format!("line {}", m.line))
                .collect();
            let mut preview = line_numbers.join(", ");
            if matches.len() > 3 {
                preview.push_str(", ...");
            }
            let location_hint = if preview.is_empty() {
                String::new()
            } else {
                format!(" at {}", preview)
            };
            return Ok(Value::String(format!(
                "Warning: old_text appears {} times{}. Provide more context to make it unique, or set replace_all=true.",
                count, location_hint
            )));
        }

        let mut new_content = content.clone();
        let selected: Vec<&MatchSpan> = if replace_all {
            matches.iter().collect()
        } else {
            vec![&matches[0]]
        };
        for m in selected.into_iter().rev() {
            let mut replacement = preserve_quote_style(&norm_old, &m.text, &norm_new);
            replacement = reindent_like_match(&norm_old, &m.text, &replacement);

            let mut end = m.end;
            if replacement.is_empty()
                && !m.text.ends_with('\n')
                && content.as_bytes().get(end) == Some(&b'\n')
            {
                end += 1;
            }
            new_content.replace_range(m.start..end, &replacement);
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

// ---------------------------------------------------------------------------
// list_dir
// ---------------------------------------------------------------------------

pub struct ListDirTool(pub FsTool);

#[async_trait]
impl Tool for ListDirTool {
    fn name(&self) -> &str {
        "list_dir"
    }
    fn description(&self) -> String {
        "List the contents of a directory. Set recursive=true to explore nested structure. Common noise dirs (.git, node_modules, ...) are auto-ignored.".into()
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
