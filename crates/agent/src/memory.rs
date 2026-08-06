//! Memory system: pure file I/O store + trait hooks for LLM-driven
//! consolidation and nightly "Dream" processing.
//!
//! Port of `nanobot.agent.memory`. The [`MemoryStore`] is a direct
//! translation; the heavyweight `Consolidator` and `Dream` classes are
//! exposed as traits so the agent crate stays provider-agnostic.

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use chrono::{DateTime, Local};
use log::warn;
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{Map, Value};

use utils::gitstore::GitStore;
use utils::helpers::{ensure_dir, strip_think, truncate_text};

const DEFAULT_MAX_HISTORY: usize = 1000;

const HISTORY_ENTRY_HARD_CAP: usize = 64_000;
const RAW_ARCHIVE_MAX_CHARS: usize = 16_000;
#[expect(dead_code, reason = "retained as the archive summary size policy")]
const ARCHIVE_SUMMARY_MAX_CHARS: usize = 8_000;

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
    oversize_logged: AtomicBool,
    corruption_logged: AtomicBool,
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
                "memory/.dream_cursor".into(),
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
            oversize_logged: AtomicBool::new(false),
            corruption_logged: AtomicBool::new(false),
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
    ///
    /// Entries are passed through `strip_think` to drop template-level leaks.
    /// A defensive cap (*max_chars*, default `HISTORY_ENTRY_HARD_CAP`) is applied
    /// as a final safety net.
    pub fn append_history(&self, entry: &str, max_chars: Option<usize>) -> std::io::Result<i64> {
        let limit = max_chars.unwrap_or(HISTORY_ENTRY_HARD_CAP);
        let cursor = self.next_cursor();
        let ts = Local::now().format("%Y-%m-%d %H:%M").to_string();
        let mut raw = entry.trim_end().to_string();
        if raw.len() > limit {
            if !self.oversize_logged.load(Ordering::Relaxed)
                && self
                    .oversize_logged
                    .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
                    .is_ok()
            {
                log::warn!(
                    "history entry exceeds {} chars ({}); truncating. Usually means a caller forgot its own cap; further occurrences suppressed.",
                    limit,
                    raw.len(),
                );
            }
            raw = truncate_text(&raw, limit);
        }
        let content = strip_think(&raw);
        if !raw.is_empty() && content.is_empty() {
            log::debug!(
                "history entry {cursor} stripped to empty (likely template leak); persisting empty content to avoid re-polluting context"
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
        if let Some(v) = std::fs::read_to_string(&self.cursor_file)
            .ok()
            .and_then(|s| s.trim().parse::<i64>().ok())
        {
            return v + 1;
        }
        self.warn_corrupted_entries();
        if let Some(c) = self
            .read_last_entry()
            .and_then(|last| valid_cursor(last.get("cursor")))
        {
            return c + 1;
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

    fn warn_corrupted_entries(&self) {
        let mut poisoned: Option<String> = None;
        for entry in self.read_entries() {
            if let Some(raw) = entry
                .get("cursor")
                .filter(|raw| valid_cursor(Some(raw)).is_none())
            {
                poisoned = Some(raw.to_string());
                break;
            }
        }
        if let Some(p) = poisoned.filter(|_| {
            !self.corruption_logged.load(Ordering::Relaxed)
                && self
                    .corruption_logged
                    .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
                    .is_ok()
        }) {
            log::warn!(
                "history.jsonl contains a non-int cursor ({}); dropping it. Usually caused by an external writer; further occurrences suppressed.",
                p,
            );
        }
    }

    fn read_entries(&self) -> Vec<Value> {
        let Ok(file) = File::open(&self.history_file) else {
            return Vec::new();
        };
        let reader = BufReader::new(file);
        reader
            .lines()
            .map_while(Result::ok)
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
        let tmp_path = self.history_file.with_extension("jsonl.tmp");
        let write_result = (|| -> std::io::Result<()> {
            let mut f = File::create(&tmp_path)?;
            for entry in entries {
                writeln!(f, "{}", serde_json::to_string(entry).unwrap())?;
            }
            f.flush()?;
            f.sync_all()?;
            std::fs::rename(&tmp_path, &self.history_file)?;
            Ok(())
        })();
        if write_result.is_err() {
            let _ = std::fs::remove_file(&tmp_path);
        }
        write_result
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
    pub fn raw_archive(&self, messages: &[Value], max_chars: Option<usize>) -> std::io::Result<()> {
        let limit = max_chars.unwrap_or(RAW_ARCHIVE_MAX_CHARS);
        let formatted = truncate_text(&Self::format_messages(messages), limit);
        let text = format!("[RAW] {} messages\n{formatted}", messages.len(),);
        self.append_history(&text, None)?;
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
            if let Some(cur) = entries
                .last()
                .and_then(|last| last.get("cursor"))
                .and_then(|v| v.as_i64())
            {
                let _ = std::fs::write(&self.cursor_file, cur.to_string());
                let _ = std::fs::write(&self.dream_cursor_file, cur.to_string());
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
            .unwrap_or_else(chrono::Utc::now);
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
            chunks.push(std::mem::take(&mut tmp).join("\n").trim().to_string());
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

    /// Hard-truncate an idle session under the consolidation lock.
    /// Returns the summary text on success, `None` if the LLM failed,
    /// or `Some("")` if there was nothing to archive.
    async fn compact_idle_session(&self, _session_key: &str, _max_suffix: usize) -> Option<String> {
        None
    }

    /// Update the provider and model used for consolidation.
    fn set_provider(
        &mut self,
        _provider: Arc<dyn LLMProvider>,
        _model: String,
        _context_window_tokens: u32,
    ) {
    }
}

/// Nightly memory processor. Mirrors `nanobot.agent.memory.Dream`.
#[async_trait]
pub trait Dream: Send + Sync {
    async fn run(&self) -> bool;
    fn set_provider(&mut self, provider: Arc<dyn LLMProvider>, model: String);
}

// ---------------------------------------------------------------------------
// MemoryDream — concrete implementation of the Dream trait
// ---------------------------------------------------------------------------

use std::collections::HashSet;
use std::sync::Arc;

use chrono::Datelike;
use log::{debug, info};
use serde_json::json;

use providers::{ChatRequest, LLMProvider, RetryMode};

use crate::runner::{AgentRunSpec, AgentRunner};
use crate::tools::{EditFileTool, FsTool, ReadFileTool, Tool, ToolRegistry, WriteFileTool};

const STALE_THRESHOLD_DAYS: i64 = 14;

const MEMORY_FILE_MAX_CHARS: usize = 32_000;
const SOUL_FILE_MAX_CHARS: usize = 16_000;
const USER_FILE_MAX_CHARS: usize = 16_000;
const HISTORY_ENTRY_PREVIEW_MAX_CHARS: usize = 4_000;

static VAR_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\{\{\s*(\w+)\s*\}\}").unwrap());

fn render_template_dream(template: &str, pairs: &[(&str, String)]) -> String {
    VAR_RE
        .replace_all(template, |caps: &regex::Captures| {
            let key = &caps[1];
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v.clone())
                .unwrap_or_else(|| caps[0].to_string())
        })
        .into_owned()
}

#[derive(Debug, Clone)]
pub struct DreamConfig {
    pub max_batch_size: usize,
    pub max_iterations: u32,
    pub max_tool_result_chars: usize,
    pub annotate_line_ages: bool,
}

impl Default for DreamConfig {
    fn default() -> Self {
        Self {
            max_batch_size: 20,
            max_iterations: 10,
            max_tool_result_chars: 16_000,
            annotate_line_ages: true,
        }
    }
}

pub struct MemoryDream {
    store: MemoryStore,
    provider: Arc<dyn LLMProvider>,
    runner: AgentRunner,
    model: String,
    cfg: DreamConfig,
    tools: ToolRegistry,
    skill_creator_path: PathBuf,
}

impl MemoryDream {
    pub fn new(
        workspace: PathBuf,
        provider: Arc<dyn LLMProvider>,
        model: String,
        cfg: DreamConfig,
    ) -> Self {
        let store = MemoryStore::new(&workspace, None);
        let runner = AgentRunner::new(provider.clone());
        let skill_creator_path = workspace
            .join(".nanobot")
            .join("cache")
            .join("skill-creator-reference.md");
        Self {
            store,
            provider,
            runner,
            model,
            cfg,
            tools: ToolRegistry::new(),
            skill_creator_path,
        }
    }

    pub async fn initialize(&self) -> std::io::Result<()> {
        let workspace = self.store.workspace.clone();
        let skills_dir = workspace.join("skills");
        std::fs::create_dir_all(&skills_dir)?;

        let read_tool = ReadFileTool(FsTool::new(
            Some(workspace.clone()),
            Some(workspace.clone()),
            Vec::new(),
        ));
        let edit_tool = EditFileTool(FsTool::new(
            Some(workspace.clone()),
            Some(workspace.clone()),
            Vec::new(),
        ));
        let write_tool = WriteFileTool(FsTool::new(
            Some(workspace.clone()),
            Some(skills_dir),
            Vec::new(),
        ));

        self.tools
            .register(Arc::new(read_tool) as Arc<dyn Tool>)
            .await;
        self.tools
            .register(Arc::new(edit_tool) as Arc<dyn Tool>)
            .await;
        self.tools
            .register(Arc::new(write_tool) as Arc<dyn Tool>)
            .await;

        if let Some(parent) = self.skill_creator_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Some(content) = skills::skill_manifest("skill-creator") {
            let _ = std::fs::write(&self.skill_creator_path, content);
        }
        Ok(())
    }

    fn list_existing_skills(&self) -> Vec<String> {
        let mut entries: Vec<(String, String)> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        let desc_re = Regex::new(r"(?m)^description:\s*(.+)$").unwrap();

        let workspace_skills = self.store.workspace.join("skills");
        if let Ok(rd) = std::fs::read_dir(&workspace_skills) {
            for entry in rd.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                let skill_md = path.join("SKILL.md");
                if !skill_md.exists() {
                    continue;
                }
                let head = std::fs::read_to_string(&skill_md).unwrap_or_default();
                let head = head.chars().take(500).collect::<String>();
                let desc = desc_re
                    .captures(&head)
                    .and_then(|c| c.get(1))
                    .map(|m| m.as_str().trim().to_string())
                    .unwrap_or_else(|| "(no description)".to_string());
                seen.insert(name.to_string());
                entries.push((name.to_string(), desc));
            }
        }

        for name in skills::skill_names() {
            if seen.contains(name) {
                continue;
            }
            let manifest = skills::skill_manifest(name).unwrap_or_default();
            let head = manifest.chars().take(500).collect::<String>();
            let desc = desc_re
                .captures(&head)
                .and_then(|c| c.get(1))
                .map(|m| m.as_str().trim().to_string())
                .unwrap_or_else(|| "(no description)".to_string());
            entries.push((name.to_string(), desc));
        }

        entries.sort_by(|a, b| a.0.cmp(&b.0));
        entries
            .into_iter()
            .map(|(n, d)| format!("{n} — {d}"))
            .collect()
    }

    fn annotate_with_ages(&self, content: &str) -> String {
        let ages = self.store.git().line_ages("memory/MEMORY.md");
        if ages.is_empty() {
            return content.to_string();
        }
        let had_trailing = content.ends_with('\n');
        let lines: Vec<&str> = content.split('\n').collect();
        let lines: Vec<&str> =
            if had_trailing && lines.last().map(|s| s.is_empty()).unwrap_or(false) {
                lines[..lines.len() - 1].to_vec()
            } else {
                lines
            };
        if lines.len() != ages.len() {
            debug!(
                "line_ages length mismatch for memory/MEMORY.md (lines={}, ages={}); skipping annotation",
                lines.len(),
                ages.len()
            );
            return content.to_string();
        }
        let mut annotated: Vec<String> = Vec::with_capacity(lines.len());
        for (line, age) in lines.iter().zip(ages.iter()) {
            if line.trim().is_empty() {
                annotated.push(line.to_string());
            } else if age.age_days > STALE_THRESHOLD_DAYS {
                annotated.push(format!("{line}  \u{2190} {}d", age.age_days));
            } else {
                annotated.push(line.to_string());
            }
        }
        let mut result = annotated.join("\n");
        if had_trailing {
            result.push('\n');
        }
        result
    }
}

impl MemoryDream {
    /// Update the provider and model used by Dream for memory consolidation.
    pub fn set_provider(&mut self, provider: Arc<dyn LLMProvider>, model: String) {
        self.provider = provider.clone();
        self.model = model;
        self.runner = AgentRunner::new(provider);
    }
}

#[async_trait]
impl Dream for MemoryDream {
    async fn run(&self) -> bool {
        let last_cursor = self.store.last_dream_cursor();
        let entries = self.store.read_unprocessed_history(last_cursor);
        if entries.is_empty() {
            return false;
        }

        let take = entries.len().min(self.cfg.max_batch_size);
        let batch = &entries[..take];

        let last_cursor_in_batch = batch
            .last()
            .and_then(|e| e.get("cursor"))
            .and_then(|v| v.as_i64())
            .unwrap_or(last_cursor);
        info!(
            "Dream: processing {} entries (cursor {}→{}), batch={}",
            entries.len(),
            last_cursor,
            last_cursor_in_batch,
            batch.len()
        );

        let history_text = batch
            .iter()
            .map(|e| {
                let ts = e.get("timestamp").and_then(|v| v.as_str()).unwrap_or("?");
                let content = e.get("content").and_then(|v| v.as_str()).unwrap_or("");
                format!(
                    "[{ts}] {}",
                    truncate_text(content, HISTORY_ENTRY_PREVIEW_MAX_CHARS)
                )
            })
            .collect::<Vec<_>>()
            .join("\n");

        let now = Local::now();
        let current_date = format!("{:04}-{:02}-{:02}", now.year(), now.month(), now.day());

        let raw_memory = self.store.read_memory();
        let raw_memory = if raw_memory.is_empty() {
            "(empty)".to_string()
        } else {
            raw_memory
        };
        let annotated_memory = if self.cfg.annotate_line_ages {
            self.annotate_with_ages(&raw_memory)
        } else {
            raw_memory.clone()
        };
        let current_memory = truncate_text(&annotated_memory, MEMORY_FILE_MAX_CHARS);

        let raw_soul = self.store.read_soul();
        let current_soul = truncate_text(
            if raw_soul.is_empty() {
                "(empty)"
            } else {
                &raw_soul
            },
            SOUL_FILE_MAX_CHARS,
        );
        let raw_user = self.store.read_user();
        let current_user = truncate_text(
            if raw_user.is_empty() {
                "(empty)"
            } else {
                &raw_user
            },
            USER_FILE_MAX_CHARS,
        );

        let file_context = format!(
            "## Current Date\n{current_date}\n\n\
             ## Current MEMORY.md ({mlen} chars)\n{current_memory}\n\n\
             ## Current SOUL.md ({slen} chars)\n{current_soul}\n\n\
             ## Current USER.md ({ulen} chars)\n{current_user}",
            mlen = current_memory.chars().count(),
            slen = current_soul.chars().count(),
            ulen = current_user.chars().count(),
        );

        let phase1_system = render_template_dream(
            include_str!("prompts/dream_phase1.md.tpl"),
            &[("stale_threshold_days", STALE_THRESHOLD_DAYS.to_string())],
        );
        let phase1_user = format!("## Conversation History\n{history_text}\n\n{file_context}");

        let phase1_response = self
            .provider
            .chat_with_retry(
                ChatRequest {
                    messages: vec![
                        json!({"role":"system","content": phase1_system}),
                        json!({"role":"user","content": phase1_user}),
                    ],
                    tools: None,
                    model: Some(self.model.clone()),
                    max_tokens: 0,
                    temperature: f32::NAN,
                    reasoning_effort: None,
                    tool_choice: None,
                },
                RetryMode::Standard,
                None,
            )
            .await;

        if phase1_response.finish_reason == "error" {
            warn!(
                "Dream Phase 1 failed: {}",
                phase1_response.content.as_deref().unwrap_or("<no detail>")
            );
            return false;
        }

        let analysis = phase1_response.content.unwrap_or_default();
        let preview: String = analysis.chars().take(500).collect();
        debug!(
            "Dream Phase 1 analysis ({} chars): {}",
            analysis.chars().count(),
            preview
        );

        let existing_skills = self.list_existing_skills();
        let skills_section = if existing_skills.is_empty() {
            String::new()
        } else {
            let body: String = existing_skills
                .into_iter()
                .map(|s| format!("- {s}"))
                .collect::<Vec<_>>()
                .join("\n");
            format!("\n\n## Existing Skills\n{body}")
        };
        let phase2_system = render_template_dream(
            include_str!("prompts/dream_phase2.md.tpl"),
            &[(
                "skill_creator_path",
                self.skill_creator_path.to_string_lossy().into_owned(),
            )],
        );
        let phase2_user =
            format!("## Analysis Result\n{analysis}\n\n{file_context}{skills_section}");
        let messages = vec![
            json!({"role":"system","content": phase2_system}),
            json!({"role":"user","content": phase2_user}),
        ];

        let mut spec = AgentRunSpec::new(
            messages,
            self.tools.clone(),
            self.model.clone(),
            self.cfg.max_iterations,
            self.cfg.max_tool_result_chars,
        );
        spec.workspace = Some(self.store.workspace.clone());
        spec.session_key = Some("system::dream".into());
        spec.fail_on_tool_error = false;

        let result = self.runner.run(spec).await;

        debug!(
            "Dream Phase 2 complete: stop_reason={}, tool_events={}",
            result.stop_reason,
            result.tool_events.len()
        );
        for ev in &result.tool_events {
            let detail: String = ev.detail.chars().take(200).collect();
            info!(
                "Dream tool_event: name={}, status={}, detail={}",
                ev.name, ev.status, detail
            );
        }

        let changelog: Vec<String> = result
            .tool_events
            .iter()
            .filter(|e| e.status == "ok")
            .map(|e| format!("{}: {}", e.name, e.detail))
            .collect();

        let new_cursor = last_cursor_in_batch;

        if result.stop_reason == "completed" {
            let _ = self.store.set_last_dream_cursor(new_cursor);
            info!(
                "Dream done: {} change(s), cursor advanced to {}",
                changelog.len(),
                new_cursor
            );
        } else {
            warn!(
                "Dream incomplete ({}): cursor NOT advanced, will retry next cron cycle",
                result.stop_reason
            );
        }

        let _ = self.store.compact_history();

        if !changelog.is_empty() && self.store.git().is_initialized() {
            let ts = batch
                .last()
                .and_then(|e| e.get("timestamp"))
                .and_then(|v| v.as_str())
                .unwrap_or("?")
                .to_string();
            let summary = format!("dream: {ts}, {} change(s)", changelog.len());
            let commit_msg = format!("{summary}\n\n{}", analysis.trim());
            if let Some(sha) = self.store.git().auto_commit(&commit_msg) {
                info!("Dream commit: {sha}");
            }
        }

        true
    }

    fn set_provider(&mut self, provider: Arc<dyn LLMProvider>, model: String) {
        self.provider = provider.clone();
        self.model = model;
        self.runner = AgentRunner::new(provider);
    }
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
        let c1 = store.append_history("hello", None).unwrap();
        let c2 = store.append_history("world", None).unwrap();
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
