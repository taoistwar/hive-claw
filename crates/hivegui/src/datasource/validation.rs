//! `validation` — public boundary for write/query DTOs, stable error
//! envelopes, conflict catalog, and enum catalog. The Foundation owns
//! the contract that every public boundary returns a
//! [`PublicBoundaryError`] wrapping a [`PublicErrorEnvelope`]
//! instead of a raw `anyhow::Error` or `sqlx::Error`.

#![warn(missing_docs)]

use std::fmt;

/// Stable error envelope returned by every public boundary. Each
/// variant carries only safe values; the UI MUST never receive raw
/// SQL error strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublicErrorEnvelope {
    /// Caller supplied an invalid value.
    InvalidInput {
        /// Field that failed validation (e.g. `page`, `search`).
        field: String,
        /// Stable reason code (e.g. `out_of_range`,
        /// `fixed_value_required`, `too_long`).
        reason: String,
    },
    /// Caller requested creation/update that collides with an
    /// existing row.
    Conflict {
        /// Conflict shape (`value` uniqueness or `references`).
        shape: String,
        /// Field that produced the conflict.
        field: String,
        /// Stable conflict reason.
        reason: String,
    },
    /// Caller asked for a record that does not exist.
    NotFound,
    /// Caller is not allowed to perform the action.
    Forbidden,
    /// The request reached an internal boundary that could not complete. The
    /// stable reason never contains backend text; the cause remains private.
    Internal {
        /// Stable, non-sensitive failure category.
        reason: String,
    },
}

/// Public error type. Wraps an envelope and an optional cause that
/// MUST NOT be propagated to the UI layer.
#[derive(Debug)]
pub struct PublicBoundaryError {
    envelope: Box<PublicErrorEnvelope>,
    value: Option<String>,
    references: Vec<String>,
    cause: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl PublicBoundaryError {
    /// Build a new public boundary error with no underlying cause.
    pub fn new(envelope: PublicErrorEnvelope) -> Self {
        Self {
            envelope: Box::new(envelope),
            value: None,
            references: Vec::new(),
            cause: None,
        }
    }

    /// Attach the safe conflicting value for a `shape = "value"` envelope.
    pub fn with_value(mut self, value: impl Into<String>) -> Self {
        self.value = Some(value.into());
        self
    }

    /// Attach safe identifiers for a `shape = "references"` envelope.
    pub fn with_references(mut self, references: impl IntoIterator<Item = String>) -> Self {
        self.references = references.into_iter().collect();
        self
    }

    /// Attach an underlying cause (e.g. a `sqlx::Error`).
    pub fn with_cause<E>(mut self, cause: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        self.cause = Some(Box::new(cause));
        self
    }

    /// Returns the envelope.
    pub fn envelope(&self) -> &PublicErrorEnvelope {
        &self.envelope
    }

    /// Return the safe conflicting value, when the envelope uses value shape.
    pub fn value(&self) -> Option<&str> {
        self.value.as_deref()
    }

    /// Return safe referencing identifiers, when the envelope uses reference
    /// shape. SQL fragments and raw row payloads are never stored here.
    pub fn references(&self) -> &[String] {
        &self.references
    }
}

impl fmt::Display for PublicBoundaryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "public boundary error: {:?}", self.envelope)
    }
}

impl std::error::Error for PublicBoundaryError {}

impl From<PublicErrorEnvelope> for PublicBoundaryError {
    fn from(envelope: PublicErrorEnvelope) -> Self {
        Self::new(envelope)
    }
}

/// Stable shape label for [`PublicConflictCatalog`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictShape {
    /// Value uniqueness conflict.
    Value,
    /// Reference / FK conflict.
    References,
}

impl ConflictShape {
    /// Returns the canonical string label.
    pub fn as_str(&self) -> &'static str {
        match self {
            ConflictShape::Value => "value",
            ConflictShape::References => "references",
        }
    }
}

/// One row of the public conflict catalog.
#[derive(Debug, Clone, Copy)]
pub struct PublicConflictRow {
    /// Entity name (e.g. `plugin`).
    pub entity: &'static str,
    /// Field involved (e.g. `identifier`).
    pub field: &'static str,
    /// Stable reason code.
    pub reason: &'static str,
    /// Conflict shape.
    pub shape: ConflictShape,
    /// Owner phase (e.g. `Foundation`).
    pub owner_phase: &'static str,
    /// Activation task.
    pub activation_task: &'static str,
}

/// One row of the public enum catalog.
#[derive(Debug, Clone, Copy)]
pub struct PublicEnumRow {
    /// Entity name.
    pub entity: &'static str,
    /// Field name.
    pub field: &'static str,
    /// Allowed values.
    pub values: &'static [&'static str],
    /// Owner phase.
    pub owner_phase: &'static str,
    /// Activation task.
    pub activation_task: &'static str,
    /// Whether the field may accept legacy aliases.
    pub allow_aliases: bool,
}

/// One row of the public write field catalog.
#[derive(Debug, Clone, Copy)]
pub struct PublicWriteFieldRow {
    /// Entity name.
    pub entity: &'static str,
    /// Field name.
    pub field: &'static str,
    /// Owner phase.
    pub owner_phase: &'static str,
    /// Activation task.
    pub activation_task: &'static str,
}

/// One row of the public query field catalog.
#[derive(Debug, Clone, Copy)]
pub struct PublicQueryFieldRow {
    /// Entity name.
    pub entity: &'static str,
    /// Field name.
    pub field: &'static str,
    /// Owner phase.
    pub owner_phase: &'static str,
    /// Activation task.
    pub activation_task: &'static str,
}

/// Public conflict catalog. Each row is owned by a single user story
/// so the Foundation can prove the gate is complete without
/// committing to downstream contracts.
pub fn public_conflict_catalog() -> &'static [PublicConflictRow] {
    CONFLICT_CATALOG
}

const CONFLICT_CATALOG: &[PublicConflictRow] = &[
    PublicConflictRow {
        entity: "global_config",
        field: "key",
        reason: "duplicate",
        shape: ConflictShape::Value,
        owner_phase: "US3",
        activation_task: "T043",
    },
    PublicConflictRow {
        entity: "tag",
        field: "name",
        reason: "duplicate",
        shape: ConflictShape::Value,
        owner_phase: "US5",
        activation_task: "T057",
    },
    PublicConflictRow {
        entity: "capability",
        field: "name",
        reason: "duplicate",
        shape: ConflictShape::Value,
        owner_phase: "US7",
        activation_task: "T068",
    },
    PublicConflictRow {
        entity: "plugin",
        field: "identifier",
        reason: "duplicate",
        shape: ConflictShape::Value,
        owner_phase: "US8",
        activation_task: "T076",
    },
    PublicConflictRow {
        entity: "function",
        field: "identifier",
        reason: "duplicate",
        shape: ConflictShape::Value,
        owner_phase: "US9",
        activation_task: "T086",
    },
    PublicConflictRow {
        entity: "workflow",
        field: "identifier",
        reason: "duplicate",
        shape: ConflictShape::Value,
        owner_phase: "US10",
        activation_task: "T094",
    },
    PublicConflictRow {
        entity: "workflow",
        field: "id",
        reason: "referenced_by_tool",
        shape: ConflictShape::References,
        owner_phase: "US10",
        activation_task: "T094",
    },
    PublicConflictRow {
        entity: "tool",
        field: "identifier",
        reason: "duplicate",
        shape: ConflictShape::Value,
        owner_phase: "US11",
        activation_task: "T104",
    },
    PublicConflictRow {
        entity: "skill",
        field: "identifier",
        reason: "duplicate",
        shape: ConflictShape::Value,
        owner_phase: "US12",
        activation_task: "T111",
    },
    PublicConflictRow {
        entity: "agent",
        field: "identifier",
        reason: "duplicate",
        shape: ConflictShape::Value,
        owner_phase: "US13",
        activation_task: "T123",
    },
    PublicConflictRow {
        entity: "category",
        field: "id",
        reason: "has_children",
        shape: ConflictShape::References,
        owner_phase: "US6",
        activation_task: "T062",
    },
    PublicConflictRow {
        entity: "llm_provider",
        field: "id",
        reason: "referenced_by_model",
        shape: ConflictShape::References,
        owner_phase: "US4",
        activation_task: "T050",
    },
    PublicConflictRow {
        entity: "plugin",
        field: "id",
        reason: "referenced_by_function",
        shape: ConflictShape::References,
        owner_phase: "US8",
        activation_task: "T076",
    },
    PublicConflictRow {
        entity: "function",
        field: "id",
        reason: "referenced_by_workflow_node",
        shape: ConflictShape::References,
        owner_phase: "US9",
        activation_task: "T086",
    },
    PublicConflictRow {
        entity: "function",
        field: "id",
        reason: "referenced_by_tool",
        shape: ConflictShape::References,
        owner_phase: "US9",
        activation_task: "T086",
    },
    PublicConflictRow {
        entity: "agent",
        field: "is_default",
        reason: "replacement_required",
        shape: ConflictShape::References,
        owner_phase: "US13",
        activation_task: "T123",
    },
    PublicConflictRow {
        entity: "llm_preset",
        field: "name",
        reason: "referenced_by_agent",
        shape: ConflictShape::References,
        owner_phase: "US4",
        activation_task: "T050",
    },
];

/// Public enum catalog. Each row lists the canonical enum values
/// accepted by the public boundary.
pub fn public_enum_catalog() -> &'static [PublicEnumRow] {
    ENUM_CATALOG
}

const ENUM_CATALOG: &[PublicEnumRow] = &[
    PublicEnumRow {
        entity: "plugin",
        field: "runtime",
        values: &["extism"],
        owner_phase: "US8",
        activation_task: "T076",
        allow_aliases: false,
    },
    PublicEnumRow {
        entity: "function",
        field: "kind",
        values: &["builtin", "custom", "placeholder"],
        owner_phase: "US9",
        activation_task: "T086",
        allow_aliases: false,
    },
    PublicEnumRow {
        entity: "function",
        field: "builtin_identifier",
        values: &[
            "format_template",
            "json_parse",
            "json_stringify",
            "text_regex_match",
        ],
        owner_phase: "US9",
        activation_task: "T086",
        allow_aliases: false,
    },
    PublicEnumRow {
        entity: "workflow_node",
        field: "node_type",
        values: &[
            "start_node",
            "end_node",
            "function_node",
            "generate_answer_node",
        ],
        owner_phase: "US10",
        activation_task: "T094",
        allow_aliases: false,
    },
    PublicEnumRow {
        entity: "tool",
        field: "kind",
        values: &["function-wrap", "workflow-wrap"],
        owner_phase: "US11",
        activation_task: "T104",
        allow_aliases: false,
    },
    PublicEnumRow {
        entity: "tool",
        field: "source",
        values: &["workspace", "builtin"],
        owner_phase: "US11",
        activation_task: "T104",
        allow_aliases: false,
    },
    PublicEnumRow {
        entity: "skill",
        field: "source",
        values: &["workspace", "builtin"],
        owner_phase: "US12",
        activation_task: "T111",
        allow_aliases: false,
    },
];

/// Public write field catalog. The Foundation rows cover
/// `data_source`; subsequent rows are added per user story.
pub fn public_write_field_catalog() -> &'static [PublicWriteFieldRow] {
    WRITE_FIELD_CATALOG
}

const WRITE_FIELD_CATALOG: &[PublicWriteFieldRow] = &[
    PublicWriteFieldRow {
        entity: "data_source",
        field: "name",
        owner_phase: "US2",
        activation_task: "T037",
    },
    PublicWriteFieldRow {
        entity: "data_source",
        field: "host",
        owner_phase: "US2",
        activation_task: "T037",
    },
    PublicWriteFieldRow {
        entity: "data_source",
        field: "port",
        owner_phase: "US2",
        activation_task: "T037",
    },
    PublicWriteFieldRow {
        entity: "data_source",
        field: "username",
        owner_phase: "US2",
        activation_task: "T037",
    },
    PublicWriteFieldRow {
        entity: "data_source",
        field: "password",
        owner_phase: "US2",
        activation_task: "T037",
    },
    PublicWriteFieldRow {
        entity: "global_config",
        field: "name",
        owner_phase: "US3",
        activation_task: "T043",
    },
    PublicWriteFieldRow {
        entity: "global_config",
        field: "key",
        owner_phase: "US3",
        activation_task: "T043",
    },
    PublicWriteFieldRow {
        entity: "global_config",
        field: "config_type",
        owner_phase: "US3",
        activation_task: "T043",
    },
    PublicWriteFieldRow {
        entity: "global_config",
        field: "data",
        owner_phase: "US3",
        activation_task: "T043",
    },
    PublicWriteFieldRow {
        entity: "llm_preset",
        field: "name",
        owner_phase: "US4",
        activation_task: "T050",
    },
    PublicWriteFieldRow {
        entity: "llm_preset",
        field: "description",
        owner_phase: "US4",
        activation_task: "T050",
    },
    PublicWriteFieldRow {
        entity: "llm_preset",
        field: "is_default",
        owner_phase: "US4",
        activation_task: "T050",
    },
    PublicWriteFieldRow {
        entity: "llm_preset",
        field: "max_tokens",
        owner_phase: "US4",
        activation_task: "T050",
    },
    PublicWriteFieldRow {
        entity: "llm_preset",
        field: "temperature",
        owner_phase: "US4",
        activation_task: "T050",
    },
    PublicWriteFieldRow {
        entity: "llm_provider",
        field: "name",
        owner_phase: "US4",
        activation_task: "T050",
    },
    PublicWriteFieldRow {
        entity: "llm_provider",
        field: "category",
        owner_phase: "US4",
        activation_task: "T050",
    },
    PublicWriteFieldRow {
        entity: "llm_provider",
        field: "base_url",
        owner_phase: "US4",
        activation_task: "T050",
    },
    PublicWriteFieldRow {
        entity: "llm_provider",
        field: "token",
        owner_phase: "US4",
        activation_task: "T050",
    },
    PublicWriteFieldRow {
        entity: "llm_provider",
        field: "token_env",
        owner_phase: "US4",
        activation_task: "T050",
    },
    PublicWriteFieldRow {
        entity: "model",
        field: "name",
        owner_phase: "US4",
        activation_task: "T050",
    },
    PublicWriteFieldRow {
        entity: "model",
        field: "preset_id",
        owner_phase: "US4",
        activation_task: "T050",
    },
    PublicWriteFieldRow {
        entity: "model",
        field: "provider_id",
        owner_phase: "US4",
        activation_task: "T050",
    },
    PublicWriteFieldRow {
        entity: "model",
        field: "priority",
        owner_phase: "US4",
        activation_task: "T050",
    },
    PublicWriteFieldRow {
        entity: "tag",
        field: "name",
        owner_phase: "US5",
        activation_task: "T057",
    },
    PublicWriteFieldRow {
        entity: "tag",
        field: "color",
        owner_phase: "US5",
        activation_task: "T057",
    },
    PublicWriteFieldRow {
        entity: "category",
        field: "parent_id",
        owner_phase: "US6",
        activation_task: "T062",
    },
    PublicWriteFieldRow {
        entity: "category",
        field: "name",
        owner_phase: "US6",
        activation_task: "T062",
    },
    PublicWriteFieldRow {
        entity: "category",
        field: "slug",
        owner_phase: "US6",
        activation_task: "T062",
    },
    PublicWriteFieldRow {
        entity: "category",
        field: "description",
        owner_phase: "US6",
        activation_task: "T062",
    },
    PublicWriteFieldRow {
        entity: "capability",
        field: "name",
        owner_phase: "US7",
        activation_task: "T068",
    },
    PublicWriteFieldRow {
        entity: "capability",
        field: "description",
        owner_phase: "US7",
        activation_task: "T068",
    },
    PublicWriteFieldRow {
        entity: "capability",
        field: "is_dangerous",
        owner_phase: "US7",
        activation_task: "T068",
    },
    PublicWriteFieldRow {
        entity: "capability",
        field: "category_id",
        owner_phase: "US7",
        activation_task: "T068",
    },
    PublicWriteFieldRow {
        entity: "plugin",
        field: "identifier",
        owner_phase: "US8",
        activation_task: "T076",
    },
    PublicWriteFieldRow {
        entity: "plugin",
        field: "name",
        owner_phase: "US8",
        activation_task: "T076",
    },
    PublicWriteFieldRow {
        entity: "plugin",
        field: "description",
        owner_phase: "US8",
        activation_task: "T076",
    },
    PublicWriteFieldRow {
        entity: "plugin",
        field: "manifest",
        owner_phase: "US8",
        activation_task: "T076",
    },
    PublicWriteFieldRow {
        entity: "plugin",
        field: "runtime",
        owner_phase: "US8",
        activation_task: "T076",
    },
    PublicWriteFieldRow {
        entity: "plugin",
        field: "version",
        owner_phase: "US8",
        activation_task: "T076",
    },
    PublicWriteFieldRow {
        entity: "plugin",
        field: "author",
        owner_phase: "US8",
        activation_task: "T076",
    },
    PublicWriteFieldRow {
        entity: "plugin",
        field: "repository_url",
        owner_phase: "US8",
        activation_task: "T076",
    },
    PublicWriteFieldRow {
        entity: "plugin",
        field: "s3_key",
        owner_phase: "US8",
        activation_task: "T076",
    },
    PublicWriteFieldRow {
        entity: "plugin",
        field: "sha256",
        owner_phase: "US8",
        activation_task: "T076",
    },
    PublicWriteFieldRow {
        entity: "plugin",
        field: "size_bytes",
        owner_phase: "US8",
        activation_task: "T076",
    },
    PublicWriteFieldRow {
        entity: "plugin",
        field: "category_id",
        owner_phase: "US8",
        activation_task: "T076",
    },
    PublicWriteFieldRow {
        entity: "plugin",
        field: "timeout_ms",
        owner_phase: "US8",
        activation_task: "T076",
    },
    PublicWriteFieldRow {
        entity: "plugin",
        field: "memory_limit_mb",
        owner_phase: "US8",
        activation_task: "T076",
    },
    PublicWriteFieldRow {
        entity: "plugin",
        field: "output_limit_bytes",
        owner_phase: "US8",
        activation_task: "T076",
    },
    PublicWriteFieldRow {
        entity: "function",
        field: "identifier",
        owner_phase: "US9",
        activation_task: "T086",
    },
    PublicWriteFieldRow {
        entity: "function",
        field: "name",
        owner_phase: "US9",
        activation_task: "T086",
    },
    PublicWriteFieldRow {
        entity: "function",
        field: "description",
        owner_phase: "US9",
        activation_task: "T086",
    },
    PublicWriteFieldRow {
        entity: "function",
        field: "kind",
        owner_phase: "US9",
        activation_task: "T086",
    },
    PublicWriteFieldRow {
        entity: "function",
        field: "input_schema",
        owner_phase: "US9",
        activation_task: "T086",
    },
    PublicWriteFieldRow {
        entity: "function",
        field: "output_schema",
        owner_phase: "US9",
        activation_task: "T086",
    },
    PublicWriteFieldRow {
        entity: "function",
        field: "plugin_id",
        owner_phase: "US9",
        activation_task: "T086",
    },
    PublicWriteFieldRow {
        entity: "function",
        field: "plugin_export",
        owner_phase: "US9",
        activation_task: "T086",
    },
    PublicWriteFieldRow {
        entity: "function",
        field: "category_id",
        owner_phase: "US9",
        activation_task: "T086",
    },
    PublicWriteFieldRow {
        entity: "function",
        field: "required_capabilities",
        owner_phase: "US9",
        activation_task: "T086",
    },
    PublicWriteFieldRow {
        entity: "workflow",
        field: "identifier",
        owner_phase: "US10",
        activation_task: "T094",
    },
    PublicWriteFieldRow {
        entity: "workflow",
        field: "name",
        owner_phase: "US10",
        activation_task: "T094",
    },
    PublicWriteFieldRow {
        entity: "workflow",
        field: "description",
        owner_phase: "US10",
        activation_task: "T094",
    },
    PublicWriteFieldRow {
        entity: "workflow",
        field: "timeout_ms",
        owner_phase: "US10",
        activation_task: "T094",
    },
    PublicWriteFieldRow {
        entity: "workflow",
        field: "category_id",
        owner_phase: "US10",
        activation_task: "T094",
    },
    PublicWriteFieldRow {
        entity: "workflow",
        field: "input_schema",
        owner_phase: "US10",
        activation_task: "T094",
    },
    PublicWriteFieldRow {
        entity: "workflow",
        field: "start_description",
        owner_phase: "US10",
        activation_task: "T094",
    },
    PublicWriteFieldRow {
        entity: "workflow",
        field: "output_schema",
        owner_phase: "US10",
        activation_task: "T094",
    },
    PublicWriteFieldRow {
        entity: "workflow",
        field: "required_capabilities",
        owner_phase: "US10",
        activation_task: "T094",
    },
    PublicWriteFieldRow {
        entity: "workflow_node",
        field: "workflow_id",
        owner_phase: "US10",
        activation_task: "T094",
    },
    PublicWriteFieldRow {
        entity: "workflow_node",
        field: "node_key",
        owner_phase: "US10",
        activation_task: "T094",
    },
    PublicWriteFieldRow {
        entity: "workflow_node",
        field: "node_type",
        owner_phase: "US10",
        activation_task: "T094",
    },
    PublicWriteFieldRow {
        entity: "workflow_node",
        field: "function_id",
        owner_phase: "US10",
        activation_task: "T094",
    },
    PublicWriteFieldRow {
        entity: "workflow_node",
        field: "position_x",
        owner_phase: "US10",
        activation_task: "T094",
    },
    PublicWriteFieldRow {
        entity: "workflow_node",
        field: "position_y",
        owner_phase: "US10",
        activation_task: "T094",
    },
    PublicWriteFieldRow {
        entity: "workflow_node",
        field: "node_config",
        owner_phase: "US10",
        activation_task: "T094",
    },
    PublicWriteFieldRow {
        entity: "workflow_edge",
        field: "workflow_id",
        owner_phase: "US10",
        activation_task: "T094",
    },
    PublicWriteFieldRow {
        entity: "workflow_edge",
        field: "src_node_key",
        owner_phase: "US10",
        activation_task: "T094",
    },
    PublicWriteFieldRow {
        entity: "workflow_edge",
        field: "dst_node_key",
        owner_phase: "US10",
        activation_task: "T094",
    },
    PublicWriteFieldRow {
        entity: "workflow_edge",
        field: "mapping",
        owner_phase: "US10",
        activation_task: "T094",
    },
    PublicWriteFieldRow {
        entity: "tool",
        field: "identifier",
        owner_phase: "US11",
        activation_task: "T104",
    },
    PublicWriteFieldRow {
        entity: "tool",
        field: "name",
        owner_phase: "US11",
        activation_task: "T104",
    },
    PublicWriteFieldRow {
        entity: "tool",
        field: "description",
        owner_phase: "US11",
        activation_task: "T104",
    },
    PublicWriteFieldRow {
        entity: "tool",
        field: "kind",
        owner_phase: "US11",
        activation_task: "T104",
    },
    PublicWriteFieldRow {
        entity: "tool",
        field: "source",
        owner_phase: "US11",
        activation_task: "T104",
    },
    PublicWriteFieldRow {
        entity: "tool",
        field: "is_always",
        owner_phase: "US11",
        activation_task: "T104",
    },
    PublicWriteFieldRow {
        entity: "tool",
        field: "function_id",
        owner_phase: "US11",
        activation_task: "T104",
    },
    PublicWriteFieldRow {
        entity: "tool",
        field: "workflow_id",
        owner_phase: "US11",
        activation_task: "T104",
    },
    PublicWriteFieldRow {
        entity: "tool",
        field: "input_schema",
        owner_phase: "US11",
        activation_task: "T104",
    },
    PublicWriteFieldRow {
        entity: "tool",
        field: "output_schema",
        owner_phase: "US11",
        activation_task: "T104",
    },
    PublicWriteFieldRow {
        entity: "tool",
        field: "category_id",
        owner_phase: "US11",
        activation_task: "T104",
    },
    PublicWriteFieldRow {
        entity: "tool",
        field: "required_capabilities",
        owner_phase: "US11",
        activation_task: "T104",
    },
    PublicWriteFieldRow {
        entity: "skill",
        field: "identifier",
        owner_phase: "US12",
        activation_task: "T111",
    },
    PublicWriteFieldRow {
        entity: "skill",
        field: "name",
        owner_phase: "US12",
        activation_task: "T111",
    },
    PublicWriteFieldRow {
        entity: "skill",
        field: "description",
        owner_phase: "US12",
        activation_task: "T111",
    },
    PublicWriteFieldRow {
        entity: "skill",
        field: "frontmatter",
        owner_phase: "US12",
        activation_task: "T111",
    },
    PublicWriteFieldRow {
        entity: "skill",
        field: "content",
        owner_phase: "US12",
        activation_task: "T111",
    },
    PublicWriteFieldRow {
        entity: "skill",
        field: "source",
        owner_phase: "US12",
        activation_task: "T111",
    },
    PublicWriteFieldRow {
        entity: "skill",
        field: "is_always",
        owner_phase: "US12",
        activation_task: "T111",
    },
    PublicWriteFieldRow {
        entity: "skill",
        field: "category_id",
        owner_phase: "US12",
        activation_task: "T111",
    },
    PublicWriteFieldRow {
        entity: "skill",
        field: "required_capabilities",
        owner_phase: "US12",
        activation_task: "T111",
    },
    PublicWriteFieldRow {
        entity: "agent",
        field: "identifier",
        owner_phase: "US13",
        activation_task: "T123",
    },
    PublicWriteFieldRow {
        entity: "agent",
        field: "name",
        owner_phase: "US13",
        activation_task: "T123",
    },
    PublicWriteFieldRow {
        entity: "agent",
        field: "description",
        owner_phase: "US13",
        activation_task: "T123",
    },
    PublicWriteFieldRow {
        entity: "agent",
        field: "system_prompt",
        owner_phase: "US13",
        activation_task: "T123",
    },
    PublicWriteFieldRow {
        entity: "agent",
        field: "parent_agent_id",
        owner_phase: "US13",
        activation_task: "T123",
    },
    PublicWriteFieldRow {
        entity: "agent",
        field: "is_default",
        owner_phase: "US13",
        activation_task: "T123",
    },
    PublicWriteFieldRow {
        entity: "agent",
        field: "model_preset",
        owner_phase: "US13",
        activation_task: "T123",
    },
    PublicWriteFieldRow {
        entity: "start_session",
        field: "user_message",
        owner_phase: "US13",
        activation_task: "T123",
    },
    PublicWriteFieldRow {
        entity: "continue_session",
        field: "session_id",
        owner_phase: "US13",
        activation_task: "T123",
    },
    PublicWriteFieldRow {
        entity: "continue_session",
        field: "user_message",
        owner_phase: "US13",
        activation_task: "T123",
    },
    PublicWriteFieldRow {
        entity: "stop_execution",
        field: "execution_id",
        owner_phase: "US13",
        activation_task: "T123",
    },
    PublicWriteFieldRow {
        entity: "delete_session",
        field: "session_id",
        owner_phase: "US13",
        activation_task: "T123",
    },
    PublicWriteFieldRow {
        entity: "clear_history",
        field: "retention_filter",
        owner_phase: "US13",
        activation_task: "T123",
    },
];

/// Public query field catalog. The Foundation rows cover
/// `paged_list`; later rows cover search/MySQL inputs.
pub fn public_query_field_catalog() -> &'static [PublicQueryFieldRow] {
    QUERY_FIELD_CATALOG
}

const QUERY_FIELD_CATALOG: &[PublicQueryFieldRow] = &[
    PublicQueryFieldRow {
        entity: "paged_list",
        field: "search",
        owner_phase: "Foundation",
        activation_task: "T017",
    },
    PublicQueryFieldRow {
        entity: "paged_list",
        field: "page",
        owner_phase: "Foundation",
        activation_task: "T017",
    },
    PublicQueryFieldRow {
        entity: "paged_list",
        field: "page_size",
        owner_phase: "Foundation",
        activation_task: "T017",
    },
];

/// Returns the canonical DTO field list for the given write entity,
/// or `None` when the entity is not yet covered by the public
/// boundary.
pub fn public_write_dto_fields(entity: &str) -> Option<&'static [&'static str]> {
    match entity {
        "data_source" => Some(&["name", "host", "port", "username", "password"]),
        "global_config" => Some(&["name", "key", "config_type", "data"]),
        "tag" => Some(&["name", "color"]),
        "category" => Some(&["parent_id", "name", "slug", "description"]),
        "capability" => Some(&["name", "description", "is_dangerous", "category_id"]),
        "function" => Some(&[
            "identifier",
            "name",
            "description",
            "kind",
            "input_schema",
            "output_schema",
            "plugin_id",
            "plugin_export",
            "category_id",
            "required_capabilities",
        ]),
        _ => None,
    }
}

/// Returns the canonical DTO field list for the given query entity,
/// or `None` when the entity is not yet covered by the public
/// boundary.
pub fn public_query_dto_fields(entity: &str) -> Option<&'static [&'static str]> {
    match entity {
        "paged_list" => Some(&["search", "page", "page_size"]),
        "category_search" => Some(&["search"]),
        "mysql_table_query" => Some(&[
            "database", "table", "filters", "order_by", "limit", "offset",
        ]),
        _ => None,
    }
}
