//! Adapter bridging `AgentLoop` to the `ApiAgent` trait used by the HTTP API.
//!
//! The HTTP server never knows about `AgentLoop`; it only speaks `ApiAgent`.
//! This module implements `ApiAgent` by delegating to a real `AgentLoop`.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use bus::InboundMessage;
use chrono::Local;
use tokio::sync::Mutex;

use agent::AgentLoop;
use api::{ApiAgent, ApiAnswer, ApiRequest};

/// Wraps an `AgentLoop` so it can be used as an `ApiAgent`.
pub struct AgentLoopApi {
    loop_: Arc<AgentLoop>,
    /// Serialise per-session requests so we don't race on session state.
    session_locks: Arc<Mutex<std::collections::HashMap<String, Arc<tokio::sync::Semaphore>>>>,
}

impl AgentLoopApi {
    pub fn new(loop_: Arc<AgentLoop>) -> Self {
        Self {
            loop_,
            session_locks: Arc::new(Mutex::new(std::collections::HashMap::new())),
        }
    }

    /// Acquire a per-session semaphore to serialise requests.
    async fn acquire_session(&self, session_key: &str) -> Arc<tokio::sync::Semaphore> {
        let mut locks = self.session_locks.lock().await;
        let entry = locks
            .entry(session_key.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Semaphore::new(1)));
        entry.clone()
    }
}

#[async_trait]
impl ApiAgent for AgentLoopApi {
    async fn generate(&self, req: ApiRequest) -> Result<ApiAnswer, String> {
        let session_key = req.session_key();
        let semaphore = self.acquire_session(&session_key).await;
        let _permit = semaphore
            .acquire()
            .await
            .map_err(|e| format!("session lock error: {e}"))?;

        let msg = InboundMessage {
            channel: req.channel.clone(),
            sender_id: req.chat_id.clone(),
            chat_id: req.chat_id.clone(),
            content: req.content.clone(),
            timestamp: Local::now(),
            media: req
                .media
                .iter()
                .map(|p| p.to_string_lossy().to_string())
                .collect(),
            metadata: HashMap::new(),
            session_key_override: Some(session_key.clone()),
        };

        let result = self.loop_.process_message(msg, Some(session_key)).await?;

        match result {
            Some(outbound) => Ok(ApiAnswer {
                content: outbound.content,
            }),
            None => Ok(ApiAnswer {
                content: String::new(),
            }),
        }
    }
}
