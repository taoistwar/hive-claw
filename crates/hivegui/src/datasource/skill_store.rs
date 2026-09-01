//! US12 Skill store (T108-T114 boundary).
//!
//! The public types in this file are the minimum surface the T108
//! Red tests require. Behaviour for full CRUD, content validation,
//! Plugin/Capability references, pagination and EXPLAIN live in
//! the T112 implementation pass.

#![warn(missing_docs)]

use std::sync::Arc;

use parking_lot::Mutex;
use sqlx::{Pool, Sqlite};
use thiserror::Error;

/// Validated input to a `SkillStore::create` call.
#[derive(Debug, Clone)]
pub struct SkillInput {
    name: String,
    content: String,
}

impl SkillInput {
    /// Construct a new input. The T108 minimum surface accepts
    /// any non-empty name and any content; the T112 pass adds
    /// content-not-empty and other field validation on
    /// [`SkillStore::create`].
    pub fn new(
        name: impl Into<String>,
        content: impl Into<String>,
    ) -> Result<Self, SkillStoreError> {
        let name = name.into();
        let content = content.into();
        if name.is_empty() {
            return Err(SkillStoreError {
                kind: SkillStoreErrorKind::InvalidInput,
                field: "name".to_string(),
                references: Vec::new(),
            });
        }
        Ok(Self { name, content })
    }

    /// Name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Content.
    pub fn content(&self) -> &str {
        &self.content
    }
}

/// Persisted Skill record.
#[derive(Debug, Clone)]
pub struct SkillRecord {
    id: i64,
    name: String,
    content: String,
}

impl SkillRecord {
    /// Database id.
    pub fn id(&self) -> i64 {
        self.id
    }

    /// Name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Content.
    pub fn content(&self) -> &str {
        &self.content
    }
}

/// Failure modes for the Skill store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillStoreErrorKind {
    /// Identifier or content violated a store-level invariant.
    InvalidInput,
    /// Name already exists.
    Conflict,
    /// Underlying SQL error.
    Io,
}

impl SkillStoreErrorKind {
    /// Stable reason string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidInput => "invalid_input",
            Self::Conflict => "conflict",
            Self::Io => "io",
        }
    }
}

/// Error envelope.
#[derive(Debug, Clone, Error)]
#[error("skill store error: {kind}", kind = self.kind.as_str())]
pub struct SkillStoreError {
    kind: SkillStoreErrorKind,
    field: String,
    references: Vec<String>,
}

impl SkillStoreError {
    /// Field that triggered the failure, when known.
    pub fn field(&self) -> &str {
        &self.field
    }

    /// Stable reason string.
    pub fn reason(&self) -> &str {
        self.kind.as_str()
    }

    /// Conflicting references.
    pub fn references(&self) -> &[String] {
        &self.references
    }
}

/// Skill store handle.
#[derive(Debug, Clone)]
pub struct SkillStore {
    inner: Arc<SkillStoreInner>,
}

#[derive(Debug)]
struct SkillStoreInner {
    records: Mutex<Vec<SkillRecord>>,
    next_id: Mutex<i64>,
}

impl SkillStore {
    /// Open a new store against the given pool.
    pub fn new(_pool: Pool<Sqlite>) -> Result<Self, SkillStoreError> {
        Ok(Self {
            inner: Arc::new(SkillStoreInner {
                records: Mutex::new(Vec::new()),
                next_id: Mutex::new(1),
            }),
        })
    }

    /// Create a new Skill record. Rejects empty content with
    /// `field = "content"` and duplicate names with
    /// `field = "name"`.
    pub fn create(&self, input: SkillInput) -> Result<SkillRecord, SkillStoreError> {
        if input.content.is_empty() {
            return Err(SkillStoreError {
                kind: SkillStoreErrorKind::InvalidInput,
                field: "content".to_string(),
                references: Vec::new(),
            });
        }
        let mut records = self.inner.records.lock();
        for existing in records.iter() {
            if existing.name == input.name {
                return Err(SkillStoreError {
                    kind: SkillStoreErrorKind::Conflict,
                    field: "name".to_string(),
                    references: vec![existing.name.clone()],
                });
            }
        }
        let mut next_id = self.inner.next_id.lock();
        let record = SkillRecord {
            id: *next_id,
            name: input.name,
            content: input.content,
        };
        *next_id += 1;
        records.push(record.clone());
        Ok(record)
    }
}
