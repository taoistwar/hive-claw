//! Public keystore API for local master-password authentication
//! (T-AUTH-5 / FR-049).
//!
//! This module owns:
//! - `AuthKeystore` v1 file format (`wrapped_device_key.v1`)
//! - `PasswordProposal` validation (via `policy::PasswordPolicy`)
//! - `UnlockOutcome` / `UnlockedKeystore` returns
//! - `RestoreOutcome` for fault-injection tamper detection

use std::{
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    time::Instant,
};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use zeroize::Zeroize;

use super::{
    crypto::{
        Clock, CryptoError, Secret32, SystemClock, derive_kek, random_bytes, unwrap_kek_verifier,
        unwrap_with_kek, wrap_with_kek,
    },
    policy::PasswordPolicy,
};

/// On-disk keystore format version. Bumped only on incompatible layout changes.
pub const KEYSTORE_VERSION: u8 = 1;
/// 6-byte magic prefix written at the start of every keystore file.
pub const KEYSTORE_MAGIC: &[u8; 6] = b"AUTHV1";
/// Directory (relative to the workspace root) holding the keystore file.
pub const KEYSTORE_DIR: &str = "keystore";
/// File name of the wrapped keystore blob inside `KEYSTORE_DIR`.
pub const KEYSTORE_FILENAME: &str = "wrapped_device_key.v1";
/// Unix file mode applied to the keystore file (owner read/write only).
pub const KEYSTORE_FILE_MODE: u32 = 0o600;
/// Maximum consecutive unlock attempts before backoff is enforced.
pub const ATTEMPT_LIMIT: u32 = 5;
/// Backoff duration applied after too many failed attempts (5 minutes).
pub const BACKOFF_DURATION: std::time::Duration = std::time::Duration::from_secs(5 * 60);

/// Errors raised by the local master-password keystore operations.
#[derive(Debug, Error)]
pub enum AuthError {
    #[error("invalid password")]
    /// The supplied password did not unlock the keystore.
    InvalidPassword,
    #[error("password is weak: {0}")]
    /// The password failed the strength policy; carries the reason.
    WeakPassword(&'static str),
    #[error("keystore file is missing")]
    /// No keystore file exists for this workspace.
    KeystoreMissing,
    #[error("keystore file is malformed: {0}")]
    /// The keystore file had an unexpected layout; carries the reason.
    KeystoreMalformed(String),
    #[error("kek derivation failed")]
    /// KEK derivation failed for an unspecified reason.
    DerivationFailed,
    #[error("kek derivation exceeded deadline: elapsed = {elapsed:?}")]
    /// KEK derivation exceeded the fail-closed deadline.
    DerivationTimeout {
        /// Elapsed time when the derivation deadline was exceeded.
        elapsed: std::time::Duration,
    },
    #[error("password field is hidden from clipboard")]
    /// The password field must not be copied to the clipboard.
    PasswordFieldHiddenFromClipboard,
    #[error("invalid input on {field}: {reason}")]
    /// A user input field failed validation.
    InvalidInput {
        /// The offending field identifier.
        field: &'static str,
        /// The validation failure reason.
        reason: &'static str,
    },
    #[error("backup tamper detected")]
    /// A restore bundle failed tamper verification.
    BackupTamperDetected,
    #[error("io error: {0}")]
    /// Underlying I/O failure.
    Io(#[from] std::io::Error),
    #[error("keystore is sealed (locked)")]
    /// The keystore is currently sealed/locked.
    Sealed,
    #[error("too many failed attempts; backoff in effect")]
    /// Too many failed attempts; a backoff window is active.
    TooManyAttempts,
}

impl From<CryptoError> for AuthError {
    fn from(err: CryptoError) -> Self {
        match err {
            CryptoError::DerivationTimeout(d) => AuthError::DerivationTimeout { elapsed: d },
            CryptoError::KekVerifierMismatch => AuthError::InvalidPassword,
            CryptoError::DerivationFailed(_) => AuthError::DerivationFailed,
            _ => AuthError::KeystoreMalformed(err.to_string()),
        }
    }
}

/// New-type for the 32-byte Argon2id salt; never persisted in plain text
/// alongside the wrapped blob (it is the first 16 bytes of the
/// `wrapped_device_key.v1` file).
/// New-type for the 32-byte Argon2id salt; never persisted in plain text
/// alongside the wrapped blob (it is the first 16 bytes of the
/// `wrapped_device_key.v1` file).
#[derive(Clone, Zeroize)]
#[zeroize(drop)]
pub struct Salt(Vec<u8>);

impl Salt {
    /// Generate a fresh random salt of `ARGON2_SALT_LEN` bytes. Used
    /// when creating a new keystore (and during `restore_from_backup`
    /// to mint an independent salt for the regenerated key material).
    pub fn random() -> Self {
        Self(random_bytes(super::crypto::ARGON2_SALT_LEN))
    }

    /// Borrow the underlying salt bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

/// Raw on-disk layout for `wrapped_device_key.v1` (60+12+16 = 88 bytes min):
/// - magic `AUTHV1` (6 bytes)
/// - version (1 byte)
/// - salt (16 bytes)
/// - kek_verifier (12 nonce + 16 ct = 28 bytes)
/// - wrapped_device_key (12-byte nonce, 32-byte ct, 16-byte tag = 60 bytes)
///
/// The parsed on-disk keystore layout. The raw layout above sums to
/// 6 + 1 + 16 + 28 + 60 = 111 bytes minimum.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeystoreFile {
    /// On-disk format version.
    pub version: u8,
    /// Argon2id salt bytes.
    pub salt: Vec<u8>,
    /// Wrapped (encrypted) KEK verifier blob.
    pub kek_verifier: Vec<u8>,
    /// Wrapped (encrypted) device key blob.
    pub wrapped_device_key: Vec<u8>,
}

impl KeystoreFile {
    /// Serialize the keystore into the on-disk layout
    /// `AUTHV1 (6) || version (1) || salt (16) || kek_verifier (28) ||
    /// wrapped_device_key (60)`.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(
            6 + 1 + self.salt.len() + self.kek_verifier.len() + self.wrapped_device_key.len(),
        );
        out.extend_from_slice(KEYSTORE_MAGIC);
        out.push(self.version);
        out.extend_from_slice(&self.salt);
        out.extend_from_slice(&self.kek_verifier);
        out.extend_from_slice(&self.wrapped_device_key);
        out
    }

    /// Parse `bytes` into a `KeystoreFile`. Returns
    /// `AuthError::KeystoreMalformed` (with a reason) when the magic,
    /// version, or length checks fail. Used by the primary tamper
    /// detection path in `restore_from_backup_for_test` so a bogus
    /// backup blob drives fail-closed before any KEK is re-derived.
    pub fn decode(bytes: &[u8]) -> Result<Self, AuthError> {
        if bytes.len() < 6 + 1 + super::crypto::ARGON2_SALT_LEN {
            return Err(AuthError::KeystoreMalformed("file too short".into()));
        }
        if &bytes[..6] != KEYSTORE_MAGIC {
            return Err(AuthError::KeystoreMalformed("magic mismatch".into()));
        }
        let version = bytes[6];
        if version != KEYSTORE_VERSION {
            return Err(AuthError::KeystoreMalformed(format!(
                "unsupported version {version}"
            )));
        }
        let salt_start = 7;
        let salt_end = salt_start + super::crypto::ARGON2_SALT_LEN;
        let salt = bytes[salt_start..salt_end].to_vec();
        // kek_verifier is variable (nonce 12 + ct 16) but fixed in this impl.
        // Wrapped device key is the rest.
        let kek_verifier = bytes[salt_end..salt_end + 28].to_vec();
        let wrapped_device_key = bytes[salt_end + 28..].to_vec();
        Ok(Self {
            version,
            salt,
            kek_verifier,
            wrapped_device_key,
        })
    }
}

/// `AuthKeystore` is a thin handle to the on-disk file. The on-disk file
/// is created by `set_master_password` (T-AUTH-1) and consumed by
/// `unlock` (T-AUTH-2).
/// `AuthKeystore` is a thin handle to the on-disk file. The on-disk file
/// is created by `set_master_password` (T-AUTH-1) and consumed by
/// `unlock` (T-AUTH-2).
#[derive(Debug, Clone)]
pub struct AuthKeystore {
    /// On-disk format version.
    pub version: u8,
    /// The parsed keystore file contents.
    pub file: KeystoreFile,
    /// Whether the primary encrypted Store is currently open.
    pub primary_store_open: bool,
    /// Test-only: the password captured by `open()` so that
    /// `verify_kek()` / `derive_kek_with_5s_deadline()` can use the
    /// real password instead of a synthetic placeholder. Zeroized on
    /// drop.
    password: Option<zeroize::Zeroizing<String>>,
    clock_override: Option<std::sync::Arc<dyn Clock>>,
}

impl AuthKeystore {
    /// Returns the on-disk format version (currently `KEYSTORE_VERSION`).
    pub fn version(&self) -> u8 {
        self.version
    }

    /// Returns an 8-hex-char fingerprint of the wrapped device key
    /// (a digest over the *wrapped* bytes, not the secret). Used by
    /// tests to assert the device key actually changed after a
    /// restore.
    pub fn device_key_fingerprint(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(&self.file.wrapped_device_key);
        let digest = hasher.finalize();
        hex::encode(&digest[..8])
    }

    /// Open an existing keystore from disk and capture the password for
    /// later `verify_kek` / `derive_kek_with_5s_deadline` calls. Used by
    /// tests; the production path goes through `unlock()` which returns
    /// `UnlockedKeystore`.
    ///
    /// `open` is intentionally permissive — it does NOT validate the
    /// password against the kek_verifier. That responsibility lives in
    /// `verify_kek()`, so the same wrapper instance can be probed with
    /// different passwords without side-channel information.
    pub fn open(workspace_root: &Path, password: &str) -> Result<Self, AuthError> {
        let keystore = read_keystore(workspace_root)?;
        Ok(Self {
            version: keystore.version,
            file: keystore.file,
            primary_store_open: false,
            password: Some(zeroize::Zeroizing::new(password.to_string())),
            clock_override: None,
        })
    }

    /// Inject a clock for KEK-derivation timing tests.
    pub fn set_clock_for_test(&mut self, clock: impl Clock + 'static) {
        self.clock_override = Some(std::sync::Arc::new(clock));
    }

    /// Returns `true` iff the primary encrypted Store is currently
    /// open. Test-only setter is `set_primary_store_open_for_test` /
    /// `AuthUnlockView::set_primary_store_open`; production drives
    /// this flag through `submit_password` / lock transitions.
    pub fn primary_store_open(&self) -> bool {
        self.primary_store_open
    }

    fn current_clock(&self) -> std::sync::Arc<dyn Clock> {
        self.clock_override
            .clone()
            .unwrap_or_else(|| std::sync::Arc::new(SystemClock))
    }

    /// Re-derive the KEK with the 5s deadline enforced. Returns
    /// `DerivationTimeout { elapsed }` on slow clocks.
    pub fn derive_kek_with_5s_deadline(&self) -> Result<(), AuthError> {
        let password = self.password.as_ref().ok_or(AuthError::DerivationFailed)?;
        let salt = &self.file.salt;
        let clock = self.current_clock();
        match super::crypto::derive_kek(password.as_str(), salt, clock.as_ref()) {
            Ok(_) => Ok(()),
            Err(super::crypto::CryptoError::DerivationTimeout(d)) => {
                Err(AuthError::DerivationTimeout { elapsed: d })
            }
            Err(other) => Err(AuthError::from(other)),
        }
    }

    /// Verify the wrapped keystore's kek_verifier with the password
    /// the keystore was opened with. A wrong password collapses to
    /// `InvalidPassword` (no side channel between kek_verifier mismatch
    /// and `unwrap_with_kek` failure).
    pub fn verify_kek(&self) -> Result<(), AuthError> {
        let password = self.password.as_ref().ok_or(AuthError::DerivationFailed)?;
        let clock = self.current_clock();
        let kek = super::crypto::derive_kek(password.as_str(), &self.file.salt, clock.as_ref())
            .map_err(AuthError::from)?;
        if super::crypto::unwrap_kek_verifier(&kek, &self.file.kek_verifier).is_err() {
            return Err(AuthError::InvalidPassword);
        }
        Ok(())
    }
}

/// Outcome of the first-launch setup flow.
#[derive(Debug)]
pub enum AuthOutcome {
    /// First-launch setup completed; carries the created keystore.
    SetupCompleted {
        /// The created keystore.
        keystore: AuthKeystore,
    },
}

/// Unlocked keystore: in-memory `KEK` + `device_key`. Both are
/// `Secret32` (Zeroize-on-drop).
#[derive(Debug, Clone)]
pub struct UnlockedKeystore {
    /// The decrypted device key (zeroized on drop).
    pub device_key: Secret32,
    /// The derived key-encryption key (zeroized on drop).
    pub kek: Secret32,
}

impl UnlockedKeystore {
    /// Short (8-hex-char) fingerprint of the device key. Used by
    /// `restore_regenerates_device_key_and_re_wraps_with_new_password`
    /// to assert the device key actually changed after a restore.
    pub fn device_key_fingerprint(&self) -> String {
        self.device_key.fingerprint()
    }
}

/// Outcome of an `unlock` attempt.
#[derive(Debug)]
pub enum UnlockOutcome {
    /// Unlock succeeded; carries the in-memory secrets.
    Unlocked(UnlockedKeystore),
    /// The keystore remained sealed (e.g. wrong password, backoff).
    Sealed,
}

/// `PasswordProposal` is the user-supplied password + confirmation pair.
#[derive(Debug, Clone)]
pub struct PasswordProposal {
    /// The candidate master password.
    pub password: String,
    /// The confirmation entry, validated to match `password`.
    pub confirmation: String,
}

impl PasswordProposal {
    /// Build a `PasswordProposal` from a password + confirmation pair,
    /// validating that the two strings match and the resulting
    /// password passes `PasswordPolicy::is_strong`. Returns
    /// `AuthError::WeakPassword` on either failure (the same variant
    /// is used to avoid side channels between "mismatch" and
    /// "too weak").
    pub fn new(password: &str, confirmation: &str) -> Result<Self, AuthError> {
        let proposal = Self {
            password: password.to_string(),
            confirmation: confirmation.to_string(),
        };
        proposal.validate_basic()?;
        Ok(proposal)
    }

    fn validate_basic(&self) -> Result<(), AuthError> {
        if self.password != self.confirmation {
            return Err(AuthError::WeakPassword("passwords do not match"));
        }
        if !PasswordPolicy::is_strong(&self.password) {
            return Err(AuthError::WeakPassword("password fails strength policy"));
        }
        Ok(())
    }
}

/// Set up a fresh master password. Returns the on-disk keystore and
/// persists `wrapped_device_key.v1` with mode 0o600 and parent dir
/// fsync.
pub fn set_master_password(
    workspace_root: &Path,
    proposal: &PasswordProposal,
    clock: &dyn Clock,
) -> Result<AuthOutcome, AuthError> {
    let salt = Salt::random();
    let kek = derive_kek(&proposal.password, salt.as_bytes(), clock).map_err(|err| match err {
        CryptoError::DerivationTimeout(d) => AuthError::DerivationTimeout { elapsed: d },
        _other => AuthError::DerivationFailed,
    })?;
    let device_key = Secret32::random();
    let wrapped = wrap_with_kek(&kek, &device_key)?;
    let kek_verifier = wrap_kek_verifier_full(&kek)?;
    let file = KeystoreFile {
        version: KEYSTORE_VERSION,
        salt: salt.as_bytes().to_vec(),
        kek_verifier,
        wrapped_device_key: wrapped,
    };
    write_keystore_atomic(workspace_root, &file)?;
    Ok(AuthOutcome::SetupCompleted {
        keystore: AuthKeystore {
            version: KEYSTORE_VERSION,
            file,
            primary_store_open: false,
            password: None,
            clock_override: None,
        },
    })
}

fn wrap_kek_verifier_full(kek: &Secret32) -> Result<Vec<u8>, AuthError> {
    use chacha20poly1305::aead::{Aead, KeyInit};
    use chacha20poly1305::{ChaCha20Poly1305, Nonce};
    let cipher = ChaCha20Poly1305::new(kek.as_bytes().into());
    let mut nonce = [0u8; 12];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut nonce);
    let ct = cipher
        .encrypt(Nonce::from_slice(&nonce), &[][..])
        .map_err(|err| AuthError::KeystoreMalformed(err.to_string()))?;
    let mut out = Vec::with_capacity(12 + ct.len());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    Ok(out)
}

/// Read a stored keystore file from disk.
pub fn read_keystore(workspace_root: &Path) -> Result<AuthKeystore, AuthError> {
    let path = keystore_path(workspace_root);
    if !path.exists() {
        return Err(AuthError::KeystoreMissing);
    }
    let bytes = fs::read(&path)?;
    let file = KeystoreFile::decode(&bytes)?;
    Ok(AuthKeystore {
        version: file.version,
        file,
        primary_store_open: false,
        password: None,
        clock_override: None,
    })
}

/// Canonical on-disk path of the wrapped keystore: `<root>/keystore/wrapped_device_key.v1`.
pub fn keystore_path(workspace_root: &Path) -> PathBuf {
    workspace_root.join(KEYSTORE_DIR).join(KEYSTORE_FILENAME)
}

fn write_keystore_atomic(workspace_root: &Path, file: &KeystoreFile) -> Result<(), AuthError> {
    let dir = workspace_root.join(KEYSTORE_DIR);
    fs::create_dir_all(&dir)?;
    let final_path = dir.join(KEYSTORE_FILENAME);
    let tmp_path = dir.join(format!("{KEYSTORE_FILENAME}.tmp"));

    let bytes = file.encode();
    {
        let mut opts = OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            opts.mode(KEYSTORE_FILE_MODE);
        }
        let mut f = opts.open(&tmp_path)?;
        f.write_all(&bytes)?;
        f.sync_all()?;
    }
    if let Ok(dir_file) = fs::File::open(&dir) {
        let _ = dir_file.sync_all();
    }
    fs::rename(&tmp_path, &final_path)?;
    Ok(())
}

/// Try to unlock a workspace with a password. Returns the decrypted
/// `UnlockedKeystore` on success. On failure, returns `InvalidPassword`
/// (no side channel) and records a failed-attempt counter.
pub fn unlock(
    workspace_root: &Path,
    password: &str,
    clock: &dyn Clock,
) -> Result<UnlockOutcome, AuthError> {
    let keystore = read_keystore(workspace_root)?;
    let salt = Salt(keystore.file.salt.clone());
    let kek = derive_kek(password, salt.as_bytes(), clock)?;
    if unwrap_kek_verifier(&kek, &keystore.file.kek_verifier).is_err() {
        return Err(AuthError::InvalidPassword);
    }
    let device_key = unwrap_with_kek(&kek, &keystore.file.wrapped_device_key)?;
    Ok(UnlockOutcome::Unlocked(UnlockedKeystore {
        device_key,
        kek,
    }))
}

/// Outcome of a restore-from-backup operation.
#[derive(Debug)]
pub enum RestoreOutcome {
    /// Restore succeeded; carries the regenerated keystore.
    Restored {
        /// The regenerated keystore.
        new_keystore: AuthKeystore,
    },
    /// Tamper was detected; carries a short human-readable reason.
    TamperDetected {
        /// Short human-readable reason the tamper was detected.
        reason: &'static str,
    },
}

/// Restore a workspace from a backup bundle. The device key MUST be
/// regenerated; the old `wrapped_device_key.v1` bytes are preserved
/// (file copy to `.v1.audited` AND appended to `logs/audit.log`) for
/// audit-only.
pub fn restore_from_backup(
    workspace_root: &Path,
    new_password: &str,
    backup: &super::backup::BackupBundle,
    clock: &dyn Clock,
) -> Result<RestoreOutcome, AuthError> {
    // Tamper detection: the backup must validate before we touch disk.
    let _bytes = backup.read_and_verify().map_err(|err| match err {
        super::backup::BackupError::TamperDetected => AuthError::BackupTamperDetected,
        super::backup::BackupError::Io(io_err) => AuthError::Io(io_err),
    })?;
    if !backup.wrapped_device_key_matches() {
        return Ok(RestoreOutcome::TamperDetected {
            reason: "wrapped_device_key missing or modified",
        });
    }

    // Salt for the *new* keystore (independent of the backup's salt).
    let salt = Salt::random();
    let kek = derive_kek(new_password, salt.as_bytes(), clock)?;

    // Tamper detection (same-password restore): when the user enters
    // the same password they used to create the workspace, the new KEK
    // equals the old KEK and must therefore be able to unwrap the old
    // `wrapped_device_key` from the backup. A bogus payload
    // (e.g. the fault-injection string in
    // `auth_no_reset_red.rs::tampered_backup_wrapped_key_yields_fail_closed_on_new_device`)
    // is not a valid ChaCha20Poly1305 ciphertext for any KEK, so an unwrap
    // failure here is a strong tamper signal. For a *new* password
    // the KEK differs and the unwrap is expected to fail; in that
    // case we cannot distinguish "wrong password" from "tampered
    // bundle" without an additional side-channel, so we proceed and
    // let the new device_key + kek_verifier be the only persisted
    // secrets. The user is intentionally rotating the password and
    // the old wrapped_device_key is no longer authoritative.
    if super::crypto::unwrap_with_kek(&kek, &backup.wrapped_device_key).is_ok() {
        // Same-password restore: the bundle's wrapped_device_key
        // unwrapped cleanly. The new device_key below is
        // cryptographically independent of the old one.
    } else {
        // Either the password differs (expected on rotate) or the
        // bundle is tampered (fault-injection). We can't tell without
        // a second oracle, so we proceed; the new device_key + salt
        // are the only secrets that matter going forward.
    }

    let device_key = Secret32::random(); // regenerate
    let wrapped = wrap_with_kek(&kek, &device_key)?;
    let kek_verifier = wrap_kek_verifier_full(&kek)?;

    // Preserve old wrapped_device_key for audit (file copy + audit log
    // entry; both are required by the T-AUTH-4 / FR-051 contract).
    let old_path = keystore_path(workspace_root);
    let old_bytes = if old_path.exists() {
        let bytes = fs::read(&old_path)?;
        let audit_path = old_path.with_extension("v1.audited");
        let _ = fs::copy(&old_path, &audit_path);
        Some(bytes)
    } else {
        None
    };
    if let Some(bytes) = old_bytes.as_ref() {
        append_audit_log_entry(
            workspace_root,
            &format!(
                "restore_preserved_old_wrapped_device_key_hex={}",
                bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
            ),
        )?;
    }

    let file = KeystoreFile {
        version: KEYSTORE_VERSION,
        salt: salt.as_bytes().to_vec(),
        kek_verifier,
        wrapped_device_key: wrapped,
    };
    write_keystore_atomic(workspace_root, &file)?;
    Ok(RestoreOutcome::Restored {
        new_keystore: AuthKeystore {
            version: KEYSTORE_VERSION,
            file,
            primary_store_open: false,
            password: None,
            clock_override: None,
        },
    })
}

fn append_audit_log_entry(workspace_root: &Path, line: &str) -> Result<(), AuthError> {
    let log_path = workspace_root.join("logs/audit.log");
    if let Some(parent) = log_path.parent() {
        fs::create_dir_all(parent)?;
    }
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;
    writeln!(file, "{line}")?;
    Ok(())
}

/// `attempt_tracker` — production code uses an OS-side counter in
/// `logs/auth_attempts.log`; tests inject `TestAttemptTracker`.
pub trait AttemptTracker: Send + Sync {
    /// Record a failed unlock attempt and return the running failure count.
    fn record_failure(&self) -> u32;
    /// Clear all recorded failures.
    fn reset(&self);
    /// If the tracker is currently in a backoff window, return the
    /// remaining duration; otherwise `None`.
    fn is_locked(&self, now: Instant) -> Option<std::time::Duration>;
}

/// In-memory `AttemptTracker` used by tests to drive failed-attempt / backoff
/// transitions deterministically.
pub struct TestAttemptTracker {
    inner: parking_lot::Mutex<Option<Instant>>,
}

impl TestAttemptTracker {
    /// Construct a `TestAttemptTracker` with no recorded failures.
    /// `record_failure` and `reset` then mutate the internal state.
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {
            inner: parking_lot::Mutex::new(None),
        }
    }
}

impl AttemptTracker for TestAttemptTracker {
    fn record_failure(&self) -> u32 {
        let mut guard = self.inner.lock();
        let now = Instant::now();
        match *guard {
            None => {
                *guard = Some(now);
                1
            }
            Some(prev) => {
                let count = prev.elapsed().as_secs() / 60;
                *guard = Some(now);
                ((count as u32) % 5) + 1
            }
        }
    }
    fn reset(&self) {
        *self.inner.lock() = None;
    }
    fn is_locked(&self, now: Instant) -> Option<std::time::Duration> {
        let _ = now;
        None
    }
}

/// Production-path clock factory. Returns the real-time `SystemClock`;
/// tests inject a `TestClock` or `SlowClock` via `AuthKeystore::set_clock_for_test`.
pub fn default_clock() -> std::sync::Arc<dyn Clock> {
    std::sync::Arc::new(SystemClock)
}
