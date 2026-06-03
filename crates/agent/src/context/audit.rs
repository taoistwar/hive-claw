//! Audit logging for AgentContext.
//!
//! Provides structured audit records for tool calls, skill executions,
//! state changes, and agent delegations.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::category::Category;

/// Status of a skill execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SkillExecutionStatus {
    /// Skill executed successfully
    Success,
    /// Skill execution failed
    Failure,
    /// Skill execution was interrupted
    Interrupted,
    /// Skill execution timed out
    Timeout,
}

impl std::fmt::Display for SkillExecutionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SkillExecutionStatus::Success => write!(f, "success"),
            SkillExecutionStatus::Failure => write!(f, "failure"),
            SkillExecutionStatus::Interrupted => write!(f, "interrupted"),
            SkillExecutionStatus::Timeout => write!(f, "timeout"),
        }
    }
}

/// Record of a tool call execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRecord {
    /// Tool name
    pub tool_name: String,
    /// Tool arguments
    pub arguments: serde_json::Value,
    /// Tool result (None if not yet completed)
    pub result: Option<serde_json::Value>,
    /// Execution status
    pub status: super::category::ToolCallStatus,
    /// Start timestamp
    #[serde(with = "chrono::serde::ts_milliseconds")]
    pub started_at: DateTime<Utc>,
    /// Completion timestamp (None if still running)
    #[serde(
        default,
        with = "chrono::serde::ts_milliseconds_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub completed_at: Option<DateTime<Utc>>,
}

/// Record of a skill execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillExecutionRecord {
    /// Skill name
    pub skill_name: String,
    /// Skill input
    pub input: serde_json::Value,
    /// Skill output (None if not yet completed)
    pub output: Option<serde_json::Value>,
    /// Execution status
    pub status: SkillExecutionStatus,
    /// Start timestamp
    #[serde(with = "chrono::serde::ts_milliseconds")]
    pub started_at: DateTime<Utc>,
    /// Completion timestamp (None if still running)
    #[serde(
        default,
        with = "chrono::serde::ts_milliseconds_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub completed_at: Option<DateTime<Utc>>,
}

/// Log entry for a state change in the context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateChangeLog {
    /// Category that changed
    pub category: Category,
    /// Key that changed
    pub key: String,
    /// Previous value (None if new record)
    pub old_value: Option<serde_json::Value>,
    /// New value
    pub new_value: serde_json::Value,
    /// Source of the change (tool name, skill name, etc.)
    pub source: String,
    /// Iteration number when the change occurred
    pub iteration: usize,
    /// Timestamp of the change
    #[serde(with = "chrono::serde::ts_milliseconds")]
    pub changed_at: DateTime<Utc>,
}

/// Record of an agent delegation (sub-agent invocation).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDelegationRecord {
    /// Sub-agent identifier
    pub subagent_id: String,
    /// Delegation chain at time of delegation
    pub delegation_chain: String,
    /// Data passed to the sub-agent
    pub input: serde_json::Value,
    /// Data returned by the sub-agent
    pub output: Option<serde_json::Value>,
    /// Whether the delegation succeeded
    pub success: bool,
    /// Start timestamp
    #[serde(with = "chrono::serde::ts_milliseconds")]
    pub started_at: DateTime<Utc>,
    /// Completion timestamp
    #[serde(
        default,
        with = "chrono::serde::ts_milliseconds_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub completed_at: Option<DateTime<Utc>>,
}

/// Union of all audit record types.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuditRecord {
    /// A tool call execution
    ToolCall(ToolCallRecord),
    /// A skill execution
    SkillExecution(SkillExecutionRecord),
    /// A context state change
    StateChange(StateChangeLog),
    /// An agent delegation (sub-agent invocation)
    Delegation(AgentDelegationRecord),
}

impl AuditRecord {
    /// Get the timestamp of this audit record.
    pub fn timestamp(&self) -> DateTime<Utc> {
        match self {
            AuditRecord::ToolCall(r) => r.started_at,
            AuditRecord::SkillExecution(r) => r.started_at,
            AuditRecord::StateChange(r) => r.changed_at,
            AuditRecord::Delegation(r) => r.started_at,
        }
    }
}

/// Internal audit logger for AgentContext.
///
/// Provides helper methods to create and append audit records to the
/// context's audit log.
pub struct AuditLogger;

impl AuditLogger {
    /// Create a new `AuditLogger`.
    pub fn new() -> Self {
        Self
    }

    /// Record a tool call in the audit log.
    pub fn record_tool_call(
        &self,
        audit_log: &mut Vec<AuditRecord>,
        tool_name: String,
        arguments: serde_json::Value,
        result: Option<serde_json::Value>,
        status: super::category::ToolCallStatus,
        started_at: DateTime<Utc>,
        completed_at: Option<DateTime<Utc>>,
    ) {
        let record = ToolCallRecord {
            tool_name,
            arguments,
            result,
            status,
            started_at,
            completed_at,
        };
        audit_log.push(AuditRecord::ToolCall(record));
    }

    /// Record a skill execution in the audit log.
    pub fn record_skill_execution(
        &self,
        audit_log: &mut Vec<AuditRecord>,
        skill_name: String,
        input: serde_json::Value,
        output: Option<serde_json::Value>,
        status: SkillExecutionStatus,
        started_at: DateTime<Utc>,
        completed_at: Option<DateTime<Utc>>,
    ) {
        let record = SkillExecutionRecord {
            skill_name,
            input,
            output,
            status,
            started_at,
            completed_at,
        };
        audit_log.push(AuditRecord::SkillExecution(record));
    }

    /// Record a state change in the audit log.
    pub fn record_state_change(
        &self,
        audit_log: &mut Vec<AuditRecord>,
        category: Category,
        key: String,
        old_value: Option<serde_json::Value>,
        new_value: serde_json::Value,
        source: String,
        iteration: usize,
    ) {
        let record = StateChangeLog {
            category,
            key,
            old_value,
            new_value,
            source,
            iteration,
            changed_at: Utc::now(),
        };
        audit_log.push(AuditRecord::StateChange(record));
    }

    /// Record an agent delegation in the audit log.
    pub fn record_delegation(
        &self,
        audit_log: &mut Vec<AuditRecord>,
        subagent_id: String,
        delegation_chain: String,
        input: serde_json::Value,
        output: Option<serde_json::Value>,
        success: bool,
        started_at: DateTime<Utc>,
        completed_at: Option<DateTime<Utc>>,
    ) {
        let record = AgentDelegationRecord {
            subagent_id,
            delegation_chain,
            input,
            output,
            success,
            started_at,
            completed_at,
        };
        audit_log.push(AuditRecord::Delegation(record));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_call_record_serialization() {
        let record = ToolCallRecord {
            tool_name: "weather".into(),
            arguments: serde_json::json!({"city": "Beijing"}),
            result: Some(serde_json::json!({"temp": 25})),
            status: super::super::category::ToolCallStatus::Success,
            started_at: Utc::now(),
            completed_at: Some(Utc::now()),
        };

        let json = serde_json::to_string(&record).unwrap();
        let _: ToolCallRecord = serde_json::from_str(&json).unwrap();
    }

    #[test]
    fn audit_record_tagged_serialization() {
        let record = AuditRecord::ToolCall(ToolCallRecord {
            tool_name: "test".into(),
            arguments: serde_json::json!({}),
            result: None,
            status: super::super::category::ToolCallStatus::Success,
            started_at: Utc::now(),
            completed_at: None,
        });

        let json = serde_json::to_string(&record).unwrap();
        // Should contain the type tag
        assert!(json.contains("\"type\":\"tool_call\""));
    }

    #[test]
    fn state_change_log_has_all_fields() {
        let log = StateChangeLog {
            category: Category::Entities,
            key: "user".into(),
            old_value: Some(serde_json::json!("old")),
            new_value: serde_json::json!("new"),
            source: "test".into(),
            iteration: 1,
            changed_at: Utc::now(),
        };

        assert_eq!(log.category, Category::Entities);
        assert_eq!(log.key, "user");
    }

    #[test]
    fn audit_logger_records_tool_call() {
        let logger = AuditLogger::new();
        let mut log = Vec::new();

        logger.record_tool_call(
            &mut log,
            "test_tool".into(),
            serde_json::json!({}),
            Some(serde_json::json!("ok")),
            super::super::category::ToolCallStatus::Success,
            Utc::now(),
            Some(Utc::now()),
        );

        assert_eq!(log.len(), 1);
        assert!(matches!(&log[0], AuditRecord::ToolCall(_)));
    }
}
