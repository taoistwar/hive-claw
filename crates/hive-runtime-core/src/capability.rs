//! Capability declarations, assignments, and local handler registration.
//!
//! Persisted capability metadata is not evidence that a runtime handler exists.
//! Product adapters register their actual local handlers, and authorization is
//! evaluated from an execution-time permission snapshot before dispatch. This
//! module contains no database model and no network implementation.
//!
//! The contract is split into two layers:
//!
//! 1. A *declaration* side: [`CapabilityId`] and [`CapabilitySet`] are portable
//!    value types that travel with the execution context. They can be
//!    constructed freely by any product adapter — the mere presence of a
//!    [`CapabilityId`] in metadata or in a permission set MUST NOT imply the
//!    existence of a real handler.
//! 2. A *registration* side: [`HandlerRegistry`] owns a map of
//!    `CapabilityId -> Handler`. Registration is explicit and duplicate
//!    registrations are rejected with [`DispatchError::AlreadyRegistered`].
//!
//! Dispatch is the only function that bridges the two layers. A successful
//! dispatch is represented by [`DispatchOutcome::Handled`]; every failure mode
//! is enumerated in [`DispatchError`] so product code can pattern-match
//! without string inspection.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// Stable identifier for a Capability.
///
/// The string form is the only payload; this type does not add storage or
/// network concerns. The [`CapabilityId::new`] constructor validates that the
/// identifier matches the contract (`[a-z0-9_]+(\.[a-z0-9_]+)?`); arbitrary
/// strings are rejected with [`CapabilityIdError`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CapabilityId(String);

impl CapabilityId {
    /// Validates and constructs a new [`CapabilityId`].
    pub fn new(value: &str) -> Result<Self, CapabilityIdError> {
        if value.is_empty() {
            return Err(CapabilityIdError::Empty);
        }
        // No leading or trailing dot.
        if value.starts_with('.') || value.ends_with('.') {
            return Err(CapabilityIdError::Malformed);
        }
        // Split into segments and validate each.
        for segment in value.split('.') {
            validate_segment(segment)?;
        }
        Ok(Self(value.to_string()))
    }

    /// Returns the underlying string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CapabilityId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl AsRef<str> for CapabilityId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// Failure mode for [`CapabilityId::new`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CapabilityIdError {
    /// Empty input.
    #[error("capability id is empty")]
    Empty,
    /// A segment contains a character outside `[a-z0-9_]`.
    #[error("capability id segment contains invalid characters")]
    InvalidCharacter,
    /// Identifier is structurally malformed (multiple dots, leading/trailing dot).
    #[error("capability id is malformed (dot placement)")]
    Malformed,
    /// Empty segment between dots.
    #[error("capability id has an empty segment")]
    EmptySegment,
}

fn validate_segment(segment: &str) -> Result<(), CapabilityIdError> {
    if segment.is_empty() {
        return Err(CapabilityIdError::EmptySegment);
    }
    for ch in segment.chars() {
        let is_lower = ch.is_ascii_lowercase();
        let is_digit = ch.is_ascii_digit();
        let is_underscore = ch == '_';
        if !(is_lower || is_digit || is_underscore) {
            return Err(CapabilityIdError::InvalidCharacter);
        }
    }
    Ok(())
}

/// Ordered, de-duplicated set of [`CapabilityId`] values.
///
/// Used for the permission snapshot at dispatch time. Internally backed by a
/// `BTreeSet` so iteration is canonical and roundtrips are byte-stable.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilitySet {
    inner: std::collections::BTreeSet<CapabilityId>,
}

impl CapabilitySet {
    /// Returns an empty set.
    pub fn empty() -> Self {
        Self {
            inner: std::collections::BTreeSet::new(),
        }
    }

    /// Returns `true` if `id` is in the set.
    pub fn contains(&self, id: &CapabilityId) -> bool {
        self.inner.contains(id)
    }

    /// Inserts a capability id. No-op if the id is already present.
    pub fn insert(&mut self, id: CapabilityId) -> bool {
        self.inner.insert(id)
    }

    /// Returns the number of capabilities in the set.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Returns `true` if the set is empty.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Iterates the set in canonical order.
    pub fn iter(&self) -> impl Iterator<Item = &CapabilityId> {
        self.inner.iter()
    }
}

impl FromIterator<CapabilityId> for CapabilitySet {
    fn from_iter<I: IntoIterator<Item = CapabilityId>>(iter: I) -> Self {
        let mut set = Self::empty();
        for id in iter {
            set.insert(id);
        }
        set
    }
}

/// Outcome of a successful dispatch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DispatchOutcome {
    /// A registered handler accepted the input and produced this output.
    Handled {
        /// The CapabilityId whose handler ran.
        capability: CapabilityId,
        /// The JSON value the handler returned.
        output: serde_json::Value,
    },
}

/// Failure mode for [`HandlerRegistry::dispatch`] and friends.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DispatchError {
    /// No handler is registered for the requested capability.
    #[error("no handler registered for capability {capability}")]
    HandlerNotRegistered {
        /// The capability that was requested.
        capability: CapabilityId,
    },
    /// A handler was already registered for this capability.
    #[error("capability {capability} is already registered")]
    AlreadyRegistered {
        /// The capability that was requested.
        capability: CapabilityId,
    },
    /// The dispatch caller did not present a matching [`CapabilitySet`].
    #[error("dispatcher is not authorised for capability {capability}")]
    Unauthorised {
        /// The capability that was requested.
        capability: CapabilityId,
    },
    /// The handler itself returned an error.
    #[error("handler for capability {capability} returned an error: {message}")]
    HandlerFailed {
        /// The capability whose handler failed.
        capability: CapabilityId,
        /// Human-readable error message.
        message: String,
    },
}

/// Handler function signature.
pub type HandlerFn =
    Arc<dyn Fn(serde_json::Value) -> Result<serde_json::Value, String> + Send + Sync>;

/// Map of `CapabilityId -> Handler`.
///
/// Handlers are wrapped in `Arc<dyn Fn>` so the registry is cheap to clone and
/// can be shared across threads. The registry itself is not thread-safe for
/// mutation; production adapters should wrap it in a `Mutex`/`RwLock` and
/// populate it during process start.
#[derive(Clone, Default)]
pub struct HandlerRegistry {
    handlers: BTreeMap<CapabilityId, HandlerFn>,
}

impl HandlerRegistry {
    /// Returns an empty registry.
    pub fn empty() -> Self {
        Self {
            handlers: BTreeMap::new(),
        }
    }

    /// Returns `true` if a handler is registered for `capability`.
    pub fn has_handler(&self, capability: &CapabilityId) -> bool {
        self.handlers.contains_key(capability)
    }

    /// Registers a new handler for `capability`.
    ///
    /// Returns [`DispatchError::AlreadyRegistered`] if a handler is already
    /// present. The first registration wins; later attempts are rejected so
    /// the dispatch table is unambiguous.
    pub fn register<F>(
        &mut self,
        capability: &CapabilityId,
        handler: F,
    ) -> Result<(), DispatchError>
    where
        F: Fn(serde_json::Value) -> Result<serde_json::Value, String> + Send + Sync + 'static,
    {
        if self.handlers.contains_key(capability) {
            return Err(DispatchError::AlreadyRegistered {
                capability: capability.clone(),
            });
        }
        self.handlers.insert(capability.clone(), Arc::new(handler));
        Ok(())
    }

    /// Dispatches `input` to the handler for `capability` without any
    /// permission check.
    pub fn dispatch(
        &self,
        capability: &CapabilityId,
        input: &serde_json::Value,
    ) -> Result<DispatchOutcome, DispatchError> {
        let handler =
            self.handlers
                .get(capability)
                .ok_or_else(|| DispatchError::HandlerNotRegistered {
                    capability: capability.clone(),
                })?;
        let output = handler(input.clone()).map_err(|message| DispatchError::HandlerFailed {
            capability: capability.clone(),
            message,
        })?;
        Ok(DispatchOutcome::Handled {
            capability: capability.clone(),
            output,
        })
    }

    /// Dispatches only if `permissions` already contains `capability`.
    pub fn dispatch_authorised(
        &self,
        capability: &CapabilityId,
        permissions: &CapabilitySet,
        input: &serde_json::Value,
    ) -> Result<DispatchOutcome, DispatchError> {
        if !permissions.contains(capability) {
            return Err(DispatchError::Unauthorised {
                capability: capability.clone(),
            });
        }
        self.dispatch(capability, input)
    }
}

impl fmt::Debug for HandlerRegistry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HandlerRegistry")
            .field("capabilities", &self.handlers.keys().collect::<Vec<_>>())
            .finish()
    }
}
