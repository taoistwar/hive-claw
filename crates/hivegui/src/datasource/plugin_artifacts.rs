//! Plugin v4 durability ledger (operations + GC).
//!
//! The Foundation owns the schema of `plugin_artifact_operations`
//! and `plugin_artifact_gc`. Behaviour (create/replace flows,
//! GC triggers, lease management) is layered on top of this
//! schema in US8 T073-T082.
//!
//! The ledger is the single source of truth for: which Plugin
//! artifact operation is in flight, where its staging bytes live,
//! and which artifacts are eligible for garbage collection. Each
//! operation is bound by the `operation_id` and the deterministic
//! `staging_name` derived from it.

#![warn(missing_docs)]

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// State of a single plugin artifact operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OperationState {
    /// Initial state; both `staging_identity` and `new_identity` are NULL.
    Prepared,
    /// Staging has been written; only `staging_identity` may be non-null.
    Staged,
    /// Artifact has been published; both identities are non-null.
    Published,
    /// The artifact is in active use by a Plugin reference.
    Referenced,
    /// Terminal success state.
    Done,
    /// State in which the operation is held because of an
    /// identity/ownership conflict. The Store refuses to open.
    Conflict,
}

impl OperationState {
    /// Lower-case identifier used in the `state` SQL column.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::Staged => "staged",
            Self::Published => "published",
            Self::Referenced => "referenced",
            Self::Done => "done",
            Self::Conflict => "conflict",
        }
    }
}

/// Kind of plugin artifact operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OperationKind {
    /// First-time creation of a plugin.
    Create,
    /// Replacement of an existing plugin.
    Replace,
}

impl OperationKind {
    /// Lower-case identifier used in the `kind` SQL column.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Replace => "replace",
        }
    }
}

/// State of the GC record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GcState {
    /// GC has flagged the artifact but the work has not yet run.
    Pending,
    /// GC is blocked (conflict, identity drift, …).
    Blocked,
}

impl GcState {
    /// Lower-case identifier used in the `state` SQL column.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Blocked => "blocked",
        }
    }
}

/// Deterministic staging-name derivation: `staging-<sha256(operation_id)[:16]>`.
pub fn derive_staging_name(operation_id: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(operation_id.as_bytes());
    let digest = hasher.finalize();
    let hex: String = digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("staging-{hex}")
}

/// Failures emitted by the plugin-artifacts boundary.
#[derive(Debug, Clone, Error)]
pub enum PluginArtifactError {
    /// Operation id is empty.
    #[error("operation_id is empty")]
    EmptyOperationId,
    /// Stage transition is not allowed from the current state.
    #[error("illegal stage transition: {from} -> {to}")]
    IllegalTransition {
        /// Current state.
        from: &'static str,
        /// Target state.
        to: &'static str,
    },
    /// Underlying SQL error.
    #[error("plugin artifact sql error: {0}")]
    Sql(String),
}

/// In-memory record for a plugin artifact operation.
#[derive(Debug, Clone)]
pub struct OperationRecord {
    operation_id: String,
    kind: OperationKind,
    state: OperationState,
    staging_name: String,
}

impl OperationRecord {
    /// Construct a new operation record.
    pub fn new(
        operation_id: impl Into<String>,
        kind: OperationKind,
    ) -> Result<Self, PluginArtifactError> {
        let operation_id = operation_id.into();
        if operation_id.is_empty() {
            return Err(PluginArtifactError::EmptyOperationId);
        }
        let staging_name = derive_staging_name(&operation_id);
        Ok(Self {
            operation_id,
            kind,
            state: OperationState::Prepared,
            staging_name,
        })
    }

    /// Operation id.
    pub fn operation_id(&self) -> &str {
        &self.operation_id
    }

    /// Operation kind.
    pub fn kind(&self) -> OperationKind {
        self.kind
    }

    /// Operation state.
    pub fn state(&self) -> OperationState {
        self.state
    }

    /// Staging name (UNIQUE, derived from `operation_id`).
    pub fn staging_name(&self) -> &str {
        &self.staging_name
    }
}
