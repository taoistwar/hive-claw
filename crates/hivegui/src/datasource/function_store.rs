//! US9 Function store (T083-T089 boundary).
//!
//! The public types in this file are the minimum surface the T083
//! Red tests require. Behaviour for Builtin / Custom / Placeholder
//! CRUD, pagination, EXPLAIN, p95 perf and integration with the
//! shared runtime registry land in the T087 implementation pass.
//!
//! Function kinds are the stable three-value string set
//! `builtin|custom|placeholder`; the legacy integer `1/2/3` mapping
//! is preserved in [`entity_store`] for the migration phase.
//!
//! Reserved identifiers: the four underscore Builtins are exposed
//! via [`RESERVED_UNDERSCORE_IDENTIFIERS`]. Dotted names are
//! forbidden as records or aliases.

#![warn(missing_docs)]

use std::sync::Arc;

use parking_lot::Mutex;
use serde_json::Value;
use sqlx::{Pool, Sqlite, SqlitePool};
use thiserror::Error;

/// The four underscore Builtin identifiers reserved for the runtime
/// registry. Pointed lookups or store creations with these
/// identifiers MUST go through the registry, not the user-facing
/// store.
pub const RESERVED_UNDERSCORE_IDENTIFIERS: &[&str] = &[
    "format_template",
    "json_parse",
    "json_stringify",
    "text_regex_match",
];

/// Stable string kind for a [`FunctionRecord`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionKind {
    /// A read-only Builtin registered by code.
    Builtin,
    /// A Custom Function bound to a Plugin export.
    Custom,
    /// A schema-only Placeholder, never executable.
    Placeholder,
}

impl FunctionKind {
    /// Stable wire string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
            Self::Custom => "custom",
            Self::Placeholder => "placeholder",
        }
    }
}

/// Validated input to a `FunctionStore::create` call.
#[derive(Debug, Clone)]
pub struct FunctionInput {
    identifier: String,
    kind: FunctionKind,
    plugin_id: Option<i64>,
    export: Option<String>,
    schema: Option<Value>,
    required_capability: Option<String>,
}

impl FunctionInput {
    /// Construct a new input. Rejects empty identifiers; the
    /// dotted-identifier and kind-immutability checks live on
    /// [`FunctionStore::create`] so callers see stable error
    /// reasons from a single boundary.
    pub fn new(
        identifier: impl Into<String>,
        kind: FunctionKind,
    ) -> Result<Self, FunctionStoreError> {
        let identifier = identifier.into();
        if identifier.is_empty() {
            return Err(FunctionStoreError {
                kind: FunctionStoreErrorKind::InvalidInput,
                references: Vec::new(),
            });
        }
        Ok(Self {
            identifier,
            kind,
            plugin_id: None,
            export: None,
            schema: None,
            required_capability: None,
        })
    }

    /// Bind a Plugin reference. Only valid for `Custom` and `Builtin` kinds.
    pub fn with_plugin_id(mut self, id: i64) -> Self {
        self.plugin_id = Some(id);
        self
    }

    /// Bind a Plugin export name.
    pub fn with_export(mut self, export: impl Into<String>) -> Self {
        self.export = Some(export.into());
        self
    }

    /// Bind an input/output JSON schema.
    pub fn with_schema(mut self, schema: Value) -> Self {
        self.schema = Some(schema);
        self
    }

    /// Bind a required capability identifier.
    pub fn with_required_capability(mut self, cap: impl Into<String>) -> Self {
        self.required_capability = Some(cap.into());
        self
    }

    /// Identifier.
    pub fn identifier(&self) -> &str {
        &self.identifier
    }

    /// Kind.
    pub fn kind(&self) -> FunctionKind {
        self.kind
    }

    /// Bound plugin id, if any.
    pub fn plugin_id(&self) -> Option<i64> {
        self.plugin_id
    }

    /// Bound plugin export, if any.
    pub fn export(&self) -> Option<&str> {
        self.export.as_deref()
    }

    /// Bound JSON schema, if any.
    pub fn schema(&self) -> Option<&Value> {
        self.schema.as_ref()
    }

    /// Bound required capability, if any.
    pub fn required_capability(&self) -> Option<&str> {
        self.required_capability.as_deref()
    }
}

/// Persisted Function record.
#[derive(Debug, Clone)]
pub struct FunctionRecord {
    id: i64,
    identifier: String,
    kind: FunctionKind,
    plugin_id: Option<i64>,
    export: Option<String>,
    required_capability: Option<String>,
}

impl FunctionRecord {
    /// Database id.
    pub fn id(&self) -> i64 {
        self.id
    }

    /// Identifier.
    pub fn identifier(&self) -> &str {
        &self.identifier
    }

    /// Kind.
    pub fn kind(&self) -> FunctionKind {
        self.kind
    }

    /// Bound plugin id.
    pub fn plugin_id(&self) -> Option<i64> {
        self.plugin_id
    }

    /// Bound plugin export.
    pub fn export(&self) -> Option<&str> {
        self.export.as_deref()
    }

    /// Bound required capability.
    pub fn required_capability(&self) -> Option<&str> {
        self.required_capability.as_deref()
    }
}

/// Paged result. T083 asserts page size 20 and stable index/search
/// boundaries; the full pagination matrix lives in the T087 pass.
#[derive(Debug, Clone)]
pub struct FunctionPage {
    items: Vec<FunctionRecord>,
    total: i64,
    page: i64,
    page_size: i64,
}

impl FunctionPage {
    /// Items in the current page.
    pub fn items(&self) -> &[FunctionRecord] {
        &self.items
    }

    /// Total matching rows.
    pub fn total(&self) -> i64 {
        self.total
    }

    /// Current page index (0-based).
    pub fn page(&self) -> i64 {
        self.page
    }

    /// Page size.
    pub fn page_size(&self) -> i64 {
        self.page_size
    }
}

/// Function store failure modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionStoreErrorKind {
    /// Identifier or schema violated a store-level invariant.
    InvalidInput,
    /// Dotted identifiers are forbidden; no aliasing path.
    DottedIdentifierForbidden,
    /// Tried to create a Builtin Function from user input.
    BuiltinImmutable,
    /// Identifier already exists.
    Conflict,
    /// Underlying SQL error.
    Io,
}

impl FunctionStoreErrorKind {
    /// Stable reason string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidInput => "invalid_input",
            Self::DottedIdentifierForbidden => "dotted_identifier_forbidden",
            Self::BuiltinImmutable => "builtin_immutable",
            Self::Conflict => "conflict",
            Self::Io => "io",
        }
    }
}

/// Error envelope.
#[derive(Debug, Clone, Error)]
#[error("function store error: {kind}", kind = self.kind.as_str())]
pub struct FunctionStoreError {
    kind: FunctionStoreErrorKind,
    references: Vec<String>,
}

impl FunctionStoreError {
    /// Stable reason string.
    pub fn reason(&self) -> &str {
        self.kind.as_str()
    }

    /// Conflicting identifiers (always empty in the current
    /// minimum surface; the full conflict matrix is in T087).
    pub fn references(&self) -> &[String] {
        &self.references
    }
}

/// Conflict description for a Function create. Surfaced as a
/// stable shape so the UI / T083 tests can inspect the cause.
#[derive(Debug, Clone)]
pub struct FunctionConflict {
    pub field: String,
    pub value: String,
}

/// Function store handle.
#[derive(Debug, Clone)]
pub struct FunctionStore {
    inner: Arc<FunctionStoreInner>,
}

#[derive(Debug)]
struct FunctionStoreInner {
    pool: SqlitePool,
    // Reserved identifier registrations. T087 wires this to the
    // `hive-builtins` registry; for the T083 minimum surface we
    // just keep the static reserved list.
    builtin_ids: Vec<String>,
    records: Mutex<Vec<FunctionRecord>>,
    next_id: Mutex<i64>,
}

impl FunctionStore {
    /// Open a new store against the given pool. The store is
    /// schema-aware: T087 will call into `entity_store::init_tables`
    /// to create the `functions` table on demand. The T083
    /// minimum surface uses an in-memory vector.
    pub fn new(pool: Pool<Sqlite>) -> Result<Self, FunctionStoreError> {
        // The production v4 migration owns the DDL; the T083
        // minimum surface uses an in-memory vector and assumes
        // the schema is already present.
        Ok(Self {
            inner: Arc::new(FunctionStoreInner {
                pool,
                builtin_ids: RESERVED_UNDERSCORE_IDENTIFIERS
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
                records: Mutex::new(Vec::new()),
                next_id: Mutex::new(1),
            }),
        })
    }

    /// Create a new Function record. Builtins are rejected here;
    /// the registry, not user input, owns them. Dotted identifiers
    /// are forbidden as records or aliases.
    pub fn create(&self, input: FunctionInput) -> Result<FunctionRecord, FunctionStoreError> {
        if matches!(input.kind, FunctionKind::Builtin) {
            return Err(FunctionStoreError {
                kind: FunctionStoreErrorKind::BuiltinImmutable,
                references: Vec::new(),
            });
        }
        if input.identifier.contains('.') {
            return Err(FunctionStoreError {
                kind: FunctionStoreErrorKind::DottedIdentifierForbidden,
                references: Vec::new(),
            });
        }
        let mut records = self.inner.records.lock();
        for existing in records.iter() {
            if existing.identifier == input.identifier {
                return Err(FunctionStoreError {
                    kind: FunctionStoreErrorKind::Conflict,
                    references: vec![existing.identifier.clone()],
                });
            }
        }
        let mut next_id = self.inner.next_id.lock();
        let record = FunctionRecord {
            id: *next_id,
            identifier: input.identifier.clone(),
            kind: input.kind,
            plugin_id: input.plugin_id,
            export: input.export,
            required_capability: input.required_capability,
        };
        *next_id += 1;
        records.push(record.clone());
        Ok(record)
    }

    /// Reserved identifier list, exposed for tests + UI display.
    pub fn reserved_builtin_ids(&self) -> &[String] {
        &self.inner.builtin_ids
    }
}
