use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;

use super::file_state::FileStateStore;
use super::image_generation::ImageGenerationProviderConfig;
use crate::loop_::ProviderSnapshot;
use crate::subagent::SubagentManager;
use config::schema::Config;
use cron::service::CronService;
use session::manager::SessionManager;

/// Per-request context injected into tools at message-processing time.
#[derive(Debug, Clone, Default)]
pub struct RequestContext {
    pub channel: String,
    pub chat_id: String,
    pub message_id: Option<String>,
    pub session_key: Option<String>,
    pub metadata: HashMap<String, Value>,
}

/// Tools implementing this trait receive per-request context updates.
pub trait ContextAware {
    fn set_context(&mut self, ctx: &RequestContext);
}

/// Build-time context passed to tools during construction.
pub struct ToolContext {
    pub config: Arc<Config>,
    pub workspace: String,
    pub bus: Option<Arc<bus::MessageBus>>,
    pub session_manager: Option<Arc<SessionManager>>,
    pub timezone: String,
    pub subagent_manager: Option<Arc<SubagentManager>>,
    pub cron_service: Option<Arc<CronService>>,
    pub sessions: Option<Arc<tokio::sync::Mutex<SessionManager>>>,
    pub file_state_store: Option<Arc<FileStateStore>>,
    pub provider_snapshot_loader: Option<Arc<dyn Fn() -> ProviderSnapshot + Send + Sync>>,
    pub image_generation_provider_configs: Option<HashMap<String, ImageGenerationProviderConfig>>,
}

impl ToolContext {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        config: Arc<Config>,
        workspace: String,
        bus: Option<Arc<bus::MessageBus>>,
        session_manager: Option<Arc<SessionManager>>,
        timezone: String,
        subagent_manager: Option<Arc<SubagentManager>>,
        cron_service: Option<Arc<CronService>>,
        sessions: Option<Arc<tokio::sync::Mutex<SessionManager>>>,
        file_state_store: Option<Arc<FileStateStore>>,
        provider_snapshot_loader: Option<Arc<dyn Fn() -> ProviderSnapshot + Send + Sync>>,
        image_generation_provider_configs: Option<HashMap<String, ImageGenerationProviderConfig>>,
    ) -> Self {
        Self {
            config,
            workspace,
            bus,
            session_manager,
            timezone,
            subagent_manager,
            cron_service,
            sessions,
            file_state_store,
            provider_snapshot_loader,
            image_generation_provider_configs,
        }
    }
}
