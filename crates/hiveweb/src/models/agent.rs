use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Agent {
    pub id: i64,
    pub identifier: String,
    pub name: String,
    pub description: Option<String>,
    pub system_prompt: String,
    pub parent_agent_id: Option<i64>,
    pub depth: i8,
    /// llm_presets.toml 中的命名 preset；NULL = 全局默认
    pub model_preset: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct AgentTool {
    pub agent_id: i64,
    pub tool_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct AgentSkill {
    pub agent_id: i64,
    pub skill_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct AgentPermission {
    pub agent_id: i64,
    pub capability: String,
}
