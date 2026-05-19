//! Adapter that plugs `agent::AgentLoop` into `api::ApiAgent`.
//!
//! Keeping the adapter here (rather than inside the `api` crate) avoids a
//! build-time dependency cycle and lets the `api` crate stay
//! runtime-agnostic.

use std::sync::Arc;

use agent::AgentLoop;
use api::{ApiAgent, ApiAnswer, ApiRequest};
use async_trait::async_trait;
use bus::InboundMessage;
use chrono::Local;

/// Wrap an [`AgentLoop`] as an [`ApiAgent`]. The adapter builds an
/// [`InboundMessage`] from the parsed API request, lets the loop run,
/// then returns the `final_content`.
pub struct AgentLoopApi {
    pub agent: Arc<AgentLoop>,
}

impl AgentLoopApi {
    pub fn new(agent: Arc<AgentLoop>) -> Self {
        Self { agent }
    }
}

#[async_trait]
impl ApiAgent for AgentLoopApi {
    async fn generate(&self, req: ApiRequest) -> Result<ApiAnswer, String> {
        let msg = InboundMessage {
            channel: req.channel.clone(),
            sender_id: req.session_id.clone().unwrap_or_else(|| "default".into()),
            chat_id: req.chat_id.clone(),
            content: req.content.clone(),
            timestamp: Local::now(),
            media: req
                .media
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect(),
            metadata: Default::default(),
            session_key_override: Some(req.session_key()),
        };

        let result = self.agent.process_inbound(msg).await?;
        Ok(ApiAnswer {
            content: result.final_content.unwrap_or_default(),
        })
    }
}
