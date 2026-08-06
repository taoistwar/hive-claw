//! Auto-lock configuration (T-AUTH-3, FR-050).

use std::fmt;
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutoLockMinutes(u16);

impl AutoLockMinutes {
    pub const MIN: u16 = 1;
    pub const MAX: u16 = 1440;
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

#[derive(Debug, Error)]
#[error("auto_lock_minutes={0} is out of range 1..=1440")]
pub struct InvalidAutoLockMinutes(pub u16);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidInputField {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockErrorMode {
    FailClosed,
    Disabled,
}
