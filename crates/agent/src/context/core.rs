//! AgentContext: unified state carrier for agent execution lifecycle.
//!
//! This is the core module implementing the Agent Context feature.
//! See `specs/009-agent-context/spec.md` for the full specification.

use std::collections::HashMap;
use std::sync::RwLock;

use chrono::Utc;
use serde::{Deserialize, Serialize};

use super::audit::{AuditLogger, AuditRecord, SkillExecutionStatus, StateChangeLog};
use super::category::{Category, RecordEntry, ToolCallStatus};
// Re-export for mod.rs convenience
pub use super::category::{
    Category as CategoryType, RecordEntry as RecordEntryType, ToolCallStatus as ToolCallStatusType,
};
use super::config::ContextConfig;
use super::lock::CategoryLock;
use super::response::{ExtensionContent, ResponsePayload};

// --- Sensitive data guard patterns (FR-021) ---

const SENSITIVE_FIELD_PATTERNS: &[&str] = &[
    "api_key",
    "token",
    "password",
    "secret",
    "credential",
    "authorization",
    "private_key",
];

fn is_sensitive_key(key: &str) -> bool {
    let lower = key.to_lowercase();
    SENSITIVE_FIELD_PATTERNS.iter().any(|&p| lower.contains(p))
}

fn is_sensitive_value(value: &serde_json::Value) -> bool {
    if let serde_json::Value::Object(map) = value {
        map.keys().any(|k| is_sensitive_key(k))
    } else {
        false
    }
}

// --- User Input ---

/// User request information stored in AgentContext.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInput {
    /// Raw input text from the user
    pub raw_text: String,
    /// Session identifier
    pub session_id: Option<String>,
    /// Message identifier
    pub message_id: Option<String>,
    /// Request timestamp
    #[serde(with = "chrono::serde::ts_milliseconds")]
    pub timestamp: chrono::DateTime<chrono::Utc>,
    /// Request metadata (source channel, language preference, etc.)
    pub metadata: HashMap<String, String>,
}

// --- Lifecycle State ---

/// Lifecycle state of an AgentContext execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LifecycleState {
    /// Active execution
    Active,
    /// Execution completed successfully
    Completed,
    /// Execution terminated (error, timeout, or interruption)
    Terminated,
    /// Currently merging sub-agent results
    Merging,
}

impl std::fmt::Display for LifecycleState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LifecycleState::Active => write!(f, "Active"),
            LifecycleState::Completed => write!(f, "Completed"),
            LifecycleState::Terminated => write!(f, "Terminated"),
            LifecycleState::Merging => write!(f, "Merging"),
        }
    }
}

impl LifecycleState {
    /// Check if a transition from `self` to `target` is valid.
    pub fn is_valid_transition(self, target: LifecycleState) -> bool {
        matches!(
            (self, target),
            (LifecycleState::Active, LifecycleState::Completed)
                | (LifecycleState::Active, LifecycleState::Terminated)
                | (LifecycleState::Active, LifecycleState::Merging)
                | (LifecycleState::Merging, LifecycleState::Active)
                | (LifecycleState::Merging, LifecycleState::Completed)
        )
    }
}

// --- Context Error ---

/// Error types for AgentContext operations.
#[derive(Debug, thiserror::Error)]
pub enum ContextError {
    #[error("Category not found: {0}")]
    CategoryNotFound(Category),

    #[error("Record not found: category={category}, key={key}")]
    RecordNotFound { category: Category, key: String },

    #[error(
        "Sub-agent attempted write to blacklisted category: {category} (subagent={subagent_id})"
    )]
    BlacklistedCategoryWrite {
        category: Category,
        subagent_id: String,
    },

    #[error("Illegal lifecycle state transition: {from} → {to}")]
    IllegalStateTransition {
        from: LifecycleState,
        to: LifecycleState,
    },

    #[error("Merge failed: {0}")]
    MergeFailed(String),

    #[error("Sensitive data rejected: field={field_name}, reason={reason}")]
    RejectedSensitiveData { field_name: String, reason: String },

    #[error("Invalid blacklist for sub-agent: category={category}, reason={reason}")]
    InvalidBlacklist { category: Category, reason: String },

    #[error("Schema version mismatch: expected={expected}, found={found}")]
    SchemaVersionMismatch { expected: String, found: String },
}

// --- CategoryStore ---

/// Per-category store with its own write lock.
#[derive(Debug)]
pub struct CategoryStore {
    lock: CategoryLock<Vec<RecordEntry>>,
    soft_limit: usize,
    warning_emitted: std::sync::atomic::AtomicBool,
}

impl CategoryStore {
    fn new(soft_limit: usize) -> Self {
        Self {
            lock: CategoryLock::new(Vec::new()),
            soft_limit,
            warning_emitted: std::sync::atomic::AtomicBool::new(false),
        }
    }
}

// --- AgentContext ---

/// The unified state container for a single agent execution.
///
/// Semi-structured design: core fields are strongly typed, with a flexible
/// extensions area for runtime custom data. Thread-safe via per-category
/// `Arc<RwLock>` write locks (FR-012).
///
/// Implements `Send + Sync` for cross-thread sharing.
pub struct AgentContext {
    /// Context unique identifier (matches agent execution ID)
    pub(crate) context_id: String,
    /// User request information
    pub(crate) user_input: UserInput,
    /// Per-category storage with independent write locks
    pub(crate) categories: HashMap<Category, CategoryStore>,
    /// Dynamic extension content
    pub(crate) extensions: RwLock<HashMap<String, ExtensionContent>>,
    /// Audit log
    pub(crate) audit_log: RwLock<Vec<AuditRecord>>,
    /// Final response payload
    pub(crate) response_payload: RwLock<Option<ResponsePayload>>,
    /// Current lifecycle state
    pub(crate) lifecycle_state: RwLock<LifecycleState>,
    /// Creation timestamp
    #[allow(dead_code)]
    pub(crate) created_at: chrono::DateTime<chrono::Utc>,
    /// Custom metadata
    pub(crate) metadata: RwLock<HashMap<String, String>>,
    /// Configuration
    pub(crate) config: ContextConfig,
    /// Sub-agent blacklist (present only for forked contexts)
    pub(crate) blacklisted_categories: Option<Vec<Category>>,
    /// Sub-agent ID (present only for forked contexts)
    pub(crate) subagent_id: Option<String>,
    /// Delegation chain prefix (e.g., "sub-001/sub-002/")
    pub(crate) delegation_chain: String,
    /// Audit logger (internal helper)
    pub(crate) audit_logger: AuditLogger,
}

unsafe impl Send for AgentContext {}
unsafe impl Sync for AgentContext {}

impl AgentContext {
    /// Create a new AgentContext.
    ///
    /// # Arguments
    /// * `context_id` — Unique identifier for this context
    /// * `user_input` — User request information
    /// * `config` — Configuration (soft limits, token budget, etc.)
    pub fn new(context_id: String, user_input: UserInput, config: ContextConfig) -> Self {
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
            categories.insert(cat, CategoryStore::new(config.soft_limit));
        }

        Self {
            context_id,
            user_input,
            categories,
            extensions: RwLock::new(HashMap::new()),
            audit_log: RwLock::new(Vec::new()),
            response_payload: RwLock::new(None),
            lifecycle_state: RwLock::new(LifecycleState::Active),
            created_at: Utc::now(),
            metadata: RwLock::new(HashMap::new()),
            config,
            blacklisted_categories: None,
            subagent_id: None,
            delegation_chain: String::new(),
            audit_logger: AuditLogger::new(),
        }
    }

    // --- Read Operations ---

    /// Get user input (immutable reference).
    pub fn user_input(&self) -> &UserInput {
        &self.user_input
    }

    /// Get all records for a category (acquires read lock).
    pub fn get_category(&self, category: Category) -> Result<Vec<RecordEntry>, ContextError> {
        let store = self
            .categories
            .get(&category)
            .ok_or(ContextError::CategoryNotFound(category))?;
        let guard = store
            .lock
            .read()
            .map_err(|_| ContextError::MergeFailed("Read lock poisoned".into()))?;
        Ok(guard.clone())
    }

    /// Get a specific record by category and key (acquires read lock).
    pub fn get_record(
        &self,
        category: Category,
        key: &str,
    ) -> Result<Option<RecordEntry>, ContextError> {
        let records = self.get_category(category)?;
        Ok(records.into_iter().find(|r| r.key == key))
    }

    /// Get all extension content.
    pub fn get_extensions(&self) -> Vec<ExtensionContent> {
        self.extensions
            .read()
            .map(|g| g.values().cloned().collect())
            .unwrap_or_default()
    }

    /// Get the current lifecycle state.
    pub fn lifecycle_state(&self) -> LifecycleState {
        self.lifecycle_state
            .read()
            .map(|g| *g)
            .unwrap_or(LifecycleState::Active)
    }

    /// Get the response payload (if set).
    pub fn get_response_payload(&self) -> Option<ResponsePayload> {
        self.response_payload.read().ok().and_then(|g| g.clone())
    }

    /// Get a value from user_input.metadata by key (e.g., "actor_id").
    pub fn get_user_metadata(&self, key: &str) -> Option<String> {
        self.user_input.metadata.get(key).cloned()
    }

    /// Create a read-only view for concurrent access.
    pub fn read_view(&self) -> super::read_view::ReadView {
        super::read_view::ReadView::new(self)
    }

    // --- Write Operations ---

    /// Set a record in a category (overwrites existing key if present).
    ///
    /// Records a StateChangeLog on overwrite (FR-011).
    /// Soft limit exceeded triggers warn! but does not reject (Edge Case).
    pub fn set_record(
        &self,
        category: Category,
        key: String,
        value: serde_json::Value,
        source: String,
        iteration: usize,
    ) -> Result<(), ContextError> {
        self.check_write_allowed()?;
        self.check_blacklist(category)?;
        self.check_sensitive_data(&key, &value)?;

        let store = self
            .categories
            .get(&category)
            .ok_or(ContextError::CategoryNotFound(category))?;
        let mut guard = store
            .lock
            .write()
            .map_err(|_| ContextError::MergeFailed("Write lock poisoned".into()))?;

        // Check for existing record and log state change
        if let Some(existing) = guard.iter().find(|r| r.key == key) {
            let old_value = existing.value.clone();
            self.audit_logger.record_state_change(
                &mut self.audit_log.write().unwrap(),
                category,
                key.clone(),
                Some(old_value),
                value.clone(),
                source.clone(),
                iteration,
            );
        }

        // Check soft limit
        if guard.len() >= store.soft_limit
            && !store
                .warning_emitted
                .load(std::sync::atomic::Ordering::Relaxed)
        {
            store
                .warning_emitted
                .store(true, std::sync::atomic::Ordering::Relaxed);
            log::warn!(
                "Category {} soft limit exceeded: current={}, limit={}, source={}, iteration={}",
                category,
                guard.len(),
                store.soft_limit,
                source,
                iteration
            );
        }

        // Update or insert
        if let Some(entry) = guard.iter_mut().find(|r| r.key == key) {
            entry.value = value;
            entry.source = source;
            entry.timestamp = Utc::now();
            entry.iteration = iteration;
        } else {
            guard.push(RecordEntry {
                key,
                value,
                source,
                timestamp: Utc::now(),
                iteration,
            });
        }

        Ok(())
    }

    /// Append a record to a category (non-overlapping write).
    pub fn append_record(
        &self,
        category: Category,
        key: String,
        value: serde_json::Value,
        source: String,
        iteration: usize,
    ) -> Result<(), ContextError> {
        self.check_write_allowed()?;
        self.check_blacklist(category)?;
        self.check_sensitive_data(&key, &value)?;

        let store = self
            .categories
            .get(&category)
            .ok_or(ContextError::CategoryNotFound(category))?;
        let mut guard = store
            .lock
            .write()
            .map_err(|_| ContextError::MergeFailed("Write lock poisoned".into()))?;

        if guard.len() >= store.soft_limit
            && !store
                .warning_emitted
                .load(std::sync::atomic::Ordering::Relaxed)
        {
            store
                .warning_emitted
                .store(true, std::sync::atomic::Ordering::Relaxed);
            log::warn!(
                "Category {} soft limit exceeded on append: current={}, limit={}, source={}, iteration={}",
                category,
                guard.len(),
                store.soft_limit,
                source,
                iteration
            );
        }

        guard.push(RecordEntry {
            key,
            value,
            source,
            timestamp: Utc::now(),
            iteration,
        });

        Ok(())
    }

    /// Add extension content.
    pub fn add_extension(&self, id: String, content: ExtensionContent) -> Result<(), ContextError> {
        self.check_write_allowed()?;
        let mut extensions = self
            .extensions
            .write()
            .map_err(|_| ContextError::MergeFailed("Extensions lock poisoned".into()))?;
        extensions.insert(id, content);
        Ok(())
    }

    /// Set the lifecycle state.
    pub fn set_lifecycle_state(&self, target: LifecycleState) -> Result<(), ContextError> {
        let mut state = self
            .lifecycle_state
            .write()
            .map_err(|_| ContextError::MergeFailed("Lifecycle lock poisoned".into()))?;
        let current = *state;
        if !current.is_valid_transition(target) {
            return Err(ContextError::IllegalStateTransition {
                from: current,
                to: target,
            });
        }
        *state = target;
        Ok(())
    }

    /// Set the final response payload.
    pub fn set_response_payload(&self, payload: ResponsePayload) -> Result<(), ContextError> {
        self.check_write_allowed()?;
        let mut response = self
            .response_payload
            .write()
            .map_err(|_| ContextError::MergeFailed("Response lock poisoned".into()))?;
        *response = Some(payload);
        Ok(())
    }

    /// Set a custom metadata key-value pair.
    pub fn set_metadata(&self, key: String, value: String) -> Result<(), ContextError> {
        let mut meta = self
            .metadata
            .write()
            .map_err(|_| ContextError::MergeFailed("Metadata lock poisoned".into()))?;
        meta.insert(key, value);
        Ok(())
    }

    /// Get a metadata value by key.
    pub fn get_metadata(&self, key: &str) -> Option<String> {
        self.metadata
            .read()
            .ok()
            .and_then(|m| m.get(key).cloned())
    }

    // --- Audit Operations ---

    /// Record a tool call result (FR-011).
    pub fn record_tool_call(
        &self,
        tool_name: String,
        arguments: serde_json::Value,
        result: Option<serde_json::Value>,
        status: ToolCallStatus,
        started_at: chrono::DateTime<chrono::Utc>,
        completed_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<(), ContextError> {
        self.check_write_allowed()?;
        self.audit_logger.record_tool_call(
            &mut self.audit_log.write().unwrap(),
            tool_name,
            arguments,
            result,
            status,
            started_at,
            completed_at,
        );
        Ok(())
    }

    /// Record a skill execution result (FR-011).
    pub fn record_skill_execution(
        &self,
        skill_name: String,
        input: serde_json::Value,
        output: Option<serde_json::Value>,
        status: SkillExecutionStatus,
        started_at: chrono::DateTime<chrono::Utc>,
        completed_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<(), ContextError> {
        self.check_write_allowed()?;
        self.audit_logger.record_skill_execution(
            &mut self.audit_log.write().unwrap(),
            skill_name,
            input,
            output,
            status,
            started_at,
            completed_at,
        );
        Ok(())
    }

    /// Record an agent delegation (sub-agent invocation) in the audit log.
    ///
    /// Public API for external orchestrators to record sub-agent fork/merge events.
    pub fn record_delegation(
        &self,
        subagent_id: String,
        delegation_chain: String,
        input: serde_json::Value,
        output: Option<serde_json::Value>,
        success: bool,
        started_at: chrono::DateTime<chrono::Utc>,
        completed_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<(), ContextError> {
        self.check_write_allowed()?;
        self.audit_logger.record_delegation(
            &mut self.audit_log.write().unwrap(),
            subagent_id,
            delegation_chain,
            input,
            output,
            success,
            started_at,
            completed_at,
        );
        Ok(())
    }

    /// Get the full audit log (snapshot).
    pub fn get_audit_log(&self) -> Vec<AuditRecord> {
        self.audit_log.read().map(|g| g.clone()).unwrap_or_default()
    }

    /// Get state change log entries.
    pub fn get_state_changes(&self) -> Vec<StateChangeLog> {
        self.audit_log
            .read()
            .map(|g| {
                g.iter()
                    .filter_map(|r| match r {
                        AuditRecord::StateChange(log) => Some(log.clone()),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    // --- Internal helpers ---

    fn check_write_allowed(&self) -> Result<(), ContextError> {
        let state = self.lifecycle_state();
        match state {
            LifecycleState::Active | LifecycleState::Merging => Ok(()),
            LifecycleState::Completed | LifecycleState::Terminated => {
                Err(ContextError::IllegalStateTransition {
                    from: state,
                    to: LifecycleState::Active,
                })
            }
        }
    }

    fn check_blacklist(&self, category: Category) -> Result<(), ContextError> {
        if let (Some(blacklist), Some(subagent_id)) =
            (&self.blacklisted_categories, &self.subagent_id)
        {
            if blacklist.contains(&category) {
                return Err(ContextError::BlacklistedCategoryWrite {
                    category,
                    subagent_id: subagent_id.clone(),
                });
            }
        }
        Ok(())
    }

    fn check_sensitive_data(
        &self,
        key: &str,
        value: &serde_json::Value,
    ) -> Result<(), ContextError> {
        if is_sensitive_key(key) {
            return Err(ContextError::RejectedSensitiveData {
                field_name: key.to_string(),
                reason: "Key matches sensitive field pattern".into(),
            });
        }
        if is_sensitive_value(value) {
            return Err(ContextError::RejectedSensitiveData {
                field_name: key.to_string(),
                reason: "Value contains sensitive field keys".into(),
            });
        }
        Ok(())
    }
}

// --- Safety verification ---

#[cfg(test)]
mod safety_tests {
    use super::*;

    fn assert_send<T: Send>() {}
    fn assert_sync<T: Sync>() {}

    #[test]
    fn agent_context_is_send_and_sync() {
        assert_send::<AgentContext>();
        assert_sync::<AgentContext>();
    }
}
