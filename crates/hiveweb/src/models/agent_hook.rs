use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

/// Hook configuration entity (maps to `agent_hooks` table).
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct AgentHook {
    pub id: i64,
    pub agent_id: i64,
    pub name: String,
    pub description: Option<String>,
    pub trigger_point: String,
    pub action_type: String,
    pub action_params: serde_json::Value,
    pub enabled: bool,
    pub sort_order: i32,
    pub blocking_mode: bool,
    pub timeout_ms: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Hook execution audit record (maps to `hook_executions` table).
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct HookExecution {
    pub id: i64,
    pub agent_id: i64,
    pub agent_identifier: String,
    pub hook_id: Option<i64>,
    pub session_id: Option<i64>,
    pub trigger_point: String,
    pub action_type: String,
    pub outcome: String,
    pub error_summary: Option<String>,
    pub elapsed_ms: Option<i32>,
    pub context_snapshot: Option<serde_json::Value>,
    pub request_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// Request DTO for creating a new Hook.
#[derive(Debug, Clone, Deserialize)]
pub struct CreateHookRequest {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub trigger_point: String,
    pub action_type: String,
    pub action_params: serde_json::Value,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub sort_order: i32,
    #[serde(default)]
    pub blocking_mode: bool,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: i32,
}

fn default_enabled() -> bool { true }
fn default_timeout_ms() -> i32 { 10000 }

/// Request DTO for updating an existing Hook.
#[derive(Debug, Clone, Deserialize)]
pub struct UpdateHookRequest {
    #[serde(default)]
    pub name: Option<String>,
    pub description: Option<Option<String>>,
    pub trigger_point: Option<String>,
    pub action_type: Option<String>,
    pub action_params: Option<serde_json::Value>,
    pub enabled: Option<bool>,
    pub sort_order: Option<i32>,
    pub blocking_mode: Option<bool>,
    pub timeout_ms: Option<i32>,
    /// Optimistic lock: must match the `updated_at` from GET response.
    pub updated_at: DateTime<Utc>,
}

/// Query parameters for listing Hook executions.
#[derive(Debug, Clone, Deserialize)]
pub struct HookExecutionQuery {
    pub agent_id: Option<i64>,
    pub trigger_point: Option<String>,
    pub outcome: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    #[serde(default = "default_page")]
    pub page: u32,
    #[serde(default = "default_page_size")]
    pub page_size: u32,
}

fn default_page() -> u32 { 1 }
fn default_page_size() -> u32 { 20 }
