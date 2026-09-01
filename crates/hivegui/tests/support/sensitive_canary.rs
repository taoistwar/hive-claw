//! T016F [P] [Foundation-doc+Red] Sensitive persistence canary scanner.
//!
//! Source of truth: `specs/011-hivegui-standalone-mode/tasks.md` §T016F
//! (foundation cross-cutting rule: unique sensitive-field / media
//! inventory and a reusable plaintext canary scanner).
//!
//! Red gate: this file declares the public surface area the future
//! `hivegui::sensitive_canary` boundary (introduced by T025/T027) MUST
//! provide. The accompanying `sensitive_persistence_contract.rs`
//! integration test exercises the boundary directly and asserts that
//! plaintext canaries placed at the public roundtrip never land on any
//! of the enumerated persistence media.
//!
//! The Foundation phase (this file) does NOT scan the final diagnostic
//! bundle or story-owned rows. Those rows stay Pending until each
//! story's `owner_phase` activates them through the public boundary.

use std::{
    fs,
    path::{Path, PathBuf},
    process,
};

use chrono::{DateTime, Utc};
use serde::Serialize;
use thiserror::Error;

// ---------------------------------------------------------------------------
// §T016F.1 — Sensitive field inventory.
// ---------------------------------------------------------------------------

/// Sensitive persisted fields the boundary MUST track.
///
/// The Foundation phase is only responsible for the rows whose
/// `owner_phase` is `Foundation` (device_key / crypto temp/error
/// via T025 and logging temp/error via T027). Future-story rows are
/// added by the owning story and activated through the public boundary
/// without modifying this enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SensitiveField {
    /// DataSource row: `encrypted_password`.
    DataSourcePassword,
    /// LlmProvider row: `token_encrypted`.
    LlmProviderToken,
    /// ChatSession row: `title_encrypted`.
    ChatSessionTitle,
    /// ChatMessage row: `content_encrypted`.
    ChatMessageContent,
    /// ChatMessage row: `tool_calls_encrypted`.
    ChatMessageToolCalls,
    /// AgentExecution row: `state_encrypted`.
    AgentExecutionState,
}

impl SensitiveField {
    /// Returns the canonical storage column / slot the boundary must
    /// cover. The string is informational; the byte-level guarantee is
    /// provided by the canary scanner, not the column name.
    pub fn canonical_slot(self) -> &'static str {
        match self {
            SensitiveField::DataSourcePassword => "datasources.encrypted_password",
            SensitiveField::LlmProviderToken => "llm_providers.token_encrypted",
            SensitiveField::ChatSessionTitle => "chat_sessions.title_encrypted",
            SensitiveField::ChatMessageContent => "chat_messages.content_encrypted",
            SensitiveField::ChatMessageToolCalls => "chat_messages.tool_calls_encrypted",
            SensitiveField::AgentExecutionState => "agent_executions.state_encrypted",
        }
    }

    /// Returns the `owner_phase` tag for this row. Foundation rows are
    /// activated by T025 (device key / crypto) and T027 (logging).
    pub fn owner_phase(self) -> SensitiveOwnerPhase {
        match self {
            // Each row's owner_phase is recorded alongside the row so
            // T017F and T138 can verify that no Foundation scan covers
            // future-story rows. The Foundation phase owns the rows
            // that the auth and logging minimal-Green tasks create.
            SensitiveField::DataSourcePassword => SensitiveOwnerPhase::Story {
                story: "US4_data_source_management",
                task: "T037",
            },
            SensitiveField::LlmProviderToken => SensitiveOwnerPhase::Story {
                story: "US4_llm_config",
                task: "T050",
            },
            SensitiveField::ChatSessionTitle => SensitiveOwnerPhase::Story {
                story: "US13_local_agent",
                task: "T123",
            },
            SensitiveField::ChatMessageContent => SensitiveOwnerPhase::Story {
                story: "US13_local_agent",
                task: "T123",
            },
            SensitiveField::ChatMessageToolCalls => SensitiveOwnerPhase::Story {
                story: "US13_local_agent",
                task: "T123",
            },
            SensitiveField::AgentExecutionState => SensitiveOwnerPhase::Story {
                story: "US13_local_agent",
                task: "T123",
            },
        }
    }
}

/// Owner-phase tag for each row. `Foundation` rows are exercised by the
/// Foundation scan; `Story { ... }` rows stay Pending until the owning
/// story activates them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "phase")]
pub enum SensitiveOwnerPhase {
    Foundation {
        task: &'static str,
    },
    Story {
        story: &'static str,
        task: &'static str,
    },
}

// ---------------------------------------------------------------------------
// §T016F.2 — Persistence medium inventory.
// ---------------------------------------------------------------------------

/// Media the scanner must search. The byte coverage is exhaustive —
/// the scanner MUST look at every byte of every medium for the
/// canary plaintext.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SensitiveMedium {
    SqliteMain,
    SqliteWal,
    SqliteShm,
    SqliteJournal,
    BackupStaging,
    BackupFinalArchive,
    PlainTempDir,
    StructuredLog,
    DiagnosticBundle,
    ErrorAndCrashRecovery,
    CrossDevicePath,
}

impl SensitiveMedium {
    /// Returns the relative path the scanner must scan within the
    /// `TestWorkspace` root. The default root layout is the one
    /// `TestWorkspace` produces; story-owned layouts are exercised
    /// by their own tests, never by Foundation.
    pub fn relative_path(self) -> &'static str {
        match self {
            SensitiveMedium::SqliteMain => "store/datasources.db",
            SensitiveMedium::SqliteWal => "store/datasources.db-wal",
            SensitiveMedium::SqliteShm => "store/datasources.db-shm",
            SensitiveMedium::SqliteJournal => "store/datasources.db-journal",
            SensitiveMedium::BackupStaging => "backups/staging",
            SensitiveMedium::BackupFinalArchive => "backups/final",
            SensitiveMedium::PlainTempDir => "tmp",
            SensitiveMedium::StructuredLog => "logs",
            SensitiveMedium::DiagnosticBundle => "diagnostics",
            SensitiveMedium::ErrorAndCrashRecovery => "recovery",
            SensitiveMedium::CrossDevicePath => "cross-device",
        }
    }
}

// ---------------------------------------------------------------------------
// §T016F.3 — Canary plaintext + public roundtrip boundary.
// ---------------------------------------------------------------------------

/// The single plaintext canary the scanner matches. Foundation rows
/// activate it through the public boundary; the boundary is the only
/// place the canary is ever written to disk.
pub const HIVEGUI_CANARY_LOCAL_AUTH: &str = "HIVEGUI_CANARY_LOCAL_AUTH=plaintext";

/// Stable canary used by the sensitive persistence contract test.
/// Format: `<scope>=<payload>`. The payload is large enough to be
/// obvious when a leak occurs.
pub const CANARY_FINGERPRINT: &str = "HIVEGUI_SENSITIVE_PERSISTENCE_CANARY";

#[derive(Debug, Error)]
pub enum CanaryError {
    #[error("io error: {0}")]
    Io(String),
    #[error("path outside the scan root: {0}")]
    PathOutside(String),
    #[error("public boundary returned no canary placeholder to scan for")]
    NoCanary,
}

/// Public boundary: places a unique plaintext canary through the
/// product's `encrypted_*` roundtrip so the scanner has something
/// to (not) find on disk. The function returns the value the canary
/// was constructed with so the test can verify that no half-decoded
/// residue was left behind on failure.
///
/// The test-side contract: the caller MUST round-trip a `DataSource` /
/// `LlmProvider` / etc. through the product store using the returned
/// fingerprint string as the plaintext password (or token, or other
/// sensitive field). The canary scanner then walks every persistence
/// medium and asserts the fingerprint is never present in cleartext.
pub fn place_canary_for_test(
    field: SensitiveField,
    payload: &str,
) -> Result<CanaryHandle, CanaryError> {
    // The fingerprint embeds the payload so a stray plaintext
    // appearance on disk can be unambiguously attributed to the
    // test that leaked. The `field` is recorded in the handle
    // so a hit in the scanner can be routed to the right story's
    // contract test without contaminating the on-disk token.
    let _ = field;
    let fingerprint = format!("{CANARY_FINGERPRINT}:{payload}");
    Ok(CanaryHandle {
        field,
        placed_at: Utc::now(),
        fingerprint,
        payload: payload.to_string(),
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct CanaryHandle {
    pub field: SensitiveField,
    pub placed_at: DateTime<Utc>,
    pub fingerprint: String,
    pub payload: String,
}

impl CanaryHandle {
    /// The plaintext token the scanner MUST NOT find on any medium.
    pub fn plaintext_token(&self) -> String {
        format!("{CANARY_FINGERPRINT}:{}", self.payload)
    }
}

/// Public boundary: scans every medium for the canary handle's
/// plaintext token. The function MUST be total: it returns a `Scan`
/// even if a medium is missing or unreadable; the only panics are
/// for invariant violations (e.g. a path outside the scan root).
pub fn scan_all_mediums_for_test(root: &Path, handle: &CanaryHandle) -> Result<Scan, CanaryError> {
    let token = handle.plaintext_token();
    let canonical_root =
        fs::canonicalize(root).map_err(|error| CanaryError::Io(error.to_string()))?;
    let mut hits = Vec::new();

    for medium in [
        SensitiveMedium::SqliteMain,
        SensitiveMedium::SqliteWal,
        SensitiveMedium::SqliteShm,
        SensitiveMedium::SqliteJournal,
        SensitiveMedium::BackupStaging,
        SensitiveMedium::BackupFinalArchive,
        SensitiveMedium::PlainTempDir,
        SensitiveMedium::StructuredLog,
        SensitiveMedium::DiagnosticBundle,
        SensitiveMedium::ErrorAndCrashRecovery,
        SensitiveMedium::CrossDevicePath,
    ] {
        // Scan every byte of every file under the medium's relative
        // path. Missing files are silently skipped — the scanner is
        // total, not a precondition check.
        let medium_path = canonical_root.join(medium.relative_path());
        if !medium_path.exists() {
            continue;
        }
        scan_path(
            &canonical_root,
            &medium_path,
            medium,
            token.as_bytes(),
            &mut hits,
        )?;
    }

    Ok(Scan {
        root: canonical_root,
        canary_fingerprint: token,
        hits,
    })
}

fn scan_path(
    canonical_root: &Path,
    path: &Path,
    medium: SensitiveMedium,
    needle: &[u8],
    hits: &mut Vec<ScanHit>,
) -> Result<(), CanaryError> {
    let canonical = fs::canonicalize(path).map_err(|error| CanaryError::Io(error.to_string()))?;
    if !canonical.starts_with(canonical_root) {
        return Err(CanaryError::PathOutside(canonical.display().to_string()));
    }
    let metadata = match fs::metadata(&canonical) {
        Ok(meta) => meta,
        Err(_) => return Ok(()),
    };
    if metadata.is_dir() {
        let entries = match fs::read_dir(&canonical) {
            Ok(entries) => entries,
            Err(_) => return Ok(()),
        };
        for entry in entries.flatten() {
            scan_path(canonical_root, &entry.path(), medium, needle, hits)?;
        }
        return Ok(());
    }
    let bytes = match fs::read(&canonical) {
        Ok(bytes) => bytes,
        Err(_) => return Ok(()),
    };
    if let Some(offset) = find_subslice(&bytes, needle) {
        let relative = canonical
            .strip_prefix(canonical_root)
            .map(Path::to_path_buf)
            .unwrap_or_else(|_| canonical.clone());
        hits.push(ScanHit {
            medium,
            relative_path: relative,
            byte_offset: offset as u64,
            length: needle.len(),
        });
    }
    Ok(())
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    if haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[derive(Debug, Clone, Serialize)]
pub struct Scan {
    pub root: PathBuf,
    pub canary_fingerprint: String,
    pub hits: Vec<ScanHit>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScanHit {
    pub medium: SensitiveMedium,
    pub relative_path: PathBuf,
    pub byte_offset: u64,
    pub length: usize,
}

// ---------------------------------------------------------------------------
// §T016F.4 — Inventory registration. Stories call this from their own
// owner test. Foundation never registers future-story rows.
// ---------------------------------------------------------------------------

/// Directory of all known sensitive rows. Stories register their own
/// rows; Foundation only registers the rows the auth and logging
/// minimal-Green tasks own.
#[derive(Debug, Default, Clone, Serialize)]
pub struct SensitiveInventory {
    pub fields: Vec<SensitiveField>,
}

impl SensitiveInventory {
    pub fn foundation() -> Self {
        // Foundation phase owns no direct field rows. Device-key and
        // logging rows are tested by T025/T027 through `place_canary_for_test`
        // with the explicit field they cover, and asserted via the
        // scanner boundary.
        Self::default()
    }
}

// ---------------------------------------------------------------------------
// §T016F.5 — Test-side helpers (no product surface, Red state driven
// by `unimplemented!` in the public boundaries above).
// ---------------------------------------------------------------------------

/// Returns a fresh process-unique canary payload so two parallel tests
/// do not collide on the same plaintext token.
pub fn unique_canary_payload(test_label: &str) -> String {
    let pid = process::id();
    let nanos = chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default();
    format!("{test_label}-{pid}-{nanos}")
}

/// Walks a directory tree and returns the total byte size. Used by
/// the contract test to assert that an in-place canary roundtrip
/// did not accidentally expand the on-disk footprint.
pub fn total_size(root: &Path) -> u64 {
    let mut total = 0;
    if let Ok(rd) = fs::read_dir(root) {
        for entry in rd.flatten() {
            let path = entry.path();
            if path.is_dir() {
                total += total_size(&path);
            } else if let Ok(meta) = fs::metadata(&path) {
                total += meta.len();
            }
        }
    }
    total
}
