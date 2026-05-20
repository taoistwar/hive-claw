use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;

use config::schema::Config;

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
    pub session_manager: Option<Arc<session::manager::SessionManager>>,
    pub timezone: String,
}

impl ToolContext {
    pub fn new(
        config: Arc<Config>,
        workspace: String,
        bus: Option<Arc<bus::MessageBus>>,
        session_manager: Option<Arc<session::manager::SessionManager>>,
        timezone: String,
    ) -> Self {
        Self {
            config,
            workspace,
            bus,
            session_manager,
            timezone,
        }
    }
}
