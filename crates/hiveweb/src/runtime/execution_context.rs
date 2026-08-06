//! Explicit correlation and audit-persistence context for one runtime call tree.
//!
//! The persistence mode is intentionally private runtime state. It is not
//! deserializable from an HTTP or Plugin payload and has no default.

/// Controls whether sanitized runtime audit events may be copied to MySQL.
///
/// This type deliberately does not implement `serde::Deserialize` or `Default`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AuditPersistenceMode {
    BestEffortDb,
    TracingOnly,
}

/// Correlation state propagated explicitly through the runtime call graph.
///
/// ```compile_fail
/// use hiveweb::runtime::execution_context::RuntimeExecutionContext;
/// let _ = RuntimeExecutionContext::default();
/// ```
///
/// ```compile_fail
/// use hiveweb::runtime::execution_context::RuntimeExecutionContext;
/// let _: RuntimeExecutionContext = serde_json::from_str("{}").unwrap();
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeExecutionContext {
    request_id: Option<String>,
    session_id: Option<i64>,
    persistence: AuditPersistenceMode,
}

impl RuntimeExecutionContext {
    /// Create the normal runtime context. DB persistence remains bounded and
    /// best-effort; tracing is always emitted.
    pub fn best_effort(request_id: Option<String>, session_id: Option<i64>) -> Self {
        Self {
            request_id,
            session_id,
            persistence: AuditPersistenceMode::BestEffortDb,
        }
    }

    /// Derive a Hook context while preserving correlation identifiers.
    ///
    /// Calling this on an already tracing-only context is intentionally sticky:
    /// no nested Hook → Workflow → Plugin → Capability path can re-enable DB
    /// persistence.
    pub fn for_hook(&self) -> Self {
        Self {
            request_id: self.request_id.clone(),
            session_id: self.session_id,
            persistence: AuditPersistenceMode::TracingOnly,
        }
    }

    pub fn request_id(&self) -> Option<&str> {
        self.request_id.as_deref()
    }

    pub fn session_id(&self) -> Option<i64> {
        self.session_id
    }

    pub(crate) fn persists_best_effort(&self) -> bool {
        self.persistence == AuditPersistenceMode::BestEffortDb
    }
}
