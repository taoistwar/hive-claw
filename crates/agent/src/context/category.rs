//! Category enum, RecordEntry, and related types for AgentContext.

use serde::{Deserialize, Serialize};

/// Data categories within AgentContext.
/// Each category has its own write lock for concurrent access.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Category {
    /// Entity recognition results
    Entities,
    /// Intent recognition results
    Intentions,
    /// Tool return results
    ToolResults,
    /// Data query results
    QueryResults,
    /// Workflow intermediate results
    WorkflowResults,
    /// Agent reasoning results
    ReasoningResults,
    /// Extension content index view (actual data stored in AgentContext.extensions)
    Extensions,
    /// State change records
    StateChanges,
    /// Sub-agent results (independent namespace)
    SubagentResults,
}

impl std::fmt::Display for Category {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Category::Entities => write!(f, "Entities"),
            Category::Intentions => write!(f, "Intentions"),
            Category::ToolResults => write!(f, "ToolResults"),
            Category::QueryResults => write!(f, "QueryResults"),
            Category::WorkflowResults => write!(f, "WorkflowResults"),
            Category::ReasoningResults => write!(f, "ReasoningResults"),
            Category::Extensions => write!(f, "Extensions"),
            Category::StateChanges => write!(f, "StateChanges"),
            Category::SubagentResults => write!(f, "SubagentResults"),
        }
    }
}

/// A single record entry within a category.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordEntry {
    /// Record identifier (e.g., tool name, entity name)
    pub key: String,
    /// Record value
    pub value: serde_json::Value,
    /// Write source (tool name, skill name, subagent id)
    pub source: String,
    /// Write timestamp
    #[serde(with = "chrono::serde::ts_milliseconds")]
    pub timestamp: chrono::DateTime<chrono::Utc>,
    /// Agent execution iteration number
    pub iteration: usize,
}

/// Tool call status for audit logging.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolCallStatus {
    Success,
    Failure,
    Timeout,
}

impl std::fmt::Display for ToolCallStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ToolCallStatus::Success => write!(f, "success"),
            ToolCallStatus::Failure => write!(f, "failure"),
            ToolCallStatus::Timeout => write!(f, "timeout"),
        }
    }
}
