//! Recovery risk-notice, recovery entry kinds, acknowledgement types
//! (T-AUTH-4, FR-051 / SC-035).
//!
//! `RecoveryEntryKind` is the **public** enumeration of every recovery
//! entry surface the application offers. Per FR-051 and the
//! Constitution Code-Quality gate, the only legal surface is
//! `RestoreFromBackup`. The remaining variants are dead code that
//! exists solely so the `auth_no_reset_red.rs` compile-time negative
//! test (`recovery_entry_kinds_are_only_restore_from_backup`) can
//! reference them and assert they never appear in `all()`. The grep
//! test in `auth_no_reset_red.rs::assert_no_reset_or_recovery_api_symbols`
//! separately enforces that no `pub fn` password-bypass symbol is
//! exposed.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryEntryKind {
    /// The only legal recovery entry: restore from a T129 backup.
    RestoreFromBackup,
    /// Phantom — must never be returned by `all()`. Exists so the
    /// negative compile-time test can reference it.
    #[doc(hidden)]
    ResetPassword,
    /// Phantom — must never be returned by `all()`.
    #[doc(hidden)]
    RecoverFromQuestions,
    /// Phantom — must never be returned by `all()`.
    #[doc(hidden)]
    RecoveryKey,
}

impl RecoveryEntryKind {
    /// The exhaustive list of recovery entries the application exposes
    /// to the user. Must contain exactly one element: `RestoreFromBackup`.
    pub fn all() -> Vec<Self> {
        vec![Self::RestoreFromBackup]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryAcceptance {
    Checked,
    Unchecked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcknowledgeOutcome {
    Accepted,
    BackupRequired {
        notice: super::backup::BackupRequiredNotice,
    },
    AcknowledgementRequired,
}

#[derive(Debug, Clone)]
pub struct RecoveryRiskNotice {
    pub title: String,
    pub requires_explicit_acknowledgement: bool,
}

impl RecoveryRiskNotice {
    /// Returns the human-readable title for the recovery risk notice.
    pub fn title(&self) -> &str {
        &self.title
    }
    /// Returns `true` iff the user must explicitly check an
    /// acknowledgement box before the recovery flow can proceed. Always
    /// `true` for the current risk notice (FR-051).
    pub fn requires_explicit_acknowledgement(&self) -> bool {
        self.requires_explicit_acknowledgement
    }
}

#[derive(Debug, Clone)]
pub enum RecoveryConfirmView {
    RiskNotice,
}
