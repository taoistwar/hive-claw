//! Auto-lock configuration (T-AUTH-3, FR-050).

use std::fmt;
use thiserror::Error;

/// Auto-lock idle timeout, expressed in minutes and validated to stay
/// within the policy bounds `[MIN, MAX]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutoLockMinutes(u16);

impl AutoLockMinutes {
    /// Smallest legal auto-lock value (1 minute).
    pub const MIN: u16 = 1;
    /// Largest legal auto-lock value (24 hours expressed in minutes).
    pub const MAX: u16 = 1440;
    /// Default auto-lock value used when none is configured (15 minutes).
    pub const DEFAULT: u16 = 15;

    /// Construct a value in the legal range `[MIN, MAX]`. Returns
    /// `InvalidAutoLockMinutes` (carrying the offending value) when
    /// the supplied `value` is outside the policy bounds.
    pub fn new(value: u16) -> Result<Self, InvalidAutoLockMinutes> {
        if !(Self::MIN..=Self::MAX).contains(&value) {
            return Err(InvalidAutoLockMinutes(value));
        }
        Ok(Self(value))
    }

    /// Returns the wrapped minute count.
    pub fn minutes(self) -> u16 {
        self.0
    }
}

impl Default for AutoLockMinutes {
    fn default() -> Self {
        Self(Self::DEFAULT)
    }
}

impl From<AutoLockMinutes> for u16 {
    fn from(value: AutoLockMinutes) -> Self {
        value.0
    }
}

impl fmt::Display for AutoLockMinutes {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}min", self.0)
    }
}

/// Error returned when an `AutoLockMinutes` value falls outside the
/// legal `[MIN, MAX]` range; carries the offending raw value.
#[derive(Debug, Error)]
#[error("auto_lock_minutes={0} is out of range 1..=1440")]
pub struct InvalidAutoLockMinutes(pub u16);

/// Identifies a specific user-facing input field that failed validation,
/// so callers can surface the error against the right UI control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidInputField {
    /// The `auto_lock_minutes` configuration field.
    AutoLockMinutes,
}

impl InvalidInputField {
    /// Returns the canonical string identifier for this field
    /// (e.g. `"auth.auto_lock_minutes"`). Used in `AuthError::InvalidInput`
    /// payloads so callers can map the error back to a UI field.
    pub fn as_str(self) -> &'static str {
        match self {
            InvalidInputField::AutoLockMinutes => "auth.auto_lock_minutes",
        }
    }
}

/// Behaviour to apply when a lock operation fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockErrorMode {
    /// Refuse to proceed (fail closed) so a keystore cannot be left
    /// accidentally unsealed.
    FailClosed,
    /// Treat the lock failure as non-blocking (disabled enforcement).
    Disabled,
}
