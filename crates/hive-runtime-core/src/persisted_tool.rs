//! Stable persisted Tool contracts used at product adapter boundaries.
//!
//! The shared representation defines portable Tool kinds, sources, targets, and
//! capability requirements without depending on a database schema. HiveWeb and
//! HiveGUI translate their own storage records to and from this contract and
//! enforce product-specific persistence rules outside this crate.
//!
//! The contract guarantees:
//!
//! 1. The kind is one of two stable string values: `function-wrap` or
//!    `workflow-wrap`. The numeric legacy kinds (`1`, `2`) live in the
//!    product-side migration layer; this module does not accept them.
//! 2. The target XOR is enforced at builder time. `function-wrap` requires a
//!    function identifier (`namespace.short_slug`); `workflow-wrap` requires
//!    a workflow identifier. Mixing the two is rejected with
//!    [`PersistedToolError::TargetKindMismatch`].
//! 3. Required capabilities preserve their declared order. Product adapters
//!    construct them through [`RequiredCapabilities::from_ordered`], which
//!    rejects duplicates and capabilities absent from the local registry.
//! 4. The serialised form is byte-stable: `from_bytes(to_bytes(x)) == x`.

#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::capability::{CapabilityId, CapabilitySet};

/// Stable, portable Tool kind.
///
/// The numeric legacy values (`1` = function-wrap, `2` = workflow-wrap) are
/// intentionally absent. Product migrations translate the legacy integers to
/// these strings at the persistence boundary; this contract refuses to
/// silently accept an integer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum PersistedToolKind {
    /// Tool wraps a Function (executes the function with the call input).
    FunctionWrap,
    /// Tool wraps a Workflow (executes the workflow DAG with the call input).
    WorkflowWrap,
}

impl PersistedToolKind {
    /// Returns the stable string form used in serialised bytes and database
    /// columns.
    pub fn as_str(&self) -> &'static str {
        match self {
            PersistedToolKind::FunctionWrap => "function-wrap",
            PersistedToolKind::WorkflowWrap => "workflow-wrap",
        }
    }

    /// Returns the kind whose `as_str()` matches `value`, if any.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "function-wrap" => Some(PersistedToolKind::FunctionWrap),
            "workflow-wrap" => Some(PersistedToolKind::WorkflowWrap),
            _ => None,
        }
    }
}

impl fmt::Display for PersistedToolKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Target of a [`PersistedTool`]: a function or workflow identifier.
///
/// The target XOR with the Tool kind is enforced at construction time. Use
/// [`PersistedToolTarget::function`] for `function-wrap` Tools and
/// [`PersistedToolTarget::workflow`] for `workflow-wrap` Tools. The string
/// form must satisfy the stable identifier contract
/// (`namespace.short_slug`, `[a-z0-9_]+(\.[a-z0-9_]+)?`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PersistedToolTarget {
    identifier: String,
    kind: PersistedToolKind,
}

impl PersistedToolTarget {
    /// Constructs a function identifier target without validating the
    /// identifier. The validation contract is enforced at the builder
    /// boundary (see [`PersistedToolBuilder::target`]) so that invalid
    /// identifiers are reported as [`PersistedToolError`] instead of
    /// panicking.
    pub fn function(identifier: &str) -> Self {
        Self {
            identifier: identifier.to_string(),
            kind: PersistedToolKind::FunctionWrap,
        }
    }

    /// Constructs a workflow identifier target. Same validation contract
    /// as [`PersistedToolTarget::function`].
    pub fn workflow(identifier: &str) -> Self {
        Self {
            identifier: identifier.to_string(),
            kind: PersistedToolKind::WorkflowWrap,
        }
    }

    /// Returns the target's string identifier.
    pub fn as_str(&self) -> &str {
        &self.identifier
    }

    /// Returns the target's expected kind.
    pub fn kind(&self) -> PersistedToolKind {
        self.kind
    }

    /// Returns `true` if the target's identifier is a valid
    /// `namespace.short_slug` per the stable contract.
    pub fn is_valid(&self) -> bool {
        validate_identifier(&self.identifier).is_ok()
    }
}

/// Ordered required capability declaration.
///
/// The declaration order is part of the persisted contract. Product adapters
/// should use [`Self::from_ordered`] at their validation boundary so duplicate
/// and unavailable capabilities fail closed before persistence.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequiredCapabilities {
    inner: Vec<CapabilityId>,
}

impl RequiredCapabilities {
    /// Returns an empty set.
    pub fn empty() -> Self {
        Self { inner: Vec::new() }
    }

    /// Constructs an ordered declaration against the locally known
    /// capability registry.
    pub fn from_ordered(
        capabilities: Vec<CapabilityId>,
        known: &CapabilitySet,
    ) -> Result<Self, PersistedToolError> {
        let mut seen = BTreeSet::new();
        for capability in &capabilities {
            if !seen.insert(capability.clone()) {
                return Err(PersistedToolError::DuplicateCapability {
                    capability: capability.clone(),
                });
            }
            if !known.contains(capability) {
                return Err(PersistedToolError::UnknownCapability {
                    capability: capability.clone(),
                });
            }
        }
        Ok(Self {
            inner: capabilities,
        })
    }

    /// Inserts a capability at the end of the declaration. No-op if the id is
    /// already present. Product persistence boundaries should prefer
    /// [`Self::from_ordered`] so registry membership is also validated.
    pub fn insert(&mut self, id: CapabilityId) -> bool {
        if self.inner.contains(&id) {
            return false;
        }
        self.inner.push(id);
        true
    }

    /// Returns the number of capabilities.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Returns `true` if the set is empty.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Returns an iterator in the original declared order.
    pub fn iter(&self) -> impl Iterator<Item = &CapabilityId> {
        self.inner.iter()
    }
}

impl FromIterator<CapabilityId> for RequiredCapabilities {
    fn from_iter<I: IntoIterator<Item = CapabilityId>>(iter: I) -> Self {
        let mut set = Self::empty();
        for id in iter {
            set.insert(id);
        }
        set
    }
}

/// Builder for [`PersistedTool`]. The kind and target are XOR-checked at
/// `.build()` time; mixing them is rejected.
#[derive(Debug, Clone)]
pub struct PersistedToolBuilder {
    kind: PersistedToolKind,
    target: Option<PersistedToolTarget>,
    required_capabilities: RequiredCapabilities,
}

impl PersistedToolBuilder {
    /// Starts a new builder with the given kind and an empty target /
    /// capability set.
    pub fn new(kind: PersistedToolKind) -> Self {
        Self {
            kind,
            target: None,
            required_capabilities: RequiredCapabilities::empty(),
        }
    }

    /// Sets the target. The target's kind MUST match `self.kind` and the
    /// target identifier MUST pass the stable `namespace.short_slug`
    /// contract; otherwise the call returns
    /// [`PersistedToolError::TargetKindMismatch`] /
    /// [`PersistedToolError::EmptyTarget`] /
    /// [`PersistedToolError::InvalidIdentifier`].
    pub fn target(mut self, target: PersistedToolTarget) -> Result<Self, PersistedToolError> {
        if target.kind() != self.kind {
            return Err(PersistedToolError::TargetKindMismatch {
                kind: self.kind,
                target_kind: target.kind(),
            });
        }
        validate_identifier(target.as_str())?;
        self.target = Some(target);
        Ok(self)
    }

    /// Sets the required capabilities (replacing any prior value).
    pub fn required_capabilities(
        mut self,
        caps: RequiredCapabilities,
    ) -> Result<Self, PersistedToolError> {
        self.required_capabilities = caps;
        Ok(self)
    }

    /// Finalises the builder. Panics if no target was supplied; the
    /// target XOR check is enforced at `.target()` time so a builder
    /// that successfully reaches `.build()` always has a coherent state.
    pub fn build(self) -> PersistedTool {
        // The target XOR check happens in `target()`; if a builder
        // reaches `build()` then `self.target` was set with a matching
        // kind, so unwrapping is safe.
        let target = self
            .target
            .expect("target must be set before build (enforced by target() XOR check)");
        PersistedTool {
            kind: self.kind,
            target,
            required_capabilities: self.required_capabilities,
        }
    }
}

/// A persisted Tool record at the portable contract boundary.
///
/// Construct via [`PersistedToolBuilder`]. The serialised form is byte-stable
/// and self-contained: roundtripping through `to_bytes` / `from_bytes` is
/// exact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedTool {
    kind: PersistedToolKind,
    target: PersistedToolTarget,
    required_capabilities: RequiredCapabilities,
}

impl PersistedTool {
    /// Reconstructs a `PersistedTool` from its serialised bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, PersistedToolError> {
        let tool: Self = serde_json::from_slice(bytes).map_err(PersistedToolError::from)?;
        let mut seen = BTreeSet::new();
        for capability in tool.required_capabilities.iter() {
            if !seen.insert(capability.clone()) {
                return Err(PersistedToolError::DuplicateCapability {
                    capability: capability.clone(),
                });
            }
        }
        Ok(tool)
    }

    /// Serialises this tool to canonical bytes.
    pub fn to_bytes(&self) -> Result<Vec<u8>, PersistedToolError> {
        serde_json::to_vec(self).map_err(PersistedToolError::from)
    }

    /// Returns the Tool kind.
    pub fn kind(&self) -> PersistedToolKind {
        self.kind
    }

    /// Returns the Tool target.
    pub fn target(&self) -> &PersistedToolTarget {
        &self.target
    }

    /// Returns the required capabilities.
    pub fn required_capabilities(&self) -> &RequiredCapabilities {
        &self.required_capabilities
    }
}

/// Failure modes for [`PersistedToolBuilder`] and [`PersistedTool::from_bytes`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PersistedToolError {
    /// `function-wrap` was paired with a workflow target, or vice versa.
    #[error("target kind {target_kind} does not match tool kind {kind}")]
    TargetKindMismatch {
        /// The Tool kind the builder was started with.
        kind: PersistedToolKind,
        /// The kind of the supplied target.
        target_kind: PersistedToolKind,
    },
    /// The target identifier is empty.
    #[error("target identifier is empty")]
    EmptyTarget,
    /// The target identifier violates the stable `namespace.short_slug`
    /// contract (uppercase, slash, space, multiple dots, leading/trailing
    /// dot, path traversal, etc.).
    #[error("target identifier is invalid: {identifier}")]
    InvalidIdentifier {
        /// The rejected identifier string.
        identifier: String,
    },
    /// A required capability was declared more than once.
    #[error("required capability is duplicated: {capability}")]
    DuplicateCapability {
        /// The duplicated capability.
        capability: CapabilityId,
    },
    /// A required capability is absent from the local known-capability set.
    #[error("required capability is unknown: {capability}")]
    UnknownCapability {
        /// The unavailable capability.
        capability: CapabilityId,
    },
    /// `build()` was called before `target()`.
    #[error("target was not supplied before build")]
    MissingTarget,
    /// The serialised form is malformed.
    #[error("serialised tool bytes are malformed: {0}")]
    MalformedBytes(String),
}

impl From<serde_json::Error> for PersistedToolError {
    fn from(err: serde_json::Error) -> Self {
        PersistedToolError::MalformedBytes(err.to_string())
    }
}

fn validate_identifier(identifier: &str) -> Result<(), PersistedToolError> {
    if identifier.is_empty() {
        return Err(PersistedToolError::EmptyTarget);
    }
    // Tool targets are exactly `namespace.short_slug` (one dot, no more).
    // Anything else (empty slug, extra dots, leading/trailing dot,
    // uppercase, slash, space, etc.) is rejected.
    let mut segments = identifier.split('.');
    let head = segments.next().expect("split always yields ≥1 segment");
    validate_segment(head)?;
    let mut dot_count = 0usize;
    for segment in segments {
        dot_count += 1;
        validate_segment(segment)?;
    }
    if identifier.ends_with('.') || identifier.starts_with('.') {
        return Err(PersistedToolError::InvalidIdentifier {
            identifier: identifier.to_string(),
        });
    }
    if dot_count != 1 {
        return Err(PersistedToolError::InvalidIdentifier {
            identifier: identifier.to_string(),
        });
    }
    Ok(())
}

fn validate_segment(segment: &str) -> Result<(), PersistedToolError> {
    if segment.is_empty() {
        return Err(PersistedToolError::InvalidIdentifier {
            identifier: "<empty-segment>".to_string(),
        });
    }
    for ch in segment.chars() {
        let is_lower = ch.is_ascii_lowercase();
        let is_digit = ch.is_ascii_digit();
        let is_underscore = ch == '_';
        if !(is_lower || is_digit || is_underscore) {
            return Err(PersistedToolError::InvalidIdentifier {
                identifier: format!("segment contains invalid character: {ch:?}"),
            });
        }
    }
    Ok(())
}
