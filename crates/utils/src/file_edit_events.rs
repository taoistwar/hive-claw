/// File-edit activity helpers for WebUI progress events.

use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};

use serde_json::Value;

const TRACKED_FILE_EDIT_TOOLS: &[&str] = &["write_file", "edit_file", "notebook_edit"];
const MAX_SNAPSHOT_BYTES: u64 = 2 * 1024 * 1024;
const LIVE_EMIT_INTERVAL_S: f64 = 0.18;
const LIVE_EMIT_LINE_STEP: i64 = 24;

type EmitFuture<'a> = std::pin::Pin<Box<dyn Future<Output = ()> + Send + 'a>>;
type EmitFn = Box<dyn Fn(Vec<HashMap<String, Value>>) -> EmitFuture<'static> + Send>;

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

#[derive(Debug)]
pub struct FileEditTracker {
    pub call_id: String,
    pub tool: String,
    pub path: PathBuf,
    pub display_path: String,
    pub before: FileSnapshot,
}

pub fn is_file_edit_tool(tool_name: Option<&str>) -> bool {
    tool_name
        .map(|n| TRACKED_FILE_EDIT_TOOLS.contains(&n))
        .unwrap_or(false)
}

pub fn resolve_file_edit_path(
    _tool: &dyn TODO_ToolResolver,
    workspace: Option<&Path>,
    params: Option<&HashMap<String, Value>>,
) -> Option<PathBuf> {
    let params = params?;
    let raw_path = params.get("path")?.as_str()?;
    if raw_path.trim().is_empty() {
        return None;
    }
    match workspace {
        Some(ws) => Some(ws.join(raw_path)),
        None => Some(PathBuf::from(raw_path)),
    }
}

pub fn display_file_edit_path(path: &Path, workspace: Option<&Path>) -> String {
    if let Some(ws) = workspace {
        if let (Ok(p), Ok(w)) = (path.canonicalize(), ws.canonicalize()) {
            if let Ok(rel) = p.strip_prefix(&w) {
                return rel.to_string_lossy().replace('\\', "/");
            }
        }
    }
    path.to_string_lossy().replace('\\', "/")
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

    let size = match path.metadata() {
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
        Ok(r) => r,
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

    match String::from_utf8(raw) {
        Ok(text) => FileSnapshot {
            path: path.to_path_buf(),
            exists: true,
            text: Some(text.replace("\r\n", "\n")),
            unreadable: false,
            binary: false,
            oversized: false,
        },
        Err(_) => FileSnapshot {
            path: path.to_path_buf(),
            exists: true,
            text: None,
            unreadable: false,
            binary: true,
            oversized: false,
        },
    }
}

pub fn line_diff_stats(before: Option<&str>, after: Option<&str>) -> (i64, i64) {
    let (Some(before), Some(after)) = (before, after) else {
        return (0, 0);
    };

    if before.is_empty() {
        return (text_line_count(after), 0);
    }

    let before_normalized = before.replace("\r\n", "\n");
    let after_normalized = after.replace("\r\n", "\n");

    let before_lines: Vec<&str> = before_normalized.lines().collect();
    let after_lines: Vec<&str> = after_normalized.lines().collect();

    let lcs_len = longest_common_subsequence_len(&before_lines, &after_lines) as i64;

    let deleted = before_lines.len() as i64 - lcs_len;
    let added = after_lines.len() as i64 - lcs_len;

    (added, deleted)
}

fn longest_common_subsequence_len<T: PartialEq>(a: &[T], b: &[T]) -> usize {
    if a.len() > 5000 || b.len() > 5000 {
        return 0;
    }
    let n = a.len();
    let m = b.len();
    let mut dp = vec![vec![0usize; m + 1]; n + 1];

    for i in 1..=n {
        for j in 1..=m {
            if a[i - 1] == b[j - 1] {
                dp[i][j] = dp[i - 1][j - 1] + 1;
            } else {
                dp[i][j] = dp[i - 1][j].max(dp[i][j - 1]);
            }
        }
    }

    dp[n][m]
}

fn text_line_count(text: &str) -> i64 {
    if text.is_empty() {
        return 0;
    }
    let mut count: i64 = 0;
    let mut last_was_newline = false;
    let mut last_was_cr = false;

    for ch in text.chars() {
        match ch {
            '\r' => {
                count += 1;
                last_was_newline = true;
                last_was_cr = true;
            }
            '\n' => {
                if !last_was_cr {
                    count += 1;
                }
                last_was_newline = true;
                last_was_cr = false;
            }
            _ => {
                last_was_newline = false;
                last_was_cr = false;
            }
        }
    }

    if last_was_newline { count } else { count + 1 }
}

pub fn prepare_file_edit_tracker(
    call_id: &str,
    tool_name: &str,
    _tool: &dyn TODO_ToolResolver,
    workspace: Option<&Path>,
    params: Option<&HashMap<String, Value>>,
) -> Option<FileEditTracker> {
    if !is_file_edit_tool(Some(tool_name)) {
        return None;
    }
    let path = resolve_file_edit_path(_tool, workspace, params)?;
    let before = read_file_snapshot(&path);
    Some(FileEditTracker {
        call_id: call_id.to_string(),
        tool: tool_name.to_string(),
        path: path.clone(),
        display_path: display_file_edit_path(&path, workspace),
        before,
    })
}

fn predict_after_text(
    tool_name: &str,
    params: &HashMap<String, Value>,
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
            let replace_all = params
                .get("replace_all")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            if old_text.is_empty() {
                return if before.exists {
                    Some(before_text.to_string())
                } else {
                    Some(new_text.to_string())
                };
            }

            if before_text.contains(old_text) {
                return Some(if replace_all {
                    before_text.replace(old_text, new_text)
                } else {
                    before_text.replacen(old_text, new_text, 1)
                });
            }
            None
        }
        "notebook_edit" => predict_notebook_after_text(params, before_text),
        _ => None,
    }
}

fn predict_notebook_after_text(
    params: &HashMap<String, Value>,
    before_text: &str,
) -> Option<String> {
    let nb: Value = if before_text.trim().is_empty() {
        empty_notebook()
    } else {
        serde_json::from_str(before_text).ok()?
    };

    let mut nb = nb;
    let cells = nb.get_mut("cells")?.as_array_mut()?;

    let cell_index = params
        .get("cell_index")
        .and_then(|v| v.as_i64())
        .unwrap_or(0) as usize;

    let new_source = params
        .get("new_source")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let cell_type = match params.get("cell_type").and_then(|v| v.as_str()) {
        Some("code") => "code",
        Some("markdown") => "markdown",
        _ => "code",
    };

    let mode = match params.get("edit_mode").and_then(|v| v.as_str()) {
        Some("replace") => "replace",
        Some("insert") => "insert",
        Some("delete") => "delete",
        _ => "replace",
    };

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
            cell.insert("source".to_string(), Value::String(new_source.to_string()));
            cell.insert("cell_type".to_string(), Value::String(cell_type.to_string()));
            if cell_type == "code" {
                cell.entry("outputs".to_string())
                    .or_insert_with(|| Value::Array(vec![]));
                cell.entry("execution_count".to_string())
                    .or_insert(Value::Null);
            } else {
                cell.remove("outputs");
                cell.remove("execution_count");
            }
        }
    }

    serde_json::to_string(&nb).ok()
}

fn empty_notebook() -> Value {
    serde_json::json!({
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

fn new_notebook_cell(source: &str, cell_type: &str) -> Value {
    let mut cell = serde_json::Map::new();
    cell.insert("cell_type".to_string(), Value::String(cell_type.to_string()));
    cell.insert("source".to_string(), Value::String(source.to_string()));
    cell.insert("metadata".to_string(), Value::Object(serde_json::Map::new()));
    if cell_type == "code" {
        cell.insert("outputs".to_string(), Value::Array(vec![]));
        cell.insert("execution_count".to_string(), Value::Null);
    }
    Value::Object(cell)
}

fn event_payload(
    tracker: &FileEditTracker,
    phase: &str,
    status: &str,
    added: i64,
    deleted: i64,
    approximate: bool,
    binary: bool,
) -> HashMap<String, Value> {
    let mut payload: HashMap<String, Value> = [
        ("version".to_string(), Value::Number(1.into())),
        ("call_id".to_string(), Value::String(tracker.call_id.clone())),
        ("tool".to_string(), Value::String(tracker.tool.clone())),
        ("path".to_string(), Value::String(tracker.display_path.clone())),
        ("absolute_path".to_string(), Value::String(tracker.path.to_string_lossy().to_string())),
        ("phase".to_string(), Value::String(phase.to_string())),
        ("added".to_string(), Value::Number(added.max(0).into())),
        ("deleted".to_string(), Value::Number(deleted.max(0).into())),
        ("approximate".to_string(), Value::Bool(approximate)),
        ("status".to_string(), Value::String(status.to_string())),
    ]
    .into_iter()
    .collect();

    if binary {
        payload.insert("binary".to_string(), Value::Bool(true));
    }

    payload
}

pub fn build_file_edit_start_event(
    tracker: &FileEditTracker,
    params: Option<&HashMap<String, Value>>,
) -> HashMap<String, Value> {
    let predicted_after = predict_after_text(&tracker.tool, params.unwrap_or(&HashMap::new()), &tracker.before);
    let (added, deleted) = if tracker.before.countable() && predicted_after.is_some() {
        line_diff_stats(tracker.before.text.as_deref(), predicted_after.as_deref())
    } else {
        (0, 0)
    };

    event_payload(tracker, "start", "editing", added, deleted, true, false)
}

pub fn build_file_edit_end_event(
    tracker: &FileEditTracker,
    params: Option<&HashMap<String, Value>>,
) -> HashMap<String, Value> {
    let after = read_file_snapshot(&tracker.path);
    let mut counted = false;
    let (added, deleted) = if tracker.before.countable() && after.countable() {
        counted = true;
        line_diff_stats(tracker.before.text.as_deref(), after.text.as_deref())
    } else {
        let predicted_after = predict_after_text(
            &tracker.tool,
            params.unwrap_or(&HashMap::new()),
            &tracker.before,
        );
        if tracker.before.countable() && predicted_after.is_some() {
            counted = true;
            line_diff_stats(tracker.before.text.as_deref(), predicted_after.as_deref())
        } else {
            (0, 0)
        }
    };

    event_payload(
        tracker,
        "end",
        "done",
        added,
        deleted,
        false,
        (after.binary || after.oversized || after.unreadable) && !counted,
    )
}

pub fn build_file_edit_error_event(
    tracker: &FileEditTracker,
    error: Option<&str>,
) -> HashMap<String, Value> {
    let mut payload = event_payload(tracker, "error", "error", 0, 0, false, false);
    if let Some(err) = error {
        payload.insert(
            "error".to_string(),
            Value::String(err.trim().chars().take(240).collect()),
        );
    }
    payload
}

pub fn build_file_edit_live_event(
    tracker: &FileEditTracker,
    added: i64,
    deleted: i64,
) -> HashMap<String, Value> {
    event_payload(tracker, "start", "editing", added, deleted, true, false)
}

pub fn build_file_edit_pending_event(
    call_id: &str,
    tool_name: &str,
    added: i64,
    deleted: i64,
) -> HashMap<String, Value> {
    [
        ("version".to_string(), Value::Number(1.into())),
        ("call_id".to_string(), Value::String(call_id.to_string())),
        ("tool".to_string(), Value::String(tool_name.to_string())),
        ("path".to_string(), Value::String(String::new())),
        ("phase".to_string(), Value::String("start".to_string())),
        ("added".to_string(), Value::Number(added.max(0).into())),
        ("deleted".to_string(), Value::Number(deleted.max(0).into())),
        ("approximate".to_string(), Value::Bool(true)),
        ("status".to_string(), Value::String("editing".to_string())),
        ("pending".to_string(), Value::Bool(true)),
    ]
    .into_iter()
    .collect()
}

pub struct StreamingFileEditTracker {
    workspace: Option<PathBuf>,
    states: HashMap<String, StreamingFileEditState>,
}

impl StreamingFileEditTracker {
    pub fn new(workspace: Option<PathBuf>) -> Self {
        Self {
            workspace,
            states: HashMap::new(),
        }
    }

    pub async fn update(&mut self, payload: &HashMap<String, Value>, emit: &EmitFn) {
        let key = match stream_key(payload) {
            Some(k) => k,
            None => return,
        };

        let state = self
            .states
            .entry(key)
            .or_insert_with(|| StreamingFileEditState::new());

        state.apply_delta(payload);

        if state.name != "write_file" && state.name != "edit_file" {
            return;
        }

        if state.path.is_none() {
            state.path = extract_complete_json_string(&state.arguments, "path");
        }

        if state.path.is_none() {
            let (added, deleted) = state.live_diff_counts();
            let now = std::time::Instant::now();
            if state.should_emit_pending(added, deleted, now) {
                state.mark_pending_emitted(added, deleted, now);
                emit(vec![build_file_edit_pending_event(
                    state.call_id.as_deref().unwrap_or(&state.key),
                    &state.name,
                    added,
                    deleted,
                )])
                .await;
            }
            return;
        }

        let (added, deleted) = state.live_diff_counts();
        let now = std::time::Instant::now();
        if !state.should_emit(added, deleted, now) {
            return;
        }
        state.mark_emitted(added, deleted, now);
    }

    pub async fn flush(&mut self, _emit: &EmitFn) {
        // TODO: Flush pending events
    }

    pub fn apply_final_call_ids(&mut self, _final_tool_calls: &[&dyn TODO_ToolCall]) {
        // TODO: Apply final call IDs to states
    }

    pub fn canonical_call_id_for(&self, _tool_call: &dyn TODO_ToolCall) -> Option<String> {
        None
    }

    pub async fn error_unmatched(
        &self,
        _final_tool_calls: &[&dyn TODO_ToolCall],
        _error: &str,
        _emit: &EmitFn,
    ) {
        // TODO: Mark streamed edits as failed
    }
}

fn stream_key(payload: &HashMap<String, Value>) -> Option<String> {
    if let Some(index) = payload.get("index") {
        if let Some(n) = index.as_i64() {
            return Some(format!("idx:{n}"));
        }
        if let Some(s) = index.as_str() {
            if !s.is_empty() {
                return Some(format!("idx:{s}"));
            }
        }
    }
    if let Some(call_id) = payload.get("call_id") {
        if let Some(s) = call_id.as_str() {
            if !s.is_empty() {
                return Some(format!("id:{s}"));
            }
        }
    }
    None
}

fn extract_complete_json_string(source: &str, key: &str) -> Option<String> {
    let pattern = format!(r#""{}"\s*:\s*""#, regex::escape(key));
    let re = regex::Regex::new(&pattern).ok()?;
    let m = re.find(source)?;
    let start = m.end();

    let mut out = String::new();
    let mut escape = false;
    let chars: Vec<char> = source[start..].chars().collect();
    let mut i = 0;

    while i < chars.len() {
        let ch = chars[i];
        if escape {
            escape = false;
            match ch {
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                'u' => {
                    if i + 4 < chars.len() {
                        let digits: String = chars[i + 1..i + 5].iter().collect();
                        if let Ok(code) = u32::from_str_radix(&digits, 16) {
                            if let Some(c) = char::from_u32(code) {
                                out.push(c);
                            }
                        }
                        i += 4;
                    } else {
                        return None;
                    }
                }
                _ => out.push(ch),
            }
        } else if ch == '\\' {
            escape = true;
        } else if ch == '"' {
            return Some(out);
        } else {
            out.push(ch);
        }
        i += 1;
    }
    None
}

#[derive(Debug)]
struct StreamingJsonStringField {
    key: String,
    scan_pos: Option<usize>,
    closed: bool,
    escape: bool,
    unicode_remaining: usize,
    unicode_buffer: String,
    newline_count: i64,
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

    fn line_count(&self) -> i64 {
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
            let pattern = format!(r#""{}"\s*:\s*""#, regex::escape(&self.key));
            if let Ok(re) = regex::Regex::new(&pattern) {
                if let Some(m) = re.find(source) {
                    self.scan_pos = Some(m.end());
                }
            }
        }

        let pos = match self.scan_pos {
            Some(p) => p,
            None => return,
        };

        let chars: Vec<char> = source[pos..].chars().collect();
        let mut offset = pos;

        for ch in chars {
            if self.unicode_remaining > 0 {
                self.unicode_buffer.push(ch);
                self.unicode_remaining -= 1;
                if self.unicode_remaining == 0 {
                    let decoded = u32::from_str_radix(&self.unicode_buffer, 16)
                        .ok()
                        .and_then(char::from_u32)
                        .unwrap_or('x');
                    self.unicode_buffer.clear();
                    self.mark_char(decoded);
                }
                offset += ch.len_utf8();
                continue;
            }

            if self.escape {
                self.escape = false;
                if ch == 'u' {
                    self.unicode_remaining = 4;
                    self.unicode_buffer.clear();
                } else {
                    let decoded = match ch {
                        'n' => '\n',
                        'r' => '\r',
                        _ => ch,
                    };
                    self.mark_char(decoded);
                }
                offset += ch.len_utf8();
                continue;
            }

            if ch == '\\' {
                self.escape = true;
                offset += ch.len_utf8();
                continue;
            }

            if ch == '"' {
                self.closed = true;
                self.scan_pos = Some(offset + ch.len_utf8());
                return;
            }

            self.mark_char(ch);
            offset += ch.len_utf8();
        }

        self.scan_pos = Some(offset);
    }

    fn mark_char(&mut self, ch: char) {
        self.has_chars = true;
        match ch {
            '\r' => {
                self.newline_count += 1;
                self.last_char_newline = true;
                self.last_char_cr = true;
            }
            '\n' => {
                if !self.last_char_cr {
                    self.newline_count += 1;
                }
                self.last_char_newline = true;
                self.last_char_cr = false;
            }
            _ => {
                self.last_char_newline = false;
                self.last_char_cr = false;
            }
        }
    }
}

#[derive(Debug)]
struct StreamingFileEditState {
    key: String,
    call_id: Option<String>,
    name: String,
    arguments: String,
    path: Option<String>,
    content: StreamingJsonStringField,
    old_text: StreamingJsonStringField,
    new_text: StreamingJsonStringField,
    emitted_once: bool,
    last_emitted_added: i64,
    last_emitted_deleted: i64,
    last_emit_at: std::time::Instant,
    pending_emitted: bool,
    last_pending_added: i64,
    last_pending_deleted: i64,
    last_pending_at: std::time::Instant,
}

impl StreamingFileEditState {
    fn new() -> Self {
        let now = std::time::Instant::now();
        Self {
            key: String::new(),
            call_id: None,
            name: String::new(),
            arguments: String::new(),
            path: None,
            content: StreamingJsonStringField::new("content"),
            old_text: StreamingJsonStringField::new("old_text"),
            new_text: StreamingJsonStringField::new("new_text"),
            emitted_once: false,
            last_emitted_added: -1,
            last_emitted_deleted: -1,
            last_emit_at: now,
            pending_emitted: false,
            last_pending_added: -1,
            last_pending_deleted: -1,
            last_pending_at: now,
        }
    }

    fn apply_delta(&mut self, payload: &HashMap<String, Value>) {
        if let Some(call_id) = payload.get("call_id").and_then(|v| v.as_str()) {
            if !call_id.is_empty() {
                self.call_id = Some(call_id.to_string());
            }
        }
        if let Some(name) = payload.get("name").and_then(|v| v.as_str()) {
            if !name.is_empty() {
                self.name = name.to_string();
            }
        }

        if let Some(args) = payload.get("arguments").and_then(|v| v.as_str()) {
            self.arguments = args.to_string();
            self.content.reset();
            self.old_text.reset();
            self.new_text.reset();
            return;
        }

        if let Some(delta) = payload.get("arguments_delta").and_then(|v| v.as_str()) {
            if !delta.is_empty() {
                self.arguments.push_str(delta);
            }
        }
    }

    fn live_diff_counts(&mut self) -> (i64, i64) {
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

    fn should_emit(&self, added: i64, deleted: i64, now: std::time::Instant) -> bool {
        if !self.emitted_once {
            return true;
        }
        if added == self.last_emitted_added && deleted == self.last_emitted_deleted {
            return false;
        }
        let diff = (added - self.last_emitted_added)
            .abs()
            .max((deleted - self.last_emitted_deleted).abs());
        if diff >= LIVE_EMIT_LINE_STEP {
            return true;
        }
        now.duration_since(self.last_emit_at).as_secs_f64() >= LIVE_EMIT_INTERVAL_S
    }

    fn mark_emitted(&mut self, added: i64, deleted: i64, now: std::time::Instant) {
        self.emitted_once = true;
        self.last_emitted_added = added;
        self.last_emitted_deleted = deleted;
        self.last_emit_at = now;
    }

    fn should_emit_pending(&self, added: i64, deleted: i64, now: std::time::Instant) -> bool {
        if !self.pending_emitted {
            return true;
        }
        if added == self.last_pending_added && deleted == self.last_pending_deleted {
            return false;
        }
        let diff = (added - self.last_pending_added)
            .abs()
            .max((deleted - self.last_pending_deleted).abs());
        if diff >= LIVE_EMIT_LINE_STEP {
            return true;
        }
        now.duration_since(self.last_pending_at).as_secs_f64() >= LIVE_EMIT_INTERVAL_S
    }

    fn mark_pending_emitted(&mut self, added: i64, deleted: i64, now: std::time::Instant) {
        self.pending_emitted = true;
        self.last_pending_added = added;
        self.last_pending_deleted = deleted;
        self.last_pending_at = now;
    }
}

pub trait TODO_ToolResolver: Send + Sync {
    fn resolve_path(&self, raw_path: &str) -> Option<PathBuf>;
}

pub trait TODO_ToolCall: Send + Sync {
    fn id(&self) -> Option<&str>;
    fn name(&self) -> Option<&str>;
}
