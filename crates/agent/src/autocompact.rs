//! Auto compact: proactive compression of idle sessions to reduce token
//! cost and latency. Port of `nanobot.agent.autocompact`.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Local};
use log::{error, info};
use serde_json::Value;

use session::{Session, SessionManager};

pub use crate::memory::Consolidator;

const RECENT_SUFFIX_MESSAGES: usize = 8;

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
        format!("Previous conversation summary (last active {}):\n{}", last_active.to_rfc3339(), text)
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
        let summary = self.consolidator.compact_idle_session(key, RECENT_SUFFIX_MESSAGES).await;
        if let Some(ref text) = summary {
            if !text.is_empty() && text != "(nothing)" {
                let mut mgr = self.sessions.lock().await;
                let session = mgr.get_or_create(key);
                let last_active = session.updated_at;
                let mut entry = serde_json::Map::new();
                entry.insert("text".into(), Value::String(text.clone()));
                entry.insert(
                    "last_active".into(),
                    Value::String(last_active.to_rfc3339()),
                );
                let mut session = session;
                session.metadata.insert("_last_summary".into(), Value::Object(entry));
                mgr.save(session, false).map_err(|e| e.to_string())?;
                self.inner
                    .lock()
                    .unwrap()
                    .summaries
                    .insert(key.to_string(), (text.clone(), last_active));
            }
        }
        Ok(())
    }

    /// Inspect an incoming session and inject a stored summary if one exists.
    pub async fn prepare_session(&self, session: Session, key: &str) -> (Session, Option<String>) {
        let is_archiving = self.inner.lock().unwrap().archiving.contains(key);
        let session = if is_archiving || self.is_expired(Some(session.updated_at), Local::now())
        {
            info!(
                "Auto-compact: reloading session {key} (archiving={})",
                is_archiving
            );
            self.sessions.lock().await.get_or_create(key)
        } else {
            session
        };

        // Hot path: summary from in-memory dict (process hasn't restarted).
        if let Some((text, ts)) = self.inner.lock().unwrap().summaries.remove(key) {
            return (session, Some(Self::format_summary(&text, ts)));
        }

        // Cold path: summary persisted in session metadata (process restarted).
        if let Some(meta) = session.metadata.get("_last_summary") {
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

