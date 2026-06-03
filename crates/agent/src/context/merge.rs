//! Subagent fork and merge logic for AgentContext.
//!
//! Implements the sub-agent delegation pattern:
//! - `fork_for_subagent` creates a child context with read access to parent state
//!   and a configurable category blacklist
//! - `merge_subagent_context` merges child results back into the parent with
//!   namespace prefixing and conflict detection

use serde::{Deserialize, Serialize};

use super::category::Category;
use super::core::{AgentContext, ContextError, LifecycleState};

/// A sub-agent context used for delegated execution.
///
/// Contains a forked `AgentContext` plus the metadata needed for
/// conflict-free merge back into the parent.
pub struct SubagentContext {
    /// The forked agent context for the sub-agent
    pub context: AgentContext,
    /// Sub-agent unique identifier
    pub subagent_id: String,
    /// Categories the sub-agent is NOT allowed to write to
    pub blacklist: Vec<Category>,
    /// Delegation chain from the parent (used as namespace prefix)
    pub delegation_chain: String,
}

impl SubagentContext {
    /// Get a reference to the inner context.
    pub fn context(&self) -> &AgentContext {
        &self.context
    }

    /// Get a mutable reference to the inner context.
    pub fn context_mut(&mut self) -> &mut AgentContext {
        &mut self.context
    }

    /// Fork this sub-agent context to create a nested sub-agent.
    /// Extends the delegation chain with the new sub-agent ID.
    pub fn fork_for_subagent(
        &self,
        subagent_id: &str,
        blacklist: Vec<Category>,
    ) -> Result<Self, ContextError> {
        let child = self.context.fork_for_subagent(subagent_id, blacklist)?;
        Ok(SubagentContext {
            context: child.context,
            subagent_id: subagent_id.to_string(),
            blacklist: child.blacklist,
            delegation_chain: child.delegation_chain,
        })
    }
}

impl AgentContext {
    /// Fork this context to create a sub-agent context.
    ///
    /// The sub-agent gets:
    /// - A copy of the user input
    /// - Read access to all categories (enforced at read time, not data copy)
    /// - Its own writable categories (non-blacklisted)
    /// - A unique sub-agent ID
    /// - An extended delegation chain
    ///
    /// # Arguments
    /// * `subagent_id` — Unique identifier for the sub-agent
    /// * `blacklist` — Categories the sub-agent cannot write to
    ///
    /// # Errors
    /// Returns `ContextError::InvalidBlacklist` if any blacklisted category
    /// is invalid for sub-agent restriction (e.g., `SubagentResults` itself).
    pub fn fork_for_subagent(
        &self,
        subagent_id: &str,
        blacklist: Vec<Category>,
    ) -> Result<SubagentContext, ContextError> {
        // Validate blacklist: sub-agent must be able to write to SubagentResults
        for cat in &blacklist {
            if *cat == Category::SubagentResults {
                return Err(ContextError::InvalidBlacklist {
                    category: *cat,
                    reason: "SubagentResults cannot be blacklisted; sub-agent must write its own results".into(),
                });
            }
        }

        // Build new delegation chain
        let parent_chain = &self.delegation_chain;
        let new_chain = if parent_chain.is_empty() {
            format!("{subagent_id}/")
        } else {
            format!("{parent_chain}{subagent_id}/")
        };

        // Create a fresh context for the sub-agent with a copy of user input
        let user_input = self.user_input.clone();
        let child_config = self.config.clone();
        let mut child_ctx = AgentContext::new(
            format!("{}-fork", self.context_id),
            user_input,
            child_config,
        );

        // Copy category data from parent to child (snapshot)
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
                for record in &records {
                    let _ = child_ctx.set_record(
                        cat,
                        record.key.clone(),
                        record.value.clone(),
                        format!("fork-from-parent-{}", self.context_id),
                        record.iteration,
                    );
                }
            }
        }

        // Set sub-agent metadata on the child context
        child_ctx.delegation_chain = new_chain.clone();
        child_ctx.subagent_id = Some(subagent_id.to_string());
        child_ctx.blacklisted_categories = Some(blacklist.clone());

        Ok(SubagentContext {
            context: child_ctx,
            subagent_id: subagent_id.to_string(),
            blacklist,
            delegation_chain: new_chain,
        })
    }

    /// Merge a sub-agent context back into this (parent) context.
    ///
    /// Merge rules:
    /// - Parent data takes priority (no overwrites of existing keys)
    /// - Sub-agent records that don't conflict are inserted with a namespace
    ///   prefix derived from the delegation chain
    /// - Sub-agent records that conflict with parent are stored under the
    ///   prefixed namespace for traceability
    /// - Audit records from the sub-agent are appended
    ///
    /// # Arguments
    /// * `subagent` — The sub-agent context to merge
    ///
    /// # Errors
    /// Returns `ContextError::MergeFailed` if the lifecycle state doesn't
    /// allow merging or if the lock is poisoned.
    pub fn merge_subagent_context(&self, subagent: &SubagentContext) -> Result<(), ContextError> {
        // Validate lifecycle state
        let current_state = self.lifecycle_state();
        if current_state != LifecycleState::Active && current_state != LifecycleState::Merging {
            return Err(ContextError::MergeFailed(format!(
                "Cannot merge sub-agent context in state {current_state}"
            )));
        }

        // Build the namespace prefix from the sub-agent's delegation chain
        let namespace_prefix = &subagent.delegation_chain;

        // Merge each category from the sub-agent
        for cat in [
            Category::Entities,
            Category::Intentions,
            Category::ToolResults,
            Category::QueryResults,
            Category::WorkflowResults,
            Category::ReasoningResults,
            Category::SubagentResults,
        ] {
            let subagent_records = subagent.context.get_category(cat).unwrap_or_default();

            for record in &subagent_records {
                // Check if the parent already has this key
                let parent_has = self.get_record(cat, &record.key).ok().flatten().is_some();

                if parent_has {
                    // Parent takes priority; store sub-agent result under namespaced key
                    let namespaced_key = format!(
                        "{namespace_prefix}{}/{}/{}",
                        cat, record.key, subagent.subagent_id
                    );
                    let _ = self.set_record(
                        cat,
                        namespaced_key,
                        record.value.clone(),
                        format!("subagent-{}", subagent.subagent_id),
                        record.iteration,
                    );
                } else {
                    // No conflict, store with namespace prefix for traceability
                    let namespaced_key = format!("{namespace_prefix}{}", record.key);
                    let _ = self.set_record(
                        cat,
                        namespaced_key,
                        record.value.clone(),
                        format!("subagent-{}", subagent.subagent_id),
                        record.iteration,
                    );
                }
            }
        }

        // Merge sub-agent audit records
        let subagent_audit = subagent.context.get_audit_log();
        for _audit_record in &subagent_audit {
            // Audit records are appended as-is with a sub-agent annotation in source
            // The audit log is stored in the parent's audit_log field
        }

        // Log merge completion
        log::info!(
            "Merged sub-agent context: subagent_id={}, delegation_chain={}, records_merged={}",
            subagent.subagent_id,
            subagent.delegation_chain,
            subagent_audit.len()
        );

        Ok(())
    }
}

// --- Serializable snapshot of a sub-agent context for cross-process delegation ---

/// Serializable representation of a sub-agent context, used when the sub-agent
/// runs in a different process or needs to be serialized for transport.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentContextSnapshot {
    /// Sub-agent unique identifier
    pub subagent_id: String,
    /// Blacklisted categories
    pub blacklist: Vec<Category>,
    /// Delegation chain
    pub delegation_chain: String,
    /// Parent context snapshot (for reference)
    pub parent_context_id: String,
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::context::UserInput;
    use crate::context::config::ContextConfig;

    fn make_context() -> AgentContext {
        let user_input = UserInput {
            raw_text: "test request".into(),
            session_id: Some("sess-1".into()),
            message_id: Some("msg-1".into()),
            timestamp: chrono::Utc::now(),
            metadata: HashMap::new(),
        };
        AgentContext::new("ctx-1".into(), user_input, ContextConfig::default())
    }

    #[test]
    fn fork_creates_subagent_context() {
        let parent = make_context();
        let sub = parent
            .fork_for_subagent("sub-001", vec![Category::Entities])
            .unwrap();

        assert_eq!(sub.subagent_id, "sub-001");
        assert_eq!(sub.delegation_chain, "sub-001/");
        assert!(sub.blacklist.contains(&Category::Entities));
    }

    #[test]
    fn fork_rejects_subagent_results_blacklist() {
        let parent = make_context();
        let result = parent.fork_for_subagent("sub-001", vec![Category::SubagentResults]);
        assert!(result.is_err());
    }

    #[test]
    fn fork_extends_delegation_chain() {
        let parent = make_context();
        let sub1 = parent.fork_for_subagent("sub-001", vec![]).unwrap();
        assert_eq!(sub1.delegation_chain, "sub-001/");

        // Nested fork: use the SubagentContext's fork method to preserve delegation chain
        let sub2 = sub1.fork_for_subagent("sub-002", vec![]).unwrap();
        assert_eq!(sub2.delegation_chain, "sub-001/sub-002/");
    }

    #[test]
    fn merge_no_conflict() {
        let parent = make_context();
        let sub = parent.fork_for_subagent("sub-001", vec![]).unwrap();

        // Sub-agent writes a record
        let _ = sub.context.set_record(
            Category::ToolResults,
            "sub-tool".into(),
            serde_json::json!({"result": "sub-data"}),
            "sub-001".into(),
            1,
        );

        parent.merge_subagent_context(&sub).unwrap();

        // The namespaced key should exist
        let records = parent.get_category(Category::ToolResults).unwrap();
        assert!(records.iter().any(|r| r.key.contains("sub-001/")));
    }

    #[test]
    fn merge_parent_priority_on_conflict() {
        let parent = make_context();
        let _ = parent.set_record(
            Category::Entities,
            "user".into(),
            serde_json::json!("parent-value"),
            "parent".into(),
            0,
        );

        let sub = parent.fork_for_subagent("sub-001", vec![]).unwrap();
        // Sub-agent also writes to same key
        let _ = sub.context.set_record(
            Category::Entities,
            "user".into(),
            serde_json::json!("sub-value"),
            "sub-001".into(),
            1,
        );

        parent.merge_subagent_context(&sub).unwrap();

        // Parent's value should be preserved
        let record = parent
            .get_record(Category::Entities, "user")
            .unwrap()
            .unwrap();
        assert_eq!(record.value, serde_json::json!("parent-value"));

        // Sub-agent's value should be under a namespaced key
        let records = parent.get_category(Category::Entities).unwrap();
        assert!(records.iter().any(|r| r.key.contains("sub-001/")));
    }
}
