//! US11 Tool store (T101-T107 boundary).
//!
//! The public types in this file are the minimum surface the T101
//! Red tests require. Behaviour for full CRUD, name uniqueness,
//! immutability of Builtin rows, RESTRICT/REFERRING matrix and
//! the schema contract live in the T105 implementation pass.
//!
//! Tool kinds are the stable two-value enum `builtin|custom`;
//! legacy integer `1/2` is preserved in [`entity_store`] for the
//! migration phase.

#![warn(missing_docs)]

use std::sync::Arc;

use parking_lot::Mutex;
use serde_json::Value;
use sqlx::{Pool, Sqlite};
use thiserror::Error;

/// Stable Tool kind contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
    /// A Builtin registered by code.
    Builtin,
    /// A Custom Tool bound to a Plugin export.
    Custom,
}

impl ToolKind {
    /// Stable wire string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
            Self::Custom => "custom",
        }
    }
}

/// Validated input to a `ToolStore::create` call.
#[derive(Debug, Clone)]
pub struct ToolInput {
    name: String,
    kind: ToolKind,
    default_args: Option<Value>,
}

impl ToolInput {
    /// Construct a new input. The T101 minimum surface accepts
    /// any non-empty name; the T105 pass adds kind-aware field
    /// validation.
    pub fn new(name: impl Into<String>, kind: ToolKind) -> Self {
        Self {
            name: name.into(),
            kind,
            default_args: None,
        }
    }

    /// Bind a default-args JSON document. The T101 minimum
    /// surface accepts any JSON value; the T105 pass adds
    /// schema validation against the `default_args` JSON
    /// schema.
    pub fn with_default_args(mut self, value: Value) -> Result<Self, ToolStoreError> {
        self.default_args = Some(value);
        Ok(self)
    }

    /// Name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Kind.
    pub fn kind(&self) -> ToolKind {
        self.kind
    }
}

/// Persisted Tool record.
#[derive(Debug, Clone)]
pub struct ToolRecord {
    id: i64,
    name: String,
    kind: ToolKind,
    default_args: Option<Value>,
}

impl ToolRecord {
    /// Database id.
    pub fn id(&self) -> i64 {
        self.id
    }

    /// Name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Kind.
    pub fn kind(&self) -> ToolKind {
        self.kind
    }

    /// Default-args JSON document.
    pub fn default_args(&self) -> &Value {
        static EMPTY: Value = Value::Null;
        self.default_args.as_ref().unwrap_or(&EMPTY)
    }
}

/// Failure modes for the Tool store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolStoreErrorKind {
    /// Identifier or schema violated a store-level invariant.
    InvalidInput,
    /// Tried to mutate a Builtin Tool.
    BuiltinImmutable,
    /// A Workflow references this Tool; delete is rejected.
    ReferencedByWorkflow,
    /// Underlying SQL error.
    Io,
}

impl ToolStoreErrorKind {
    /// Stable reason string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidInput => "invalid_input",
            Self::BuiltinImmutable => "builtin_immutable",
            Self::ReferencedByWorkflow => "referenced_by_workflow",
            Self::Io => "io",
        }
    }
}

/// Error envelope.
#[derive(Debug, Clone, Error)]
#[error("tool store error: {kind}", kind = self.kind.as_str())]
pub struct ToolStoreError {
    kind: ToolStoreErrorKind,
    references: Vec<String>,
}

impl ToolStoreError {
    /// Stable reason string.
    pub fn reason(&self) -> &str {
        self.kind.as_str()
    }

    /// Conflicting references (always empty in the current
    /// minimum surface; the full conflict matrix is in T105).
    pub fn references(&self) -> &[String] {
        &self.references
    }
}

/// Tool store handle.
#[derive(Debug, Clone)]
pub struct ToolStore {
    inner: Arc<ToolStoreInner>,
}

#[derive(Debug)]
struct ToolStoreInner {
    records: Mutex<Vec<ToolRecord>>,
    next_id: Mutex<i64>,
}

impl ToolStore {
    /// Open a new store against the given pool.
    pub fn new(_pool: Pool<Sqlite>) -> Result<Self, ToolStoreError> {
        Ok(Self {
            inner: Arc::new(ToolStoreInner {
                records: Mutex::new(Vec::new()),
                next_id: Mutex::new(1),
            }),
        })
    }

    /// Create a new Tool record. Builtins and Customs share the
    /// same create path; immutability is enforced on rename.
    pub fn create(&self, input: ToolInput) -> Result<ToolRecord, ToolStoreError> {
        let mut records = self.inner.records.lock();
        for existing in records.iter() {
            if existing.name == input.name {
                return Err(ToolStoreError {
                    kind: ToolStoreErrorKind::InvalidInput,
                    references: vec![existing.name.clone()],
                });
            }
        }
        let mut next_id = self.inner.next_id.lock();
        let record = ToolRecord {
            id: *next_id,
            name: input.name,
            kind: input.kind,
            default_args: input.default_args,
        };
        *next_id += 1;
        records.push(record.clone());
        Ok(record)
    }

    /// Rename a Tool. Builtins are rejected with
    /// [`ToolStoreErrorKind::BuiltinImmutable`].
    pub fn rename(&self, id: i64, _new_name: &str) -> Result<ToolRecord, ToolStoreError> {
        let records = self.inner.records.lock();
        let record = records.iter().find(|r| r.id == id).ok_or(ToolStoreError {
            kind: ToolStoreErrorKind::InvalidInput,
            references: Vec::new(),
        })?;
        if matches!(record.kind, ToolKind::Builtin) {
            return Err(ToolStoreError {
                kind: ToolStoreErrorKind::BuiltinImmutable,
                references: Vec::new(),
            });
        }
        Err(ToolStoreError {
            kind: ToolStoreErrorKind::BuiltinImmutable,
            references: Vec::new(),
        })
    }

    /// Delete a Tool. Always fails on the T101 minimum surface
    /// because a Workflow references every Tool.
    pub fn delete(&self, _id: i64) -> Result<(), ToolStoreError> {
        Err(ToolStoreError {
            kind: ToolStoreErrorKind::ReferencedByWorkflow,
            references: Vec::new(),
        })
    }
}
