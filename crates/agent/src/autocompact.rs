//! Auto compact: proactive compression of idle sessions to reduce token
//! cost and latency. Port of `nanobot.agent.autocompact`.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{DateTime, Local};
use log::{error, info};
use serde_json::Value;

use session::{Session, SessionManager};

const RECENT_SUFFIX_MESSAGES: usize = 8;

/// Trait that produces a natural-language summary for a batch of archived
/// messages. Abstracted so tests (and any LLM backend) can plug in.
#[async_trait]
pub trait Consolidator: Send + Sync {
    /// Summarize *messages* into a short text. Returns `None` when nothing
    /// should be injected into the next prompt.
    async fn archive(&self, messages: Vec<Value>) -> Option<String>;
}

/// Auto-compaction bookkeeping (TTL-based).
pub struct AutoCompact {
    sessions: Arc<tokio::sync::Mutex<SessionManager>>,
    consolidator: Arc<dyn Consolidator>,
    ttl_minutes: i64,
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    archiving: HashSet<String>,
    summaries: HashMap<String, (String, DateTime<Local>)>,
}

impl AutoCompact {
    pub fn new(
        sessions: Arc<tokio::sync::Mutex<SessionManager>>,
        consolidator: Arc<dyn Consolidator>,
        ttl_minutes: i64,
    ) -> Self {
        Self {
            sessions,
            consolidator,
            ttl_minutes,
            inner: Mutex::new(Inner::default()),
        }
    }

    fn is_expired(&self, ts: Option<DateTime<Local>>, now: DateTime<Local>) -> bool {
        if self.ttl_minutes <= 0 {
            return false;
        }
        let Some(ts) = ts else {
            return false;
        };
        (now - ts).num_seconds() >= self.ttl_minutes * 60
    }

    fn format_summary(text: &str, last_active: DateTime<Local>) -> String {
        let idle = (Local::now() - last_active).num_minutes().max(0);
        format!("Inactive for {idle} minutes.\nPrevious conversation summary: {text}")
    }

    fn split_unconsolidated(session: &Session) -> (Vec<Value>, Vec<Value>) {
        let tail: Vec<Value> = session
            .messages
            .iter()
            .skip(session.last_consolidated)
            .cloned()
            .collect();
        if tail.is_empty() {
            return (Vec::new(), Vec::new());
        }
        let mut probe = Session {
            key: session.key.clone(),
            messages: tail.clone(),
            created_at: session.created_at,
            updated_at: session.updated_at,
            metadata: HashMap::new(),
            last_consolidated: 0,
        };
        probe.retain_recent_legal_suffix(RECENT_SUFFIX_MESSAGES);
        let kept = probe.messages;
        let cut = tail.len().saturating_sub(kept.len());
        (tail[..cut].to_vec(), kept)
    }

    /// Enqueue archival for idle sessions (except those with in-flight tasks).
    /// `spawner` receives a closure that runs the archival task; typical
    /// callers wrap this in `tokio::spawn`.
    pub async fn check_expired<F>(self: Arc<Self>, spawner: F, active: &HashSet<String>)
    where
        F: Fn(futures::future::BoxFuture<'static, ()>),
    {
        let now = Local::now();
        let listing: Vec<Value> = self.sessions.lock().await.list_sessions();
        for info in listing {
            let key = info
                .get("key")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if key.is_empty() {
                continue;
            }
            {
                let inner = self.inner.lock().unwrap();
                if inner.archiving.contains(&key) {
                    continue;
                }
            }
            if active.contains(&key) {
                continue;
            }
            let updated_at = info
                .get("updated_at")
                .and_then(|v| v.as_str())
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.with_timezone(&Local));
            if !self.is_expired(updated_at, now) {
                continue;
            }
            self.inner.lock().unwrap().archiving.insert(key.clone());
            let this = Arc::clone(&self);
            spawner(Box::pin(async move { this.archive(&key).await }));
        }
    }

    async fn archive(&self, key: &str) {
        let result = self.archive_inner(key).await;
        if let Err(err) = result {
            error!("Auto-compact: failed for {key}: {err}");
        }
        self.inner.lock().unwrap().archiving.remove(key);
    }

    async fn archive_inner(&self, key: &str) -> Result<(), String> {
        let session_clone = {
            let mut mgr = self.sessions.lock().await;
            mgr.invalidate(key);
            mgr.get_or_create(key)
        };
        let (archive_msgs, kept_msgs) = Self::split_unconsolidated(&session_clone);

        if archive_msgs.is_empty() && kept_msgs.is_empty() {
            let mut updated = session_clone;
            updated.updated_at = Local::now();
            self.sessions
                .lock()
                .await
                .save(updated, false)
                .map_err(|e| e.to_string())?;
            return Ok(());
        }

        let last_active = session_clone.updated_at;
        let mut summary: Option<String> = None;
        if !archive_msgs.is_empty() {
            summary = self.consolidator.archive(archive_msgs.clone()).await;
        }

        let mut updated = session_clone;
        if let Some(ref text) = summary {
            if !text.is_empty() && text != "(nothing)" {
                self.inner
                    .lock()
                    .unwrap()
                    .summaries
                    .insert(key.to_string(), (text.clone(), last_active));
                let mut entry = serde_json::Map::new();
                entry.insert("text".into(), Value::String(text.clone()));
                entry.insert(
                    "last_active".into(),
                    Value::String(last_active.to_rfc3339()),
                );
                updated
                    .metadata
                    .insert("_last_summary".into(), Value::Object(entry));
            }
        }
        updated.messages = kept_msgs.clone();
        updated.last_consolidated = 0;
        updated.updated_at = Local::now();

        self.sessions
            .lock()
            .await
            .save(updated, false)
            .map_err(|e| e.to_string())?;

        if !archive_msgs.is_empty() {
            info!(
                "Auto-compact: archived {} (archived={}, kept={}, summary={})",
                key,
                archive_msgs.len(),
                kept_msgs.len(),
                summary.is_some()
            );
        }
        Ok(())
    }

    /// Inspect an incoming session and inject a stored summary if one exists.
    pub async fn prepare_session(&self, session: Session, key: &str) -> (Session, Option<String>) {
        let is_archiving = self.inner.lock().unwrap().archiving.contains(key);
        let mut session = if is_archiving || self.is_expired(Some(session.updated_at), Local::now())
        {
            info!(
                "Auto-compact: reloading session {key} (archiving={})",
                is_archiving
            );
            self.sessions.lock().await.get_or_create(key)
        } else {
            session
        };

        if let Some((text, ts)) = self.inner.lock().unwrap().summaries.remove(key) {
            session.metadata.remove("_last_summary");
            return (session, Some(Self::format_summary(&text, ts)));
        }

        if let Some(meta) = session.metadata.remove("_last_summary") {
            self.sessions
                .lock()
                .await
                .save(session.clone(), false)
                .ok();
            let text = meta
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let ts = meta
                .get("last_active")
                .and_then(|v| v.as_str())
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.with_timezone(&Local))
                .unwrap_or_else(Local::now);
            return (session, Some(Self::format_summary(&text, ts)));
        }
        (session, None)
    }
}

