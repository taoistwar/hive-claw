//! Session management for conversation history (port of
//! `nanobot.session.manager`).

use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Local};
use log::{info, warn};
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use config::{ensure_dir, get_legacy_sessions_dir};
use utils::helpers::{find_legal_message_start, image_placeholder_text, safe_filename};

const FILE_MAX_MESSAGES: usize = 2000;
const SESSION_PREVIEW_MAX_CHARS: usize = 120;

static MESSAGE_TIME_PREFIX_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^\[Message Time: [^\]]+\]\n?").unwrap());
static LOCAL_IMAGE_BREADCRUMB_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^\[image: (?:/|~)[^\]]+\]\s*$").unwrap());
static TOOL_CALL_ECHO_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^\s*(?:generate_image|message)\([^)]*\)\s*$").unwrap());

/// Remove internal replay artifacts that the model may have copied before.
fn sanitize_assistant_replay_text(content: &str) -> String {
    let content = MESSAGE_TIME_PREFIX_RE.replace(content, "");
    let lines: Vec<&str> = content
        .lines()
        .filter(|line| {
            !LOCAL_IMAGE_BREADCRUMB_RE.is_match(line) && !TOOL_CALL_ECHO_RE.is_match(line)
        })
        .collect();
    lines.join("\n").trim().to_string()
}

/// Return compact display text for session lists.
fn text_preview(content: &Value) -> String {
    let text = if let Some(s) = content.as_str() {
        s.to_string()
    } else if let Some(blocks) = content.as_array() {
        let parts: Vec<String> = blocks
            .iter()
            .filter_map(|block| {
                if let Some(obj) = block.as_object() {
                    if obj.get("type").and_then(|v| v.as_str()) == Some("text") {
                        return obj.get("text").and_then(|v| v.as_str()).map(String::from);
                    }
                }
                None
            })
            .collect();
        parts.join(" ")
    } else {
        return String::new();
    };
    let text = sanitize_assistant_replay_text(&text);
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() > SESSION_PREVIEW_MAX_CHARS {
        let truncated: String = text.chars().take(SESSION_PREVIEW_MAX_CHARS - 1).collect();
        format!("{}…", truncated.trim_end())
    } else {
        text
    }
}

/// Return display text for a message, scrubbing subagent announce bodies.
fn message_preview_text(msg: &Value) -> String {
    let mut content = msg.get("content").cloned().unwrap_or(Value::Null);
    if msg.get("injected_event").and_then(|v| v.as_str()) == Some("subagent_result") {
        if let Some(s) = content.as_str() {
            content = Value::String(utils::subagent_channel_display::scrub_subagent_announce_body(s));
        }
    }
    text_preview(&content)
}

/// Prepend `[Message Time: ...]` to user messages for relative-date reasoning.
fn annotate_message_time(content: &str) -> String {
    let now = Local::now();
    let time_str = now.format("%Y-%m-%d %H:%M:%S %Z");
    format!("[Message Time: {}]\n{}", time_str, content)
}

/// Rough estimate of token count for a message (4 chars ≈ 1 token for English).
fn estimate_tokens_for_message(msg: &Value) -> usize {
    let content = msg
        .get("content")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let base = content.len() / 4;
    let tool_calls = msg
        .get("tool_calls")
        .and_then(|v| v.as_array())
        .map(|a| a.len() * 20)
        .unwrap_or(0);
    let overhead = 10;
    base + tool_calls + overhead
}

/// A conversation session (one per `channel:chat_id`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub key: String,
    #[serde(default)]
    pub messages: Vec<Value>,
    #[serde(default = "now")]
    pub created_at: DateTime<Local>,
    #[serde(default = "now")]
    pub updated_at: DateTime<Local>,
    #[serde(default)]
    pub metadata: HashMap<String, Value>,
    /// Number of messages already consolidated to files.
    #[serde(default)]
    pub last_consolidated: usize,
}

fn now() -> DateTime<Local> {
    Local::now()
}

impl Session {
    pub fn new(key: impl Into<String>) -> Self {
        let n = now();
        Self {
            key: key.into(),
            messages: Vec::new(),
            created_at: n,
            updated_at: n,
            metadata: HashMap::new(),
            last_consolidated: 0,
        }
    }

    /// Add a message to the session.
    ///
    /// Extra keys (e.g. `tool_calls`, `tool_call_id`, `media`) should be
    /// passed via the `extra` map; they are merged into the stored message
    /// alongside the required `role` / `content` fields.
    pub fn add_message(&mut self, role: &str, content: &str, extra: HashMap<String, Value>) {
        let mut msg = serde_json::Map::new();
        msg.insert("role".into(), Value::String(role.into()));
        msg.insert("content".into(), Value::String(content.to_string()));
        msg.insert(
            "timestamp".into(),
            Value::String(Local::now().to_rfc3339()),
        );
        for (k, v) in extra {
            msg.insert(k, v);
        }
        self.messages.push(Value::Object(msg));
        self.updated_at = now();
    }

    /// Return unconsolidated messages for LLM input, aligned to a legal
    /// tool-call boundary.
    pub fn get_history(&self, max_messages: usize) -> Vec<Value> {
        self.get_history_with_options(max_messages, None, false)
    }

    /// Return unconsolidated messages for LLM input with options.
    /// - `max_tokens`: If set, truncate history when estimated token budget is exceeded.
    /// - `include_timestamps`: Whether to include timestamp fields in the output.
    pub fn get_history_with_options(
        &self,
        max_messages: usize,
        max_tokens: Option<usize>,
        include_timestamps: bool,
    ) -> Vec<Value> {
        let max_messages = if max_messages > 0 { max_messages } else { 120 };
        let unconsolidated = &self.messages[self.last_consolidated.min(self.messages.len())..];
        let sliced: Vec<&Value> = if unconsolidated.len() > max_messages {
            unconsolidated[unconsolidated.len() - max_messages..]
                .iter()
                .collect()
        } else {
            unconsolidated.iter().collect()
        };

        // Avoid starting mid-turn when possible, except for proactive
        // assistant deliveries that the user may be replying to.
        let mut start = 0usize;
        for (i, m) in sliced.iter().enumerate() {
            if m.get("role").and_then(Value::as_str) == Some("user") {
                start = i;
                if i > 0 {
                    if let Some(prev) = sliced.get(i - 1) {
                        if prev.get("_channel_delivery").is_some() {
                            start = i - 1;
                        }
                    }
                }
                break;
            }
        }
        let mut sliced: Vec<Value> = sliced[start..].iter().map(|v| (*v).clone()).collect();

        // Drop orphan tool results at the front.
        let legal_start = find_legal_message_start(&sliced);
        if legal_start > 0 {
            sliced = sliced.split_off(legal_start);
        }

        // Skip messages with _command flag.
        sliced.retain(|m| !m.get("_command").and_then(|v| v.as_bool()).unwrap_or(false));

        // Filter empty assistant messages.
        sliced.retain(|m| {
            if m.get("role").and_then(Value::as_str) != Some("assistant") {
                return true;
            }
            let has_content = m
                .get("content")
                .and_then(|v| v.as_str())
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false);
            let has_tool_calls = m.get("tool_calls").map(|v| !v.is_null()).unwrap_or(false);
            let has_reasoning = m
                .get("reasoning_content")
                .and_then(|v| v.as_str())
                .map(|s| !s.is_empty())
                .unwrap_or(false);
            let has_thinking = m
                .get("thinking_blocks")
                .map(|v| v.as_array().map(|a| !a.is_empty()).unwrap_or(false))
                .unwrap_or(false);
            has_content || has_tool_calls || has_reasoning || has_thinking
        });

        let mut out: Vec<Value> = Vec::with_capacity(sliced.len());

        for message in &sliced {
            let role = message.get("role").and_then(|v| v.as_str()).unwrap_or("");
            let mut content = message.get("content").cloned().unwrap_or(Value::String(String::new()));

            // Sanitize assistant replay text.
            if role == "assistant" {
                if let Some(s) = content.as_str() {
                    content = Value::String(sanitize_assistant_replay_text(s));
                }
            }

            // Synthesize image breadcrumbs from persisted media kwarg.
            if role == "user" {
                if let Some(media) = message.get("media").and_then(Value::as_array) {
                    if !media.is_empty() {
                        if let Some(text) = content.as_str() {
                            let breadcrumbs: Vec<String> = media
                                .iter()
                                .filter_map(|v| v.as_str().filter(|s| !s.is_empty()))
                                .map(|p| image_placeholder_text(Some(p)))
                                .collect();
                            let merged = if text.is_empty() {
                                breadcrumbs.join("\n")
                            } else {
                                format!("{text}\n{}", breadcrumbs.join("\n"))
                            };
                            content = Value::String(merged);
                        }
                    }
                }
            }

            // Annotate user messages with persisted timestamp for relative-date reasoning.
            if include_timestamps && role == "user" {
                if let (Some(s), Some(ts)) = (content.as_str(), message.get("timestamp").and_then(|v| v.as_str())) {
                    content = Value::String(format!("[Message Time: {ts}]\n{s}"));
                }
            }

            let mut entry = serde_json::Map::new();
            entry.insert("role".into(), Value::String(role.to_string()));
            entry.insert("content".into(), content);
            for key in [
                "tool_calls",
                "tool_call_id",
                "name",
                "reasoning_content",
                "thinking_blocks",
            ] {
                if let Some(v) = message.get(key) {
                    entry.insert(key.into(), v.clone());
                }
            }
            out.push(Value::Object(entry));
        }

        // Apply token budget if specified.
        if let Some(max_tok) = max_tokens {
            if out.is_empty() {
                return out;
            }
            let mut kept: Vec<Value> = Vec::new();
            let mut used: usize = 0;
            for message in out.iter().rev() {
                let tokens = estimate_tokens_for_message(message);
                if !kept.is_empty() && used + tokens > max_tok {
                    break;
                }
                kept.push(message.clone());
                used += tokens;
            }
            kept.reverse();

            // Keep history aligned to the first visible user turn.
            let first_user = kept.iter().position(|m| m.get("role").and_then(Value::as_str) == Some("user"));
            if let Some(idx) = first_user {
                kept = kept[idx..].to_vec();
            } else {
                // Recover nearest user turn from original output.
                let recovered_user = sliced.iter().rposition(|m| m.get("role").and_then(Value::as_str) == Some("user"));
                if let Some(idx) = recovered_user {
                    // Re-build from idx.
                    kept.clear();
                    for message in &sliced[idx..] {
                        let mut entry = serde_json::Map::new();
                        entry.insert("role".into(), message.get("role").cloned().unwrap_or(Value::Null));
                        entry.insert("content".into(), message.get("content").cloned().unwrap_or(Value::String(String::new())));
                        for key in ["tool_calls", "tool_call_id", "name", "reasoning_content", "thinking_blocks"] {
                            if let Some(v) = message.get(key) {
                                entry.insert(key.into(), v.clone());
                            }
                        }
                        kept.push(Value::Object(entry));
                    }
                }
            }

            // Keep a legal tool-call boundary at the front.
            let legal = find_legal_message_start(&kept);
            if legal > 0 {
                kept = kept[legal..].to_vec();
            }
            return kept;
        }

        out
    }

    /// Clear all messages and reset the session to an initial state.
    pub fn clear(&mut self) {
        self.messages.clear();
        self.last_consolidated = 0;
        self.updated_at = now();
        self.metadata.remove("_last_summary");
    }

    /// Keep a legal recent suffix constrained by a hard message cap.
    pub fn retain_recent_legal_suffix(&mut self, max_messages: usize) {
        if max_messages == 0 {
            self.clear();
            return;
        }
        if self.messages.len() <= max_messages {
            return;
        }

        let mut retained: Vec<Value> = self.messages[self.messages.len() - max_messages..].to_vec();

        // Prefer starting at a user turn when one exists within the tail.
        let first_user = retained.iter().position(|m| m.get("role").and_then(Value::as_str) == Some("user"));
        if let Some(idx) = first_user {
            retained = retained[idx..].to_vec();
        } else {
            // If the tail is assistant/tool-only, anchor to the latest user in
            // the full session and take a capped forward window from there.
            let latest_user = self.messages.iter().rposition(|m| m.get("role").and_then(Value::as_str) == Some("user"));
            if let Some(idx) = latest_user {
                let end = (idx + max_messages).min(self.messages.len());
                retained = self.messages[idx..end].to_vec();
            }
        }

        // Mirror get_history(): avoid persisting orphan tool results at the front.
        let legal = find_legal_message_start(&retained);
        if legal > 0 {
            retained = retained[legal..].to_vec();
        }

        // Hard-cap guarantee: never keep more than max_messages.
        if retained.len() > max_messages {
            retained = retained[retained.len() - max_messages..].to_vec();
            let legal = find_legal_message_start(&retained);
            if legal > 0 {
                retained = retained[legal..].to_vec();
            }
        }

        let dropped = self.messages.len() - retained.len();
        self.messages = retained;
        self.last_consolidated = self.last_consolidated.saturating_sub(dropped);
        self.updated_at = now();
    }

    /// Bound session message growth by archiving and trimming old prefixes.
    ///
    /// Port of Python `Session.enforce_file_cap`: when message count exceeds
    /// `limit`, retains the recent legal suffix and invokes `on_archive`
    /// with the dropped messages (minus already-consolidated ones).
    pub fn enforce_file_cap<F>(&mut self, limit: usize, mut on_archive: F) -> bool
    where
        F: FnMut(&[Value]),
    {
        if limit == 0 || self.messages.len() <= limit {
            return false;
        }

        let before = self.messages.clone();
        let before_last_consolidated = self.last_consolidated;
        let before_count = before.len();

        self.retain_recent_legal_suffix(limit);

        let dropped_count = before_count - self.messages.len();
        if dropped_count == 0 {
            return false;
        }

        let already_consolidated = before_last_consolidated.min(dropped_count);
        let archive_chunk = &before[already_consolidated..dropped_count];
        if !archive_chunk.is_empty() {
            on_archive(archive_chunk);
        }
        true
    }
}

/// Manages conversation sessions.
///
/// Sessions are stored as JSONL files in the sessions directory.
pub struct SessionManager {
    workspace: PathBuf,
    sessions_dir: PathBuf,
    legacy_sessions_dir: PathBuf,
    cache: HashMap<String, Session>,
}

impl SessionManager {
    pub fn new<P: Into<PathBuf>>(workspace: P) -> Self {
        let workspace: PathBuf = workspace.into();
        let sessions_dir = ensure_dir(workspace.join("sessions"));
        let legacy_sessions_dir = get_legacy_sessions_dir();
        Self {
            workspace,
            sessions_dir,
            legacy_sessions_dir,
            cache: HashMap::new(),
        }
    }

    /// Public helper used by HTTP handlers to map an arbitrary key to a
    /// stable filename stem.
    pub fn safe_key(key: &str) -> String {
        safe_filename(&key.replace(':', "_"))
    }

    fn session_path(&self, key: &str) -> PathBuf {
        self.sessions_dir.join(format!("{}.jsonl", Self::safe_key(key)))
    }

    fn legacy_session_path(&self, key: &str) -> PathBuf {
        self.legacy_sessions_dir
            .join(format!("{}.jsonl", Self::safe_key(key)))
    }

    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    /// Get an existing session or create a new one.
    pub fn get_or_create(&mut self, key: &str) -> Session {
        if let Some(s) = self.cache.get(key) {
            return s.clone();
        }
        let session = self.load(key).unwrap_or_else(|| Session::new(key));
        self.cache.insert(key.to_string(), session.clone());
        session
    }

    fn migrate_legacy(&self, key: &str, target: &Path) {
        let legacy = self.legacy_session_path(key);
        if legacy.exists() {
            match fs::rename(&legacy, target) {
                Ok(_) => info!("Migrated session {key} from legacy path"),
                Err(e) => warn!("Failed to migrate session {key}: {e}"),
            }
        }
    }

    fn load(&self, key: &str) -> Option<Session> {
        let path = self.session_path(key);
        if !path.exists() {
            self.migrate_legacy(key, &path);
        }
        if !path.exists() {
            return None;
        }

        match self.read_strict(key, &path) {
            Ok(session) => Some(session),
            Err(e) => {
                warn!("Failed to load session {key}: {e}");
                let repaired = self.repair(key);
                if let Some(ref r) = repaired {
                    info!(
                        "Recovered session {key} from corrupt file ({} messages)",
                        r.messages.len()
                    );
                }
                repaired
            }
        }
    }

    fn read_strict(&self, key: &str, path: &Path) -> std::io::Result<Session> {
        let file = fs::File::open(path)?;
        let reader = BufReader::new(file);

        let mut messages: Vec<Value> = Vec::new();
        let mut metadata: HashMap<String, Value> = HashMap::new();
        let mut created_at: Option<DateTime<Local>> = None;
        let mut updated_at: Option<DateTime<Local>> = None;
        let mut last_consolidated: usize = 0;

        for line in reader.lines() {
            let line = line?;
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let data: Value = serde_json::from_str(line)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            if data.get("_type").and_then(Value::as_str) == Some("metadata") {
                if let Some(m) = data.get("metadata").and_then(Value::as_object) {
                    metadata = m
                        .iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect();
                }
                created_at = data
                    .get("created_at")
                    .and_then(Value::as_str)
                    .and_then(parse_isoformat);
                updated_at = data
                    .get("updated_at")
                    .and_then(Value::as_str)
                    .and_then(parse_isoformat);
                last_consolidated = data
                    .get("last_consolidated")
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as usize;
            } else {
                messages.push(data);
            }
        }

        Ok(Session {
            key: key.to_string(),
            messages,
            created_at: created_at.unwrap_or_else(now),
            updated_at: updated_at.unwrap_or_else(now),
            metadata,
            last_consolidated,
        })
    }

    fn repair(&self, key: &str) -> Option<Session> {
        let path = self.session_path(key);
        if !path.exists() {
            return None;
        }
        let file = match fs::File::open(&path) {
            Ok(f) => f,
            Err(_) => return None,
        };
        let reader = BufReader::new(file);

        let mut messages: Vec<Value> = Vec::new();
        let mut metadata: HashMap<String, Value> = HashMap::new();
        let mut created_at: Option<DateTime<Local>> = None;
        let mut updated_at: Option<DateTime<Local>> = None;
        let mut last_consolidated: usize = 0;
        let mut skipped = 0u32;

        for line in reader.lines().flatten() {
            let line = line.trim().to_string();
            if line.is_empty() {
                continue;
            }
            let Ok(data) = serde_json::from_str::<Value>(&line) else {
                skipped += 1;
                continue;
            };
            if data.get("_type").and_then(Value::as_str) == Some("metadata") {
                if let Some(m) = data.get("metadata").and_then(Value::as_object) {
                    metadata = m.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                }
                created_at = data
                    .get("created_at")
                    .and_then(Value::as_str)
                    .and_then(parse_isoformat);
                updated_at = data
                    .get("updated_at")
                    .and_then(Value::as_str)
                    .and_then(parse_isoformat);
                last_consolidated = data
                    .get("last_consolidated")
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as usize;
            } else {
                messages.push(data);
            }
        }
        if skipped > 0 {
            warn!("Skipped {skipped} corrupt lines in session {key}");
        }
        if messages.is_empty() && metadata.is_empty() {
            return None;
        }
        Some(Session {
            key: key.to_string(),
            messages,
            created_at: created_at.unwrap_or_else(now),
            updated_at: updated_at.unwrap_or_else(now),
            metadata,
            last_consolidated,
        })
    }

    fn session_payload(session: &Session) -> Value {
        serde_json::json!({
            "key": session.key,
            "created_at": session.created_at.to_rfc3339(),
            "updated_at": session.updated_at.to_rfc3339(),
            "metadata": session.metadata,
            "messages": session.messages,
        })
    }

    /// Save a session to disk atomically.
    ///
    /// When `fsync` is `true` the final file and its parent directory are
    /// explicitly flushed to durable storage.
    pub fn save(&mut self, session: Session, fsync: bool) -> std::io::Result<()> {
        let path = self.session_path(&session.key);
        let tmp_path = path.with_extension("jsonl.tmp");

        {
            let mut f = fs::File::create(&tmp_path)?;
            let meta = serde_json::json!({
                "_type": "metadata",
                "key": session.key,
                "created_at": session.created_at.to_rfc3339(),
                "updated_at": session.updated_at.to_rfc3339(),
                "metadata": session.metadata,
                "last_consolidated": session.last_consolidated,
            });
            writeln!(f, "{}", serde_json::to_string(&meta)?)?;
            for msg in &session.messages {
                writeln!(f, "{}", serde_json::to_string(msg)?)?;
            }
            if fsync {
                f.flush()?;
                f.sync_all()?;
            }
        }

        fs::rename(&tmp_path, &path).inspect_err(|_| {
            let _ = fs::remove_file(&tmp_path);
        })?;

        // Directory fsync is best-effort: on Windows it is a no-op.
        if fsync {
            if let Some(parent) = path.parent() {
                if let Ok(dir) = fs::File::open(parent) {
                    let _ = dir.sync_all();
                }
            }
        }

        self.cache.insert(session.key.clone(), session);
        Ok(())
    }

    /// Re-save every cached session with `fsync` for durable shutdown.
    pub fn flush_all(&mut self) -> usize {
        let mut flushed = 0;
        let keys: Vec<String> = self.cache.keys().cloned().collect();
        for key in keys {
            if let Some(s) = self.cache.get(&key).cloned() {
                match self.save(s, true) {
                    Ok(_) => flushed += 1,
                    Err(e) => warn!("Failed to flush session {key}: {e}"),
                }
            }
        }
        flushed
    }

    /// Remove a session from the in-memory cache.
    pub fn invalidate(&mut self, key: &str) {
        self.cache.remove(key);
    }

    /// Remove a session from disk and the in-memory cache.
    pub fn delete_session(&mut self, key: &str) -> bool {
        let path = self.session_path(key);
        self.invalidate(key);
        if !path.exists() {
            return false;
        }
        match fs::remove_file(&path) {
            Ok(_) => true,
            Err(e) => {
                warn!("Failed to delete session file {}: {e}", path.display());
                false
            }
        }
    }

    /// Load a session from disk without caching; intended for read-only
    /// HTTP endpoints.
    pub fn read_session_file(&self, key: &str) -> Option<Value> {
        let path = self.session_path(key);
        if !path.exists() {
            return None;
        }
        match fs::File::open(&path) {
            Ok(f) => {
                let reader = BufReader::new(f);
                let mut messages: Vec<Value> = Vec::new();
                let mut metadata = serde_json::Map::new();
                let mut created_at: Option<String> = None;
                let mut updated_at: Option<String> = None;
                let mut stored_key: Option<String> = None;
                for line in reader.lines() {
                    let Ok(line) = line else {
                        return self.repair(key).map(|r| Self::session_payload(&r));
                    };
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }
                    let Ok(data) = serde_json::from_str::<Value>(line) else {
                        return self.repair(key).map(|r| Self::session_payload(&r));
                    };
                    if data.get("_type").and_then(Value::as_str) == Some("metadata") {
                        if let Some(m) = data.get("metadata").and_then(Value::as_object) {
                            metadata = m
                                .iter()
                                .map(|(k, v)| (k.clone(), v.clone()))
                                .collect();
                        }
                        created_at = data
                            .get("created_at")
                            .and_then(Value::as_str)
                            .map(str::to_string);
                        updated_at = data
                            .get("updated_at")
                            .and_then(Value::as_str)
                            .map(str::to_string);
                        stored_key = data
                            .get("key")
                            .and_then(Value::as_str)
                            .map(str::to_string);
                    } else {
                        messages.push(data);
                    }
                }
                Some(serde_json::json!({
                    "key": stored_key.unwrap_or_else(|| key.to_string()),
                    "created_at": created_at,
                    "updated_at": updated_at,
                    "metadata": metadata,
                    "messages": messages,
                }))
            }
            Err(e) => {
                warn!("Failed to read session {key}: {e}");
                self.repair(key).map(|r| Self::session_payload(&r))
            }
        }
    }

    /// List all sessions (latest first by `updated_at`).
    pub fn list_sessions(&self) -> Vec<Value> {
        let mut sessions: Vec<Value> = Vec::new();
        let Ok(entries) = fs::read_dir(&self.sessions_dir) else {
            return sessions;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            let fallback_key = stem.replacen('_', ":", 1);
            match self._read_session_file_for_list(&path, &fallback_key) {
                Some(info) => sessions.push(info),
                None => {
                    if let Some(r) = self.repair(&fallback_key) {
                        sessions.push(serde_json::json!({
                            "key": r.key,
                            "created_at": r.created_at.to_rfc3339(),
                            "updated_at": r.updated_at.to_rfc3339(),
                            "title": r.metadata.get("title").and_then(|v| v.as_str()).map(String::from).unwrap_or_default(),
                            "preview": r.messages.iter().filter_map(|m| {
                                let t = message_preview_text(m);
                                if t.is_empty() { None } else { Some(t) }
                            }).next().unwrap_or_default(),
                            "path": path.display().to_string(),
                        }));
                    }
                }
            }
        }
        sessions.sort_by(|a, b| {
            let ka = a.get("updated_at").and_then(Value::as_str).unwrap_or("");
            let kb = b.get("updated_at").and_then(Value::as_str).unwrap_or("");
            kb.cmp(ka)
        });
        sessions
    }

    fn _read_session_file_for_list(&self, path: &Path, fallback_key: &str) -> Option<Value> {
        let f = fs::File::open(path).ok()?;
        let mut reader = BufReader::new(f);
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() || line.trim().is_empty() {
            return None;
        }
        let data: Value = serde_json::from_str(line.trim()).ok()?;
        if data.get("_type").and_then(Value::as_str) != Some("metadata") {
            return None;
        }

        let key = data
            .get("key")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or(fallback_key.to_string());
        let created_at = data.get("created_at").cloned();
        let updated_at = data.get("updated_at").cloned();

        let metadata = data.get("metadata").and_then(|v| v.as_object()).map(|m| {
            m.iter().map(|(k, v)| (k.clone(), v.clone())).collect::<HashMap<String, Value>>()
        }).unwrap_or_default();

        let title = metadata.get("title").and_then(|v| v.as_str()).map(String::from).unwrap_or_default();

        // Read messages to find preview (prefer first user, fallback to first assistant).
        let mut preview = String::new();
        let mut fallback_preview = String::new();
        for line in reader.lines() {
            let Ok(line) = line else { continue };
            let line = line.trim();
            if line.is_empty() { continue };
            let Ok(item) = serde_json::from_str::<Value>(line) else { continue };
            if item.get("_type").and_then(Value::as_str) == Some("metadata") { continue };
            let text = message_preview_text(&item);
            if text.is_empty() { continue };
            if item.get("role").and_then(Value::as_str) == Some("user") {
                preview = text;
                break;
            }
            if fallback_preview.is_empty() && item.get("role").and_then(Value::as_str) == Some("assistant") {
                fallback_preview = text;
            }
        }
        if preview.is_empty() {
            preview = fallback_preview;
        }

        Some(serde_json::json!({
            "key": key,
            "created_at": created_at,
            "updated_at": updated_at,
            "title": title,
            "preview": preview,
            "path": path.display().to_string(),
        }))
    }
}

/// Parse a timestamp in the format `datetime.isoformat()` produces
/// (RFC3339-like, may omit timezone for naive datetimes).
fn parse_isoformat(s: &str) -> Option<DateTime<Local>> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.with_timezone(&Local));
    }
    if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f") {
        return Some(naive.and_local_timezone(Local).single().unwrap_or_else(now));
    }
    if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S") {
        return Some(naive.and_local_timezone(Local).single().unwrap_or_else(now));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    fn tempdir() -> PathBuf {
        let p = env::temp_dir().join(format!(
            "nanobot-session-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn save_and_load_roundtrip() {
        let ws = tempdir();
        let mut mgr = SessionManager::new(&ws);
        let mut s = Session::new("telegram:c1");
        s.add_message("user", "hi", HashMap::new());
        s.add_message("assistant", "hello", HashMap::new());
        mgr.save(s, false).unwrap();

        let mut mgr2 = SessionManager::new(&ws);
        let loaded = mgr2.get_or_create("telegram:c1");
        assert_eq!(loaded.messages.len(), 2);
        assert_eq!(
            loaded.messages[0].get("role").and_then(Value::as_str),
            Some("user")
        );
    }

    #[test]
    fn safe_key_replaces_colon() {
        assert_eq!(SessionManager::safe_key("telegram:c1"), "telegram_c1");
    }

    #[test]
    fn retain_recent_legal_suffix_preserves_user_alignment() {
        let mut s = Session::new("k");
        for i in 0..10 {
            let role = if i % 2 == 0 { "user" } else { "assistant" };
            s.add_message(role, "x", HashMap::new());
        }
        s.retain_recent_legal_suffix(3);
        assert!(s.messages.len() >= 3);
        assert_eq!(
            s.messages[0].get("role").and_then(Value::as_str),
            Some("user")
        );
    }
}
