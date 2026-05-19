//! Session management for conversation history (port of
//! `nanobot.session.manager`).

use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Local};
use log::{info, warn};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use config::{ensure_dir, get_legacy_sessions_dir};
use utils::helpers::{find_legal_message_start, image_placeholder_text, safe_filename};

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
        msg.insert("content".into(), Value::String(content.into()));
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
        let unconsolidated = &self.messages[self.last_consolidated.min(self.messages.len())..];
        let sliced: Vec<&Value> = if unconsolidated.len() > max_messages {
            unconsolidated[unconsolidated.len() - max_messages..]
                .iter()
                .collect()
        } else {
            unconsolidated.iter().collect()
        };
        // Avoid starting mid-turn when possible.
        let mut start = 0usize;
        for (i, m) in sliced.iter().enumerate() {
            if m.get("role").and_then(Value::as_str) == Some("user") {
                start = i;
                break;
            }
        }
        let mut sliced: Vec<Value> = sliced[start..].iter().map(|v| (*v).clone()).collect();

        // Drop orphan tool results at the front.
        let legal_start = find_legal_message_start(&sliced);
        if legal_start > 0 {
            sliced = sliced.split_off(legal_start);
        }

        // Rewrite media-bearing messages to include image breadcrumbs.
        let mut out: Vec<Value> = Vec::with_capacity(sliced.len());
        for mut message in sliced {
            let content = message
                .get("content")
                .cloned()
                .unwrap_or(Value::String(String::new()));
            let mut new_content = content.clone();
            if let Some(media) = message.get("media").and_then(Value::as_array) {
                if !media.is_empty() {
                    if let Some(text) = content.as_str() {
                        let breadcrumbs = media
                            .iter()
                            .filter_map(|v| v.as_str().filter(|s| !s.is_empty()))
                            .map(|p| image_placeholder_text(Some(p)))
                            .collect::<Vec<_>>()
                            .join("\n");
                        let merged = if text.is_empty() {
                            breadcrumbs
                        } else {
                            format!("{text}\n{breadcrumbs}")
                        };
                        new_content = Value::String(merged);
                    }
                }
            }
            let mut entry = serde_json::Map::new();
            if let Some(role) = message.get("role").cloned() {
                entry.insert("role".into(), role);
            }
            entry.insert("content".into(), new_content);
            for key in ["tool_calls", "tool_call_id", "name", "reasoning_content"] {
                if let Some(v) = message.get(key) {
                    entry.insert(key.into(), v.clone());
                }
            }
            // Ensure we don't accidentally keep side metadata.
            let _ = message.as_object_mut();
            out.push(Value::Object(entry));
        }
        out
    }

    /// Clear all messages and reset the session to an initial state.
    pub fn clear(&mut self) {
        self.messages.clear();
        self.last_consolidated = 0;
        self.updated_at = now();
    }

    /// Keep a legal recent suffix, mirroring `get_history` boundary rules.
    pub fn retain_recent_legal_suffix(&mut self, max_messages: usize) {
        if max_messages == 0 {
            self.clear();
            return;
        }
        if self.messages.len() <= max_messages {
            return;
        }
        let mut start_idx = self.messages.len() - max_messages;
        while start_idx > 0
            && self.messages[start_idx].get("role").and_then(Value::as_str) != Some("user")
        {
            start_idx -= 1;
        }
        let mut retained = self.messages.split_off(start_idx);
        self.messages.clear();
        let legal = find_legal_message_start(&retained);
        if legal > 0 {
            retained = retained.split_off(legal);
        }
        let dropped = (start_idx + legal) as isize;
        self.messages = retained;
        self.last_consolidated = (self.last_consolidated as isize - dropped).max(0) as usize;
        self.updated_at = now();
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
            match fs::File::open(&path) {
                Ok(f) => {
                    let mut reader = BufReader::new(f);
                    let mut line = String::new();
                    let ok = reader.read_line(&mut line).is_ok();
                    let line = line.trim().to_string();
                    if ok && !line.is_empty() {
                        if let Ok(data) = serde_json::from_str::<Value>(&line) {
                            if data.get("_type").and_then(Value::as_str) == Some("metadata") {
                                let key = data
                                    .get("key")
                                    .and_then(Value::as_str)
                                    .map(str::to_string)
                                    .unwrap_or(fallback_key.clone());
                                sessions.push(serde_json::json!({
                                    "key": key,
                                    "created_at": data.get("created_at"),
                                    "updated_at": data.get("updated_at"),
                                    "path": path.display().to_string(),
                                }));
                                continue;
                            }
                        }
                    }
                    // Fallthrough to repair path.
                    if let Some(r) = self.repair(&fallback_key) {
                        sessions.push(serde_json::json!({
                            "key": r.key,
                            "created_at": r.created_at.to_rfc3339(),
                            "updated_at": r.updated_at.to_rfc3339(),
                            "path": path.display().to_string(),
                        }));
                    }
                }
                Err(_) => {
                    if let Some(r) = self.repair(&fallback_key) {
                        sessions.push(serde_json::json!({
                            "key": r.key,
                            "created_at": r.created_at.to_rfc3339(),
                            "updated_at": r.updated_at.to_rfc3339(),
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
