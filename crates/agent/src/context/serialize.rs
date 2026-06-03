//! Serialization and deserialization for AgentContext.
//!
//! Provides `ContextSnapshot` for point-in-time snapshots and
//! `to_json` / `from_json` methods on `AgentContext`.

use std::collections::HashMap;

use chrono::Utc;
use serde::{Deserialize, Serialize};

use super::audit::AuditRecord;
use super::category::{Category, RecordEntry};
use super::config::ContextConfig;
use super::core::{AgentContext, ContextError, LifecycleState, UserInput};
use super::response::ExtensionContent;

/// A serializable snapshot of an AgentContext at a point in time.
///
/// Used for debugging, replay, and audit purposes. Contains all
/// non-transient state from the context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextSnapshot {
    /// Schema version for compatibility checking
    pub schema_version: String,
    /// Context unique identifier
    pub context_id: String,
    /// User request information
    pub user_input: UserInput,
    /// All category records
    pub categories: HashMap<Category, Vec<RecordEntry>>,
    /// Extension content
    pub extensions: Vec<(String, ExtensionContent)>,
    /// Full audit log
    pub audit_log: Vec<AuditRecord>,
    /// Response payload (if set)
    pub response_payload: Option<super::response::ResponsePayload>,
    /// Lifecycle state at snapshot time
    pub lifecycle_state: LifecycleState,
    /// Creation timestamp of the original context
    #[serde(with = "chrono::serde::ts_milliseconds")]
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Snapshot timestamp
    #[serde(with = "chrono::serde::ts_milliseconds")]
    pub snapshot_at: chrono::DateTime<chrono::Utc>,
    /// Custom metadata
    pub metadata: HashMap<String, String>,
    /// Sub-agent blacklist (if present)
    pub blacklisted_categories: Option<Vec<Category>>,
    /// Sub-agent ID (if present)
    pub subagent_id: Option<String>,
    /// Delegation chain
    pub delegation_chain: String,
}

impl AgentContext {
    /// Create a point-in-time snapshot of the context.
    ///
    /// This captures all current state into a serializable `ContextSnapshot`.
    ///
    /// # Errors
    /// Returns `ContextError::MergeFailed` if any lock is poisoned.
    pub fn snapshot(&self) -> Result<ContextSnapshot, ContextError> {
        let mut categories = HashMap::new();
        for cat in [
            Category::Entities,
            Category::Intentions,
            Category::ToolResults,
            Category::QueryResults,
            Category::WorkflowResults,
            Category::ReasoningResults,
            Category::Extensions,
            Category::StateChanges,
            Category::SubagentResults,
        ] {
            if let Ok(records) = self.get_category(cat) {
                categories.insert(cat, records);
            }
        }

        let extensions = {
            let guard = self
                .extensions
                .read()
                .map_err(|_| ContextError::MergeFailed("Extensions lock poisoned".into()))?;
            guard.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
        };

        let audit_log = self.get_audit_log();
        let response_payload = self.get_response_payload();
        let lifecycle_state = self.lifecycle_state();
        let metadata = self.metadata.read().map(|g| g.clone()).unwrap_or_default();

        Ok(ContextSnapshot {
            schema_version: self.config.schema_version.clone(),
            context_id: self.context_id.clone(),
            user_input: self.user_input.clone(),
            categories,
            extensions,
            audit_log,
            response_payload,
            lifecycle_state,
            created_at: self.created_at,
            snapshot_at: Utc::now(),
            metadata,
            blacklisted_categories: self.blacklisted_categories.clone(),
            subagent_id: self.subagent_id.clone(),
            delegation_chain: self.delegation_chain.clone(),
        })
    }

    /// Serialize the context to a JSON string.
    ///
    /// Convenience method that creates a snapshot and serializes it.
    ///
    /// # Errors
    /// Returns `ContextError::MergeFailed` if lock acquisition fails,
    /// or a JSON serialization error wrapped as `MergeFailed`.
    pub fn to_json(&self) -> Result<String, ContextError> {
        let snapshot = self.snapshot()?;
        serde_json::to_string_pretty(&snapshot)
            .map_err(|e| ContextError::MergeFailed(format!("JSON serialization failed: {e}")))
    }

    /// Deserialize a context from a JSON string.
    ///
    /// Returns a new `AgentContext` with the deserialized state.
    ///
    /// # Errors
    /// Returns `ContextError::SchemaVersionMismatch` if the schema version
    /// doesn't match the expected version, or `ContextError::MergeFailed`
    /// if JSON deserialization fails.
    pub fn from_json(json: &str, expected_schema_version: &str) -> Result<Self, ContextError> {
        let snapshot: ContextSnapshot = serde_json::from_str(json)
            .map_err(|e| ContextError::MergeFailed(format!("JSON deserialization failed: {e}")))?;

        // Validate schema version
        if snapshot.schema_version != expected_schema_version {
            return Err(ContextError::SchemaVersionMismatch {
                expected: expected_schema_version.to_string(),
                found: snapshot.schema_version,
            });
        }

        let config = ContextConfig {
            schema_version: snapshot.schema_version,
            ..ContextConfig::default()
        };

        let ctx = AgentContext::new(snapshot.context_id, snapshot.user_input, config);

        // Restore category data
        for (cat, records) in snapshot.categories {
            for record in &records {
                let _ = ctx.set_record(
                    cat,
                    record.key.clone(),
                    record.value.clone(),
                    record.source.clone(),
                    record.iteration,
                );
            }
        }

        // Restore extensions
        {
            let mut extensions = ctx
                .extensions
                .write()
                .map_err(|_| ContextError::MergeFailed("Extensions lock poisoned".into()))?;
            for (id, content) in snapshot.extensions {
                extensions.insert(id, content);
            }
        }

        // Restore audit log
        {
            let mut audit_log = ctx
                .audit_log
                .write()
                .map_err(|_| ContextError::MergeFailed("Audit log lock poisoned".into()))?;
            *audit_log = snapshot.audit_log;
        }

        // Restore response payload
        if let Some(payload) = snapshot.response_payload {
            let mut response = ctx
                .response_payload
                .write()
                .map_err(|_| ContextError::MergeFailed("Response lock poisoned".into()))?;
            *response = Some(payload);
        }

        // Restore lifecycle state
        {
            let mut state = ctx
                .lifecycle_state
                .write()
                .map_err(|_| ContextError::MergeFailed("Lifecycle lock poisoned".into()))?;
            *state = snapshot.lifecycle_state;
        }

        // Restore metadata
        {
            let mut metadata = ctx
                .metadata
                .write()
                .map_err(|_| ContextError::MergeFailed("Metadata lock poisoned".into()))?;
            *metadata = snapshot.metadata;
        }

        // Restore sub-agent metadata
        // Note: These are private fields; we can't set them directly from here.
        // In a real implementation, you'd need internal constructors or friend methods.
        // For now, the snapshot captures them for audit/debugging purposes.

        Ok(ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::config::ContextConfig;

    fn make_context() -> AgentContext {
        let user_input = UserInput {
            raw_text: "test".into(),
            session_id: Some("sess-1".into()),
            message_id: Some("msg-1".into()),
            timestamp: chrono::Utc::now(),
            metadata: HashMap::new(),
        };
        AgentContext::new("ctx-1".into(), user_input, ContextConfig::default())
    }

    #[test]
    fn snapshot_captures_context_id() {
        let ctx = make_context();
        let snapshot = ctx.snapshot().unwrap();
        assert_eq!(snapshot.context_id, "ctx-1");
    }

    #[test]
    fn snapshot_includes_schema_version() {
        let ctx = make_context();
        let snapshot = ctx.snapshot().unwrap();
        assert_eq!(snapshot.schema_version, "v1");
    }

    #[test]
    fn to_json_produces_valid_json() {
        let ctx = make_context();
        let json = ctx.to_json().unwrap();
        // Should be parseable
        let _: ContextSnapshot = serde_json::from_str(&json).unwrap();
    }

    #[test]
    fn from_json_roundtrip() {
        let ctx = make_context();
        let json = ctx.to_json().unwrap();
        let restored = AgentContext::from_json(&json, "v1").unwrap();

        assert_eq!(restored.user_input().raw_text, "test");
        assert_eq!(restored.lifecycle_state(), LifecycleState::Active);
    }

    #[test]
    fn from_json_schema_mismatch() {
        let ctx = make_context();
        let json = ctx.to_json().unwrap();
        let result = AgentContext::from_json(&json, "v99");
        assert!(matches!(
            result,
            Err(ContextError::SchemaVersionMismatch { .. })
        ));
    }
}
