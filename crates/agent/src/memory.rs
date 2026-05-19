//! Memory system: pure file I/O store + trait hooks for LLM-driven
//! consolidation and nightly "Dream" processing.
//!
//! Port of `nanobot.agent.memory`. The [`MemoryStore`] is a direct
//! translation; the heavyweight `Consolidator` and `Dream` classes are
//! exposed as traits so the agent crate stays provider-agnostic.

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use chrono::{DateTime, Local};
use log::warn;
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{Map, Value};

use utils::gitstore::GitStore;
use utils::helpers::{ensure_dir, strip_think};

const DEFAULT_MAX_HISTORY: usize = 1000;

static LEGACY_ENTRY_START: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^\[(\d{4}-\d{2}-\d{2}[^\]]*)\]\s*").unwrap());
static LEGACY_TIMESTAMP: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^\[(\d{4}-\d{2}-\d{2} \d{2}:\d{2})\]\s*").unwrap());
static LEGACY_RAW_MESSAGE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^\[\d{4}-\d{2}-\d{2}[^\]]*\]\s+[A-Z][A-Z0-9_]*(?:\s+\[tools:\s*[^\]]+\])?:")
        .unwrap()
});

/// Pure file I/O for memory files: MEMORY.md, history.jsonl, SOUL.md, USER.md.
pub struct MemoryStore {
    pub workspace: PathBuf,
    pub max_history_entries: usize,
    pub memory_dir: PathBuf,
    pub memory_file: PathBuf,
    pub history_file: PathBuf,
    pub legacy_history_file: PathBuf,
    pub soul_file: PathBuf,
    pub user_file: PathBuf,
    cursor_file: PathBuf,
    dream_cursor_file: PathBuf,
    git: GitStore,
}

impl MemoryStore {
    pub fn new(workspace: &Path, max_history_entries: Option<usize>) -> Self {
        let memory_dir =
            ensure_dir(&workspace.join("memory")).unwrap_or_else(|_| workspace.join("memory"));
        let memory_file = memory_dir.join("MEMORY.md");
        let history_file = memory_dir.join("history.jsonl");
        let legacy_history_file = memory_dir.join("HISTORY.md");
        let soul_file = workspace.join("SOUL.md");
        let user_file = workspace.join("USER.md");
        let cursor_file = memory_dir.join(".cursor");
        let dream_cursor_file = memory_dir.join(".dream_cursor");
        let git = GitStore::new(
            workspace.to_path_buf(),
            vec![
                "SOUL.md".into(),
                "USER.md".into(),
                "memory/MEMORY.md".into(),
            ],
        );
        let this = Self {
            workspace: workspace.to_path_buf(),
            max_history_entries: max_history_entries.unwrap_or(DEFAULT_MAX_HISTORY),
            memory_dir,
            memory_file,
            history_file,
            legacy_history_file,
            soul_file,
            user_file,
            cursor_file,
            dream_cursor_file,
            git,
        };
        this.maybe_migrate_legacy_history();
        this
    }

    pub fn git(&self) -> &GitStore {
        &self.git
    }

    // -- generic helpers -----------------------------------------------------

    pub fn read_file(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap_or_default()
    }

    // -- MEMORY.md / SOUL.md / USER.md ---------------------------------------

    pub fn read_memory(&self) -> String {
        Self::read_file(&self.memory_file)
    }
    pub fn write_memory(&self, content: &str) -> std::io::Result<()> {
        std::fs::write(&self.memory_file, content)
    }
    pub fn read_soul(&self) -> String {
        Self::read_file(&self.soul_file)
    }
    pub fn write_soul(&self, content: &str) -> std::io::Result<()> {
        std::fs::write(&self.soul_file, content)
    }
    pub fn read_user(&self) -> String {
        Self::read_file(&self.user_file)
    }
    pub fn write_user(&self, content: &str) -> std::io::Result<()> {
        std::fs::write(&self.user_file, content)
    }

    /// Build the "memory context" block that gets injected into the system prompt.
    pub fn memory_context(&self) -> String {
        let long_term = self.read_memory();
        if long_term.is_empty() {
            String::new()
        } else {
            format!("## Long-term Memory\n{long_term}")
        }
    }

    // -- history.jsonl -------------------------------------------------------

    /// Append `entry` to history.jsonl, returning the auto-incrementing cursor.
    pub fn append_history(&self, entry: &str) -> std::io::Result<i64> {
        let cursor = self.next_cursor();
        let ts = Local::now().format("%Y-%m-%d %H:%M").to_string();
        let raw = entry.trim_end().to_string();
        let content = strip_think(&raw);
        if !raw.is_empty() && content.is_empty() {
            log::debug!(
                "history entry {cursor} stripped to empty (likely template leak); persisting empty"
            );
        }
        let record = serde_json::json!({
            "cursor": cursor,
            "timestamp": ts,
            "content": content,
        });
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.history_file)?;
        writeln!(f, "{}", serde_json::to_string(&record).unwrap())?;
        std::fs::write(&self.cursor_file, cursor.to_string())?;
        Ok(cursor)
    }

    fn next_cursor(&self) -> i64 {
        if let Ok(s) = std::fs::read_to_string(&self.cursor_file) {
            if let Ok(v) = s.trim().parse::<i64>() {
                return v + 1;
            }
        }
        if let Some(last) = self.read_last_entry() {
            if let Some(c) = valid_cursor(last.get("cursor")) {
                return c + 1;
            }
        }
        self.iter_valid_entries().map(|(_, c)| c).max().unwrap_or(0) + 1
    }

    pub fn read_unprocessed_history(&self, since_cursor: i64) -> Vec<Value> {
        self.iter_valid_entries()
            .filter(|(_, c)| *c > since_cursor)
            .map(|(e, _)| e)
            .collect()
    }

    pub fn compact_history(&self) -> std::io::Result<()> {
        if self.max_history_entries == 0 {
            return Ok(());
        }
        let entries = self.read_entries();
        if entries.len() <= self.max_history_entries {
            return Ok(());
        }
        let start = entries.len() - self.max_history_entries;
        let kept: Vec<Value> = entries.into_iter().skip(start).collect();
        self.write_entries(&kept)
    }

    fn iter_valid_entries(&self) -> impl Iterator<Item = (Value, i64)> + '_ {
        self.read_entries().into_iter().filter_map(|entry| {
            let raw = entry.get("cursor")?;
            let c = valid_cursor(Some(raw))?;
            Some((entry, c))
        })
    }

    fn read_entries(&self) -> Vec<Value> {
        let Ok(file) = File::open(&self.history_file) else {
            return Vec::new();
        };
        let reader = BufReader::new(file);
        reader
            .lines()
            .flatten()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| serde_json::from_str::<Value>(&l).ok())
            .collect()
    }

    fn read_last_entry(&self) -> Option<Value> {
        let mut file = File::open(&self.history_file).ok()?;
        let size = file.seek(SeekFrom::End(0)).ok()?;
        if size == 0 {
            return None;
        }
        let read_size = size.min(4096);
        file.seek(SeekFrom::End(-(read_size as i64))).ok()?;
        let mut buf = vec![0u8; read_size as usize];
        use std::io::Read;
        file.read_exact(&mut buf).ok()?;
        let text = String::from_utf8(buf).ok()?;
        let last_line = text.lines().rev().find(|l| !l.trim().is_empty())?;
        serde_json::from_str(last_line).ok()
    }

    fn write_entries(&self, entries: &[Value]) -> std::io::Result<()> {
        let mut f = File::create(&self.history_file)?;
        for entry in entries {
            writeln!(f, "{}", serde_json::to_string(entry).unwrap())?;
        }
        Ok(())
    }

    // -- dream cursor --------------------------------------------------------

    pub fn last_dream_cursor(&self) -> i64 {
        std::fs::read_to_string(&self.dream_cursor_file)
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0)
    }

    pub fn set_last_dream_cursor(&self, cursor: i64) -> std::io::Result<()> {
        std::fs::write(&self.dream_cursor_file, cursor.to_string())
    }

    // -- message formatting utility ------------------------------------------

    pub fn format_messages(messages: &[Value]) -> String {
        let mut lines = Vec::new();
        for m in messages {
            let content_opt = m
                .get("content")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty());
            let Some(content) = content_opt else {
                continue;
            };
            let tools = m
                .get("tools_used")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    let names: Vec<String> = arr
                        .iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect();
                    format!(" [tools: {}]", names.join(", "))
                })
                .unwrap_or_default();
            let ts = m
                .get("timestamp")
                .and_then(|v| v.as_str())
                .unwrap_or("?")
                .chars()
                .take(16)
                .collect::<String>();
            let role = m.get("role").and_then(|v| v.as_str()).unwrap_or("");
            lines.push(format!(
                "[{ts}] {}{tools}: {content}",
                role.to_ascii_uppercase()
            ));
        }
        lines.join("\n")
    }

    /// Fallback: dump raw messages to history.jsonl without LLM summarization.
    pub fn raw_archive(&self, messages: &[Value]) -> std::io::Result<()> {
        let text = format!(
            "[RAW] {} messages\n{}",
            messages.len(),
            Self::format_messages(messages)
        );
        self.append_history(&text)?;
        warn!(
            "Memory consolidation degraded: raw-archived {} messages",
            messages.len()
        );
        Ok(())
    }

    // -- legacy migration ----------------------------------------------------

    fn maybe_migrate_legacy_history(&self) {
        if !self.legacy_history_file.exists() {
            return;
        }
        if self.history_file.exists()
            && self.history_file.metadata().map(|m| m.len()).unwrap_or(0) > 0
        {
            return;
        }

        let Ok(legacy_text) = std::fs::read_to_string(&self.legacy_history_file) else {
            log::warn!("Failed to read legacy HISTORY.md for migration");
            return;
        };
        let entries = self.parse_legacy_history(&legacy_text);
        if !entries.is_empty() {
            let _ = self.write_entries(&entries);
            if let Some(last) = entries.last() {
                if let Some(cur) = last.get("cursor").and_then(|v| v.as_i64()) {
                    let _ = std::fs::write(&self.cursor_file, cur.to_string());
                    let _ = std::fs::write(&self.dream_cursor_file, cur.to_string());
                }
            }
        }
        let backup = self.next_legacy_backup_path();
        let _ = std::fs::rename(&self.legacy_history_file, backup);
        log::info!(
            "Migrated legacy HISTORY.md to history.jsonl ({} entries)",
            entries.len()
        );
    }

    fn parse_legacy_history(&self, text: &str) -> Vec<Value> {
        let normalized = text
            .replace("\r\n", "\n")
            .replace('\r', "\n")
            .trim()
            .to_string();
        if normalized.is_empty() {
            return Vec::new();
        }
        let fallback_timestamp = self.legacy_fallback_timestamp();
        let chunks = split_legacy_history_chunks(&normalized);
        chunks
            .into_iter()
            .enumerate()
            .map(|(i, chunk)| {
                let cursor = (i as i64) + 1;
                let mut timestamp = fallback_timestamp.clone();
                let mut content = chunk.clone();
                if let Some(m) = LEGACY_TIMESTAMP.captures(&chunk) {
                    timestamp = m.get(1).unwrap().as_str().to_string();
                    let remainder = chunk[m.get(0).unwrap().end()..].trim_start().to_string();
                    if !remainder.is_empty() {
                        content = remainder;
                    }
                }
                let mut rec = Map::new();
                rec.insert("cursor".into(), Value::from(cursor));
                rec.insert("timestamp".into(), Value::String(timestamp));
                rec.insert("content".into(), Value::String(content));
                Value::Object(rec)
            })
            .collect()
    }

    fn legacy_fallback_timestamp(&self) -> String {
        let now = self
            .legacy_history_file
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .and_then(|s| DateTime::from_timestamp(s, 0))
            .unwrap_or_else(|| chrono::Utc::now());
        now.with_timezone(&Local)
            .format("%Y-%m-%d %H:%M")
            .to_string()
    }

    fn next_legacy_backup_path(&self) -> PathBuf {
        let mut candidate = self.memory_dir.join("HISTORY.md.bak");
        let mut suffix = 2;
        while candidate.exists() {
            candidate = self.memory_dir.join(format!("HISTORY.md.bak.{suffix}"));
            suffix += 1;
        }
        candidate
    }
}

fn valid_cursor(v: Option<&Value>) -> Option<i64> {
    match v? {
        Value::Number(n) => n.as_i64(),
        _ => None,
    }
}

fn split_legacy_history_chunks(text: &str) -> Vec<String> {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut chunks: Vec<String> = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    let mut saw_blank = false;

    let flush = |chunks: &mut Vec<String>, current: &mut Vec<&str>| {
        if !current.is_empty() {
            chunks.push(current.join("\n").trim().to_string());
            current.clear();
        }
    };

    for line in lines {
        if saw_blank && !line.trim().is_empty() && !current.is_empty() {
            let mut tmp = current.clone();
            let _ = std::mem::replace(&mut current, vec![line]);
            // flush previous
            chunks.push(
                tmp.drain(..)
                    .collect::<Vec<_>>()
                    .join("\n")
                    .trim()
                    .to_string(),
            );
            saw_blank = false;
            continue;
        }

        let start_new = should_start_new_legacy_chunk(line, &current);
        if start_new {
            let tmp = current.clone();
            current = vec![line];
            chunks.push(tmp.join("\n").trim().to_string());
            saw_blank = false;
            continue;
        }
        current.push(line);
        saw_blank = line.trim().is_empty();
    }

    let mut tmp = current.clone();
    flush(&mut chunks, &mut tmp);
    chunks.retain(|c| !c.is_empty());
    chunks
}

fn should_start_new_legacy_chunk(line: &str, current: &[&str]) -> bool {
    if current.is_empty() {
        return false;
    }
    if !LEGACY_ENTRY_START.is_match(line) {
        return false;
    }
    if is_raw_legacy_chunk(current) && LEGACY_RAW_MESSAGE.is_match(line) {
        return false;
    }
    true
}

fn is_raw_legacy_chunk(lines: &[&str]) -> bool {
    let first = lines
        .iter()
        .find(|l| !l.trim().is_empty())
        .copied()
        .unwrap_or("");
    let Some(m) = LEGACY_TIMESTAMP.captures(first) else {
        return false;
    };
    first[m.get(0).unwrap().end()..]
        .trim_start()
        .starts_with("[RAW]")
}

// ---------------------------------------------------------------------------
// Consolidator & Dream — trait stubs so the agent compiles without forcing
// an LLM dependency into this crate. Concrete implementations live downstream.
// ---------------------------------------------------------------------------

/// Result of estimating the prompt tokens for a session.
#[derive(Debug, Clone)]
pub struct PromptSizeEstimate {
    pub tokens: usize,
    pub source: String,
}

/// Hook for LLM-driven consolidation. Mirrors
/// `nanobot.agent.memory.Consolidator`.
#[async_trait]
pub trait Consolidator: Send + Sync {
    /// Summarize the given messages (returns `None` if nothing to archive).
    async fn archive(&self, messages: Vec<Value>) -> Option<String>;

    /// Estimate current prompt size for the session. Default: 0 tokens.
    async fn estimate_session_prompt_tokens(
        &self,
        _session_key: &str,
        _history_len: usize,
    ) -> PromptSizeEstimate {
        PromptSizeEstimate {
            tokens: 0,
            source: "noop".into(),
        }
    }

    /// Attempt token-based consolidation if the session exceeds its budget.
    /// Default: no-op.
    async fn maybe_consolidate_by_tokens(&self, _session_key: &str) {}
}

/// Nightly memory processor. Mirrors `nanobot.agent.memory.Dream`.
#[async_trait]
pub trait Dream: Send + Sync {
    /// Process unprocessed history entries. Returns `true` when work was done.
    async fn run(&self) -> bool;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn tmp() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("memstore-{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn append_and_read_unprocessed() {
        let ws = tmp();
        let store = MemoryStore::new(&ws, None);
        let c1 = store.append_history("hello").unwrap();
        let c2 = store.append_history("world").unwrap();
        assert_eq!(c2, c1 + 1);
        let entries = store.read_unprocessed_history(c1);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["content"], "world");
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn memory_context_empty_when_file_missing() {
        let ws = tmp();
        let store = MemoryStore::new(&ws, None);
        assert!(store.memory_context().is_empty());
        let _ = std::fs::remove_dir_all(&ws);
    }
}
