use serde_json::json;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;

pub const TRACKED_FILE_EDIT_TOOLS: &[&str] = &["write_file", "edit_file", "notebook_edit"];
const MAX_SNAPSHOT_BYTES: u64 = 2 * 1024 * 1024;
const LIVE_EMIT_INTERVAL_S: f64 = 0.18;
const LIVE_EMIT_LINE_STEP: i64 = 24;

#[derive(Debug, Clone)]
pub struct FileSnapshot {
    pub path: PathBuf,
    pub exists: bool,
    pub text: Option<String>,
    pub unreadable: bool,
    pub binary: bool,
    pub oversized: bool,
}

impl FileSnapshot {
    pub fn countable(&self) -> bool {
        self.text.is_some() && !self.binary && !self.oversized && !self.unreadable
    }
}

#[derive(Debug, Clone)]
pub struct FileEditTracker {
    pub call_id: String,
    pub tool: String,
    pub path: PathBuf,
    pub display_path: String,
    pub before: FileSnapshot,
}

pub fn is_file_edit_tool(tool_name: &str) -> bool {
    TRACKED_FILE_EDIT_TOOLS.contains(&tool_name)
}

pub fn display_file_edit_path(path: &Path, workspace: Option<&Path>) -> String {
    if let Some(workspace) = workspace {
        if let Ok(rel) = path.strip_prefix(workspace) {
            return rel.to_string_lossy().replace('\\', "/");
        }
    }
    path.to_string_lossy().replace('\\', "/")
}

pub fn resolve_file_edit_path(workspace: Option<&Path>, params: &serde_json::Value) -> Option<PathBuf> {
    let raw_path = params.get("path")?.as_str()?;
    if raw_path.trim().is_empty() {
        return None;
    }
    if let Some(workspace) = workspace {
        Some(workspace.join(raw_path).canonicalize().unwrap_or_else(|_| PathBuf::from(raw_path)))
    } else {
        Some(PathBuf::from(raw_path))
    }
}

pub fn read_file_snapshot(path: &Path) -> FileSnapshot {
    if !path.exists() || !path.is_file() {
        return FileSnapshot {
            path: path.to_path_buf(),
            exists: false,
            text: Some(String::new()),
            unreadable: false,
            binary: false,
            oversized: false,
        };
    }
    let size = match std::fs::metadata(path) {
        Ok(m) => m.len(),
        Err(_) => {
            return FileSnapshot {
                path: path.to_path_buf(),
                exists: path.exists(),
                text: None,
                unreadable: true,
                binary: false,
                oversized: false,
            };
        }
    };
    if size > MAX_SNAPSHOT_BYTES {
        return FileSnapshot {
            path: path.to_path_buf(),
            exists: true,
            text: None,
            unreadable: false,
            binary: false,
            oversized: true,
        };
    }
    let raw = match std::fs::read(path) {
        Ok(b) => b,
        Err(_) => {
            return FileSnapshot {
                path: path.to_path_buf(),
                exists: path.exists(),
                text: None,
                unreadable: true,
                binary: false,
                oversized: false,
            };
        }
    };
    if raw.contains(&0u8) {
        return FileSnapshot {
            path: path.to_path_buf(),
            exists: true,
            text: None,
            unreadable: false,
            binary: true,
            oversized: false,
        };
    }
    let text = match String::from_utf8(raw) {
        Ok(s) => s.replace("\r\n", "\n"),
        Err(_) => {
            return FileSnapshot {
                path: path.to_path_buf(),
                exists: true,
                text: None,
                unreadable: false,
                binary: true,
                oversized: false,
            };
        }
    };
    FileSnapshot {
        path: path.to_path_buf(),
        exists: true,
        text: Some(text),
        unreadable: false,
        binary: false,
        oversized: false,
    }
}

fn text_line_count(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    let mut line_count = 0;
    let mut last_was_newline = false;
    let mut last_was_cr = false;
    for ch in text.chars() {
        if ch == '\r' {
            line_count += 1;
            last_was_newline = true;
            last_was_cr = true;
        } else if ch == '\n' {
            if !last_was_cr {
                line_count += 1;
            }
            last_was_newline = true;
            last_was_cr = false;
        } else {
            last_was_newline = false;
            last_was_cr = false;
        }
    }
    if last_was_newline {
        line_count
    } else {
        line_count + 1
    }
}

fn similar_line_diff(before_lines: &[&str], after_lines: &[&str]) -> (usize, usize) {
    let mut added = 0;
    let mut deleted = 0;
    let mut i = 0;
    let mut j = 0;
    let mut matches: HashMap<&str, Vec<usize>> = HashMap::new();
    for (idx, line) in after_lines.iter().enumerate() {
        matches.entry(line).or_default().push(idx);
    }
    let after_matched = vec![false; after_lines.len()];
    while i < before_lines.len() && j < after_lines.len() {
        if before_lines[i] == after_lines[j] {
            i += 1;
            j += 1;
        } else {
            let mut found = false;
            if let Some(indices) = matches.get(before_lines[i]) {
                for &idx in indices {
                    if !after_matched[idx] && idx >= j {
                        let diff = idx - j;
                        added += diff;
                        j = idx;
                        found = true;
                        break;
                    }
                }
            }
            if found {
                continue;
            }
            deleted += 1;
            i += 1;
        }
    }
    deleted += before_lines.len() - i;
    added += after_lines.len() - j;
    (added, deleted)
}

pub fn line_diff_stats(before: Option<&str>, after: Option<&str>) -> (usize, usize) {
    let (before, after) = match (before, after) {
        (Some(b), Some(a)) => (b, a),
        _ => return (0, 0),
    };
    if before.is_empty() {
        return (text_line_count(after), 0);
    }
    let before_normalized = before.replace("\r\n", "\n");
    let after_normalized = after.replace("\r\n", "\n");
    let before_lines: Vec<&str> = before_normalized.lines().collect();
    let after_lines: Vec<&str> = after_normalized.lines().collect();
    similar_line_diff(&before_lines, &after_lines)
}

fn event_payload(
    tracker: &FileEditTracker,
    phase: &str,
    status: &str,
    added: usize,
    deleted: usize,
    approximate: bool,
    binary: bool,
) -> serde_json::Value {
    let mut payload = json!({
        "version": 1,
        "call_id": tracker.call_id,
        "tool": tracker.tool,
        "path": tracker.display_path,
        "absolute_path": tracker.path.to_string_lossy().replace('\\', "/"),
        "phase": phase,
        "added": added,
        "deleted": deleted,
        "approximate": approximate,
        "status": status,
    });
    if binary {
        payload["binary"] = json!(true);
    }
    payload
}

fn predict_after_text(
    tool_name: &str,
    params: &serde_json::Value,
    before: &FileSnapshot,
) -> Option<String> {
    if !before.countable() {
        return None;
    }
    let before_text = before.text.as_deref().unwrap_or("");
    match tool_name {
        "write_file" => {
            let content = params.get("content")?.as_str()?;
            Some(content.to_string())
        }
        "edit_file" => {
            let old_text = params.get("old_text")?.as_str()?;
            let new_text = params.get("new_text")?.as_str()?;
            let replace_all = params.get("replace_all").and_then(|v| v.as_bool()).unwrap_or(false);
            if old_text.is_empty() {
                if !before.exists {
                    return Some(new_text.to_string());
                }
                return Some(before_text.to_string());
            }
            if before_text.contains(old_text) {
                if replace_all {
                    Some(before_text.replace(old_text, new_text))
                } else {
                    Some(before_text.replacen(old_text, new_text, 1))
                }
            } else {
                None
            }
        }
        "notebook_edit" => predict_notebook_after_text(params, before_text),
        _ => None,
    }
}

fn empty_notebook() -> serde_json::Value {
    json!({
        "nbformat": 4,
        "nbformat_minor": 5,
        "metadata": {
            "kernelspec": {
                "display_name": "Python 3",
                "language": "python",
                "name": "python3"
            },
            "language_info": {"name": "python"}
        },
        "cells": []
    })
}

fn new_notebook_cell(source: &str, cell_type: &str) -> serde_json::Value {
    let mut cell = json!({
        "cell_type": cell_type,
        "source": source,
        "metadata": {}
    });
    if cell_type == "code" {
        cell["outputs"] = json!([]);
        cell["execution_count"] = serde_json::Value::Null;
    }
    cell
}

fn predict_notebook_after_text(params: &serde_json::Value, before_text: &str) -> Option<String> {
    let nb: serde_json::Value = if before_text.trim().is_empty() {
        empty_notebook()
    } else {
        serde_json::from_str(before_text).ok()?
    };
    let mut nb = nb;
    let cells = nb.get_mut("cells")?.as_array_mut()?;
    let cell_index = params.get("cell_index")?.as_i64()? as usize;
    let new_source = params.get("new_source")?.as_str()?;
    let cell_type = params
        .get("cell_type")
        .and_then(|v| v.as_str())
        .filter(|&t| t == "code" || t == "markdown")
        .unwrap_or("code");
    let mode = params
        .get("edit_mode")
        .and_then(|v| v.as_str())
        .filter(|&m| m == "replace" || m == "insert" || m == "delete")
        .unwrap_or("replace");
    match mode {
        "delete" => {
            if cell_index < cells.len() {
                cells.remove(cell_index);
            } else {
                return None;
            }
        }
        "insert" => {
            let insert_at = (cell_index + 1).min(cells.len());
            cells.insert(insert_at, new_notebook_cell(new_source, cell_type));
        }
        _ => {
            if cell_index >= cells.len() {
                return None;
            }
            let cell = cells[cell_index].as_object_mut()?;
            cell.insert("source".to_string(), serde_json::Value::String(new_source.to_string()));
            cell.insert("cell_type".to_string(), serde_json::Value::String(cell_type.to_string()));
            if cell_type == "code" {
                cell.entry("outputs".to_string()).or_insert_with(|| json!([]));
                cell.entry("execution_count".to_string()).or_insert(serde_json::Value::Null);
            } else {
                cell.remove("outputs");
                cell.remove("execution_count");
            }
        }
    }
    serde_json::to_string(&nb).ok()
}

pub fn build_file_edit_start_event(tracker: &FileEditTracker, params: &serde_json::Value) -> serde_json::Value {
    let predicted_after = predict_after_text(&tracker.tool, params, &tracker.before);
    let (added, deleted) = if tracker.before.countable() && predicted_after.is_some() {
        line_diff_stats(tracker.before.text.as_deref(), predicted_after.as_deref())
    } else {
        (0, 0)
    };
    event_payload(tracker, "start", "editing", added, deleted, true, false)
}

pub fn build_file_edit_end_event(tracker: &FileEditTracker, params: Option<&serde_json::Value>) -> serde_json::Value {
    let after = read_file_snapshot(&tracker.path);
    let params = params.unwrap_or(&serde_json::Value::Null);
    let mut counted = false;
    let (added, deleted) = if tracker.before.countable() && after.countable() {
        counted = true;
        line_diff_stats(tracker.before.text.as_deref(), after.text.as_deref())
    } else {
        let predicted_after = predict_after_text(&tracker.tool, params, &tracker.before);
        if tracker.before.countable() && predicted_after.is_some() {
            counted = true;
            line_diff_stats(tracker.before.text.as_deref(), predicted_after.as_deref())
        } else {
            (0, 0)
        }
    };
    let binary = (after.binary || after.oversized || after.unreadable) && !counted;
    event_payload(tracker, "end", "done", added, deleted, false, binary)
}

pub fn build_file_edit_error_event(tracker: &FileEditTracker, error: Option<&str>) -> serde_json::Value {
    let mut payload = event_payload(tracker, "error", "error", 0, 0, false, false);
    if let Some(err) = error {
        let err = err.trim();
        payload["error"] = json!(err.chars().take(240).collect::<String>());
    }
    payload
}

pub fn build_file_edit_live_event(tracker: &FileEditTracker, added: usize, deleted: usize) -> serde_json::Value {
    event_payload(tracker, "start", "editing", added, deleted, true, false)
}

pub fn build_file_edit_pending_event(call_id: &str, tool_name: &str, added: usize, deleted: usize) -> serde_json::Value {
    json!({
        "version": 1,
        "call_id": call_id,
        "tool": tool_name,
        "path": "",
        "phase": "start",
        "added": added,
        "deleted": deleted,
        "approximate": true,
        "status": "editing",
        "pending": true
    })
}

#[derive(Debug)]
struct StreamingJsonStringField {
    key: String,
    scan_pos: Option<usize>,
    closed: bool,
    escape: bool,
    unicode_remaining: usize,
    unicode_buffer: String,
    newline_count: usize,
    has_chars: bool,
    last_char_newline: bool,
    last_char_cr: bool,
}

impl StreamingJsonStringField {
    fn new(key: &str) -> Self {
        Self {
            key: key.to_string(),
            scan_pos: None,
            closed: false,
            escape: false,
            unicode_remaining: 0,
            unicode_buffer: String::new(),
            newline_count: 0,
            has_chars: false,
            last_char_newline: false,
            last_char_cr: false,
        }
    }

    fn line_count(&self) -> usize {
        if !self.has_chars {
            return 0;
        }
        self.newline_count + if self.last_char_newline { 0 } else { 1 }
    }

    fn reset(&mut self) {
        self.scan_pos = None;
        self.closed = false;
        self.escape = false;
        self.unicode_remaining = 0;
        self.unicode_buffer.clear();
        self.newline_count = 0;
        self.has_chars = false;
        self.last_char_newline = false;
        self.last_char_cr = false;
    }

    fn scan(&mut self, source: &str) {
        if self.closed {
            return;
        }
        if self.scan_pos.is_none() {
            let pattern = format!("\"{}\"\\s*:\\s*\"", regex::escape(&self.key));
            if let Some(m) = regex::Regex::new(&pattern).ok().and_then(|re| re.find(source)) {
                self.scan_pos = Some(m.end());
            } else {
                return;
            }
        }
        let mut i = self.scan_pos.unwrap();
        let chars: Vec<char> = source.chars().collect();
        let len = chars.len();
        while i < len {
            let ch = chars[i];
            if self.unicode_remaining > 0 {
                self.unicode_buffer.push(ch);
                self.unicode_remaining -= 1;
                if self.unicode_remaining == 0 {
                    let decoded = u32::from_str_radix(&self.unicode_buffer, 16)
                        .ok()
                        .and_then(|cp| char::from_u32(cp))
                        .unwrap_or('x');
                    self.unicode_buffer.clear();
                    self.mark_char(decoded);
                }
                i += 1;
                continue;
            }
            if self.escape {
                self.escape = false;
                if ch == 'u' {
                    self.unicode_remaining = 4;
                    self.unicode_buffer.clear();
                } else if ch == 'n' {
                    self.mark_char('\n');
                } else if ch == 'r' {
                    self.mark_char('\r');
                } else {
                    self.mark_char(ch);
                }
                i += 1;
                continue;
            }
            if ch == '\\' {
                self.escape = true;
                i += 1;
                continue;
            }
            if ch == '"' {
                self.closed = true;
                i += 1;
                break;
            }
            self.mark_char(ch);
            i += 1;
        }
        self.scan_pos = Some(i);
    }

    fn mark_char(&mut self, ch: char) {
        self.has_chars = true;
        if ch == '\r' {
            self.newline_count += 1;
            self.last_char_newline = true;
            self.last_char_cr = true;
        } else if ch == '\n' {
            if !self.last_char_cr {
                self.newline_count += 1;
            }
            self.last_char_newline = true;
            self.last_char_cr = false;
        } else {
            self.last_char_newline = false;
            self.last_char_cr = false;
        }
    }
}

#[derive(Debug)]
struct StreamingFileEditState {
    key: String,
    call_id: String,
    name: String,
    arguments: String,
    path: Option<String>,
    tracker: Option<FileEditTracker>,
    content: StreamingJsonStringField,
    old_text: StreamingJsonStringField,
    new_text: StreamingJsonStringField,
    emitted_once: bool,
    last_emitted_added: i64,
    last_emitted_deleted: i64,
    last_emit_at: f64,
    pending_emitted: bool,
    last_pending_added: i64,
    last_pending_deleted: i64,
    last_pending_at: f64,
}

impl StreamingFileEditState {
    fn new(key: &str) -> Self {
        Self {
            key: key.to_string(),
            call_id: String::new(),
            name: String::new(),
            arguments: String::new(),
            path: None,
            tracker: None,
            content: StreamingJsonStringField::new("content"),
            old_text: StreamingJsonStringField::new("old_text"),
            new_text: StreamingJsonStringField::new("new_text"),
            emitted_once: false,
            last_emitted_added: -1,
            last_emitted_deleted: -1,
            last_emit_at: 0.0,
            pending_emitted: false,
            last_pending_added: -1,
            last_pending_deleted: -1,
            last_pending_at: 0.0,
        }
    }

    fn apply_delta(&mut self, payload: &serde_json::Value) {
        if let Some(call_id) = payload.get("call_id").and_then(|v| v.as_str()) {
            if !call_id.is_empty() {
                self.call_id = call_id.to_string();
            }
        }
        if let Some(name) = payload.get("name").and_then(|v| v.as_str()) {
            if !name.is_empty() {
                self.name = name.to_string();
            }
        }
        if let Some(args) = payload.get("arguments") {
            if let Some(args) = args.as_str() {
                self.arguments = args.to_string();
                self.content.reset();
                self.old_text.reset();
                self.new_text.reset();
                return;
            }
        }
        if let Some(delta) = payload.get("arguments_delta").and_then(|v| v.as_str()) {
            if !delta.is_empty() {
                self.arguments.push_str(delta);
            }
        }
    }

    fn live_diff_counts(&mut self) -> (usize, usize) {
        if self.name == "write_file" {
            self.content.scan(&self.arguments);
            return (self.content.line_count(), 0);
        }
        if self.name == "edit_file" {
            self.old_text.scan(&self.arguments);
            self.new_text.scan(&self.arguments);
            return (self.new_text.line_count(), self.old_text.line_count());
        }
        (0, 0)
    }

    fn should_emit(&self, added: usize, deleted: usize, now: f64) -> bool {
        if !self.emitted_once {
            return true;
        }
        if added as i64 == self.last_emitted_added && deleted as i64 == self.last_emitted_deleted {
            return false;
        }
        if (added as i64 - self.last_emitted_added).abs() >= LIVE_EMIT_LINE_STEP
            || (deleted as i64 - self.last_emitted_deleted).abs() >= LIVE_EMIT_LINE_STEP
        {
            return true;
        }
        now - self.last_emit_at >= LIVE_EMIT_INTERVAL_S
    }

    fn mark_emitted(&mut self, added: usize, deleted: usize, now: f64) {
        self.emitted_once = true;
        self.last_emitted_added = added as i64;
        self.last_emitted_deleted = deleted as i64;
        self.last_emit_at = now;
    }

    fn should_emit_pending(&self, added: usize, deleted: usize, now: f64) -> bool {
        if !self.pending_emitted {
            return true;
        }
        if added as i64 == self.last_pending_added && deleted as i64 == self.last_pending_deleted {
            return false;
        }
        if (added as i64 - self.last_pending_added).abs() >= LIVE_EMIT_LINE_STEP
            || (deleted as i64 - self.last_pending_deleted).abs() >= LIVE_EMIT_LINE_STEP
        {
            return true;
        }
        now - self.last_pending_at >= LIVE_EMIT_INTERVAL_S
    }

    fn mark_pending_emitted(&mut self, added: usize, deleted: usize, now: f64) {
        self.pending_emitted = true;
        self.last_pending_added = added as i64;
        self.last_pending_deleted = deleted as i64;
        self.last_pending_at = now;
    }

    fn matches_final_tool_call(&mut self, tool_call: &serde_json::Value) -> bool {
        let call_id = tool_call.get("id").and_then(|v| v.as_str()).unwrap_or("");
        let canonical = if self.call_id.is_empty() {
            self.tracker.as_ref().map(|t| t.call_id.as_str()).unwrap_or("")
        } else {
            &self.call_id
        };
        if !call_id.is_empty() && !canonical.is_empty() && call_id == canonical {
            return true;
        }
        let name = tool_call.get("name").and_then(|v| v.as_str()).unwrap_or("");
        if name != self.name {
            return false;
        }
        let arguments = tool_call.get("arguments").and_then(|v| v.as_object());
        if arguments.is_none() {
            return false;
        }
        let arguments = arguments.unwrap();
        let path = arguments.get("path").and_then(|v| v.as_str()).unwrap_or("");
        if self.path.is_none() && !path.is_empty() {
            self.path = Some(path.to_string());
            return true;
        }
        !path.is_empty() && self.path.as_deref() == Some(path)
    }
}

fn stream_key(payload: &serde_json::Value) -> String {
    if let Some(index) = payload.get("index") {
        if let Some(index) = index.as_i64() {
            return format!("idx:{}", index);
        }
        if let Some(index) = index.as_str() {
            if !index.is_empty() {
                return format!("idx:{}", index);
            }
        }
    }
    if let Some(call_id) = payload.get("call_id").and_then(|v| v.as_str()) {
        if !call_id.is_empty() {
            return format!("id:{}", call_id);
        }
    }
    String::new()
}

fn extract_complete_json_string(source: &str, key: &str) -> Option<String> {
    let pattern = format!("\"{}\"\\s*:\\s*\"", regex::escape(key));
    let m = regex::Regex::new(&pattern).ok()?.find(source)?;
    let mut out = String::new();
    let chars: Vec<char> = source.chars().collect();
    let mut i = m.end();
    let mut escape = false;
    let len = chars.len();
    while i < len {
        let ch = chars[i];
        if escape {
            escape = false;
            match ch {
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                'u' => {
                    if i + 4 >= len {
                        return None;
                    }
                    let digits: String = chars[i + 1..i + 5].iter().collect();
                    if let Ok(cp) = u32::from_str_radix(&digits, 16) {
                        if let Some(c) = char::from_u32(cp) {
                            out.push(c);
                        } else {
                            return None;
                        }
                    } else {
                        return None;
                    }
                    i += 4;
                }
                _ => out.push(ch),
            }
            i += 1;
            continue;
        }
        if ch == '\\' {
            escape = true;
            i += 1;
            continue;
        }
        if ch == '"' {
            return Some(out);
        }
        out.push(ch);
        i += 1;
    }
    None
}

pub fn prepare_file_edit_tracker(
    call_id: &str,
    tool_name: &str,
    workspace: Option<&Path>,
    params: &serde_json::Value,
) -> Option<FileEditTracker> {
    if !is_file_edit_tool(tool_name) {
        return None;
    }
    let path = resolve_file_edit_path(workspace, params)?;
    let before = read_file_snapshot(&path);
    Some(FileEditTracker {
        call_id: call_id.to_string(),
        tool: tool_name.to_string(),
        path: path.clone(),
        display_path: display_file_edit_path(&path, workspace),
        before,
    })
}

pub type EmitFn = Arc<dyn Fn(Vec<serde_json::Value>) -> tokio::sync::oneshot::Receiver<()> + Send + Sync>;

pub struct StreamingFileEditTracker {
    workspace: Option<PathBuf>,
    emit: EmitFn,
    states: Arc<Mutex<HashMap<String, StreamingFileEditState>>>,
}

impl StreamingFileEditTracker {
    pub fn new(workspace: Option<PathBuf>, emit: EmitFn) -> Self {
        Self {
            workspace,
            emit,
            states: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn update(&self, payload: &serde_json::Value) {
        let key = stream_key(payload);
        if key.is_empty() {
            return;
        }
        let mut states = self.states.lock().await;
        let state = states.entry(key).or_insert_with_key(|k| StreamingFileEditState::new(k));
        state.apply_delta(payload);
        if state.name != "write_file" && state.name != "edit_file" {
            return;
        }
        if state.path.is_none() {
            state.path = extract_complete_json_string(&state.arguments, "path");
        }
        if state.path.is_none() {
            let (added, deleted) = state.live_diff_counts();
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs_f64();
            if state.should_emit_pending(added, deleted, now) {
                state.mark_pending_emitted(added, deleted, now);
                let event = build_file_edit_pending_event(
                    if state.call_id.is_empty() { &state.key } else { &state.call_id },
                    &state.name,
                    added,
                    deleted,
                );
                let _ = (self.emit)(vec![event]).await;
            }
            return;
        }
        if state.tracker.is_none() {
            let params = json!({"path": state.path.as_ref().unwrap()});
            state.tracker = prepare_file_edit_tracker(
                if state.call_id.is_empty() { &state.key } else { &state.call_id },
                &state.name,
                self.workspace.as_deref(),
                &params,
            );
            if state.tracker.is_none() {
                return;
            }
        }
        let (added, deleted) = state.live_diff_counts();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();
        if !state.should_emit(added, deleted, now) {
            return;
        }
        state.mark_emitted(added, deleted, now);
        let event = build_file_edit_live_event(state.tracker.as_ref().unwrap(), added, deleted);
        let _ = (self.emit)(vec![event]).await;
    }

    pub async fn flush(&self) {
        let mut states = self.states.lock().await;
        let mut events = Vec::new();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();
        for state in states.values_mut() {
            if state.tracker.is_none() {
                continue;
            }
            let (added, deleted) = state.live_diff_counts();
            if state.last_emitted_added == added as i64
                && state.last_emitted_deleted == deleted as i64
                && state.emitted_once
            {
                continue;
            }
            state.mark_emitted(added, deleted, now);
            events.push(build_file_edit_live_event(state.tracker.as_ref().unwrap(), added, deleted));
        }
        if !events.is_empty() {
            let _ = (self.emit)(events).await;
        }
    }

    pub async fn apply_final_call_ids(&self, final_tool_calls: &[serde_json::Value]) {
        let mut states = self.states.lock().await;
        for tool_call in final_tool_calls {
            if let Some(_canonical) = self.canonical_call_id_for_inner(tool_call, &mut states).await {
                if let Some(id) = tool_call.get("id") {
                    let _ = id;
                }
            }
        }
    }

    async fn canonical_call_id_for_inner(
        &self,
        tool_call: &serde_json::Value,
        states: &mut HashMap<String, StreamingFileEditState>,
    ) -> Option<String> {
        for state in states.values_mut() {
            if state.matches_final_tool_call(tool_call) {
                return Some(
                    if state.call_id.is_empty() {
                        state.tracker.as_ref().map(|t| t.call_id.clone()).unwrap_or(state.key.clone())
                    } else {
                        state.call_id.clone()
                    },
                );
            }
        }
        None
    }

    pub async fn canonical_call_id_for(&self, tool_call: &serde_json::Value) -> Option<String> {
        let mut states = self.states.lock().await;
        self.canonical_call_id_for_inner(tool_call, &mut states).await
    }

    pub async fn error_unmatched(&self, final_tool_calls: &[serde_json::Value], error: &str) {
        let mut states = self.states.lock().await;
        let mut events = Vec::new();
        for state in states.values_mut() {
            if state.tracker.is_none() {
                continue;
            }
            let matched = final_tool_calls.iter().any(|tc| state.matches_final_tool_call(tc));
            if matched {
                continue;
            }
            events.push(build_file_edit_error_event(state.tracker.as_ref().unwrap(), Some(error)));
        }
        drop(states);
        if !events.is_empty() {
            let _ = (self.emit)(events).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_file_edit_tool() {
        assert!(is_file_edit_tool("write_file"));
        assert!(is_file_edit_tool("edit_file"));
        assert!(is_file_edit_tool("notebook_edit"));
        assert!(!is_file_edit_tool("read_file"));
        assert!(!is_file_edit_tool(""));
    }

    #[test]
    fn test_text_line_count() {
        assert_eq!(text_line_count(""), 0);
        assert_eq!(text_line_count("hello"), 1);
        assert_eq!(text_line_count("hello\n"), 1);
        assert_eq!(text_line_count("hello\nworld"), 2);
        assert_eq!(text_line_count("hello\nworld\n"), 2);
    }

    #[test]
    fn test_line_diff_stats() {
        assert_eq!(line_diff_stats(None, Some("a")), (0, 0));
        assert_eq!(line_diff_stats(Some(""), Some("a\nb")), (2, 0));
        assert_eq!(line_diff_stats(Some("a\nb"), Some("a\nb\nc")), (1, 0));
    }

    #[test]
    fn test_display_file_edit_path() {
        let workspace = Path::new("/workspace");
        let path = Path::new("/workspace/src/main.rs");
        assert_eq!(display_file_edit_path(path, Some(workspace)), "src/main.rs");
        assert_eq!(display_file_edit_path(path, None), "/workspace/src/main.rs");
    }

    #[test]
    fn test_file_snapshot_countable() {
        let snap = FileSnapshot {
            path: PathBuf::from("test.txt"),
            exists: true,
            text: Some("hello".to_string()),
            unreadable: false,
            binary: false,
            oversized: false,
        };
        assert!(snap.countable());
        let snap = FileSnapshot {
            path: PathBuf::from("test.txt"),
            exists: true,
            text: None,
            unreadable: false,
            binary: false,
            oversized: false,
        };
        assert!(!snap.countable());
    }
}
