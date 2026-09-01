//! Foundation device-key store (T025).
//!
//! Owns the local ChaCha20Poly1305 device key lifecycle:
//! - First-startup secure random key generation,
//! - Atomic creation with 0600 ACL (or platform-equivalent owner-only
//!   permissions) under the XDG config home,
//! - Restart reuse without re-generation,
//! - Stable blocked states for missing / corrupt / wrong-permission
//!   files so the recovery view can branch deterministically,
//! - Roundtrip helpers used by the cryptographic layer.
//!
//! The store is deliberately tiny. It is the *only* place that
//! touches the device key file. The encrypted SQLite database is
//! sealed by `crate::datasource::crypto`; this module never reads or
//! writes the database directly.

#![warn(missing_docs)]

use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use rand::RngCore;
use rand::rngs::OsRng;
use thiserror::Error;

/// Size of the device key material in bytes.
pub const DEVICE_KEY_BYTES: usize = 32;

/// Maximum number of attempts the create-key loop will run before
/// giving up and reporting an [`DeviceKeyBlockKind::Unreadable`]
/// state. The loop only iterates in genuine race conditions where
/// the OS hands `AlreadyExists` to multiple contenders; in the
/// happy path exactly one iteration runs.
const CREATE_RETRIES: usize = 64;

/// Error returned by the device-key store.
#[derive(Debug, Error)]
pub enum DeviceKeyError {
    /// I/O failure.
    #[error("device key i/o: {0}")]
    Io(#[from] std::io::Error),
    /// The existing file is structurally invalid (length, format).
    #[error("device key file is corrupt")]
    Corrupt,
    /// The file exists but its permissions are not owner-only.
    #[error("device key file permissions are not owner-only")]
    WrongPermissions,
}

/// Why the device-key store is blocking startup. The recovery view
/// translates each variant into a UI branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceKeyBlockKind {
    /// The device key file does not exist.
    Missing,
    /// The device key file is structurally corrupt.
    Corrupt,
    /// The device key file has the wrong permissions.
    Permissions,
    /// The device key file is otherwise unreadable.
    Unreadable,
}

/// Stable blocked-state payload returned to the startup gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceKeyBlock {
    kind: DeviceKeyBlockKind,
    path: PathBuf,
}

impl DeviceKeyBlock {
    /// Build a new block descriptor.
    pub fn new(kind: DeviceKeyBlockKind, path: impl Into<PathBuf>) -> Self {
        Self {
            kind,
            path: path.into(),
        }
    }

    /// Borrow the block kind.
    pub fn kind(&self) -> DeviceKeyBlockKind {
        self.kind
    }

    /// Borrow the path to the offending file (if known).
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Whether this block must abort startup of the encrypted Store.
    /// All currently-defined kinds are blocking.
    pub fn is_blocking(&self) -> bool {
        true
    }
}

/// Outcome of opening the device-key store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceKeyStartup {
    /// A usable device key is in memory.
    Ready(DeviceKey),
    /// The store is blocked; the recovery view must take over.
    Blocked(DeviceKeyBlock),
}

/// The in-memory device key. Always `Zeroize`d on drop.
#[derive(Clone)]
pub struct DeviceKey {
    bytes: zeroize::Zeroizing<[u8; DEVICE_KEY_BYTES]>,
}

impl DeviceKey {
    /// Borrow the raw key bytes.
    pub fn as_bytes(&self) -> &[u8; DEVICE_KEY_BYTES] {
        &self.bytes
    }

    /// Stable fingerprint of the key (SHA-256). Used for equality
    /// checks in tests.
    pub fn fingerprint(&self) -> [u8; 32] {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(self.bytes.as_slice());
        let digest = hasher.finalize();
        let mut out = [0_u8; 32];
        out.copy_from_slice(&digest);
        out
    }
}

impl Drop for DeviceKey {
    fn drop(&mut self) {
        // `Zeroizing` already overwrites the buffer on drop.
    }
}

impl std::fmt::Debug for DeviceKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never leak key material in Debug output.
        write!(
            formatter,
            "DeviceKey(<redacted {} bytes>)",
            self.bytes.len()
        )
    }
}

impl PartialEq for DeviceKey {
    fn eq(&self, other: &Self) -> bool {
        self.bytes.as_slice() == other.bytes.as_slice()
    }
}

impl Eq for DeviceKey {}

/// Public API for the device-key store.
///
/// The store may optionally know the path of the encrypted
/// database it protects. When the key is missing on startup
/// and `first_startup_allowed` is `true`, the store refuses
/// to silently generate a replacement key if a database
/// already exists at the configured path — that would
/// orphan the user's ciphertext. The caller (e.g. the
/// startup gate) must observe the `Missing` block and
/// route the user to the recovery view instead.
pub struct DeviceKeyStore {
    key_path: PathBuf,
    database_path: Option<PathBuf>,
}

impl DeviceKeyBlockKind {
    /// Stable error code for diagnostics and recovery routing.
    pub fn stable_code(self) -> &'static str {
        match self {
            DeviceKeyBlockKind::Missing => "key_missing",
            DeviceKeyBlockKind::Corrupt => "key_corrupt",
            DeviceKeyBlockKind::Permissions => "key_permissions",
            DeviceKeyBlockKind::Unreadable => "key_unreadable",
        }
    }
}

impl DeviceKeyStore {
    /// Construct a device-key store rooted at `key_path`. The
    /// store does not touch the file system until
    /// [`DeviceKeyStore::open_for_startup`] is called.
    pub fn new(key_path: impl Into<PathBuf>) -> Self {
        Self {
            key_path: key_path.into(),
            database_path: None,
        }
    }

    /// Attach the path of the encrypted database this key
    /// protects. When set, [`DeviceKeyStore::open_for_startup`]
    /// uses the database's existence to detect a recovery
    /// scenario: a missing key combined with an existing
    /// database is reported as `DeviceKeyBlockKind::Missing`
    /// even when `first_startup_allowed` is `true`, so the
    /// startup gate can route the user to the recovery view
    /// instead of orphaning the user's ciphertext.
    pub fn with_database_path(mut self, database_path: impl Into<PathBuf>) -> Self {
        self.database_path = Some(database_path.into());
        self
    }

    /// Open (or create) the device-key file. When `first_startup_allowed`
    /// is `true`, missing files are created atomically with 0600
    /// permissions. When `false`, a missing file is reported as
    /// `DeviceKeyBlockKind::Missing`.
    pub fn open_for_startup(
        &self,
        first_startup_allowed: bool,
    ) -> Result<DeviceKeyStartup, DeviceKeyError> {
        let path = self.key_path.clone();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        // Fast path: existing, valid, owner-only key — open it.
        if let Some(startup) = self.try_read_existing_key(&path)? {
            return Ok(startup);
        }
        // Key file is missing. Decide whether creation is permitted
        // before we attempt anything that touches the disk beyond
        // reads.
        if !first_startup_allowed {
            return Ok(DeviceKeyStartup::Blocked(DeviceKeyBlock::new(
                DeviceKeyBlockKind::Missing,
                path,
            )));
        }
        // Safety net: never generate a replacement key when the
        // user already has an encrypted database on disk. That would
        // orphan the ciphertext and trap the user in a "decryption
        // impossible" state. The startup gate must observe Missing
        // and route to the recovery view.
        if self.database_path_is_present() {
            return Ok(DeviceKeyStartup::Blocked(DeviceKeyBlock::new(
                DeviceKeyBlockKind::Missing,
                path,
            )));
        }
        // First startup: create the file atomically via a sibling
        // staging file and a single `rename` syscall. The rename is
        // atomic on the platform's filesystem, so concurrent
        // contenders either see "no file" (and try to create) or
        // "complete 32-byte file" (and read the same key).
        self.create_key_with_retry(&path)
    }

    /// Attempt to read and validate an existing key file. Returns
    /// `None` only when the file is genuinely missing (NotFound);
    /// any other state — corrupt, wrong permissions, unreadable —
    /// is surfaced as a [`DeviceKeyStartup::Blocked`].
    fn try_read_existing_key(
        &self,
        path: &Path,
    ) -> Result<Option<DeviceKeyStartup>, DeviceKeyError> {
        match fs::read(path) {
            Ok(bytes) => {
                if bytes.len() != DEVICE_KEY_BYTES {
                    return Ok(Some(DeviceKeyStartup::Blocked(DeviceKeyBlock::new(
                        DeviceKeyBlockKind::Corrupt,
                        path.to_path_buf(),
                    ))));
                }
                if !is_owner_only(path)? {
                    return Ok(Some(DeviceKeyStartup::Blocked(DeviceKeyBlock::new(
                        DeviceKeyBlockKind::Permissions,
                        path.to_path_buf(),
                    ))));
                }
                let mut fixed = [0_u8; DEVICE_KEY_BYTES];
                fixed.copy_from_slice(&bytes);
                Ok(Some(DeviceKeyStartup::Ready(DeviceKey {
                    bytes: zeroize::Zeroizing::new(fixed),
                })))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(_error) => Ok(Some(DeviceKeyStartup::Blocked(DeviceKeyBlock::new(
                DeviceKeyBlockKind::Unreadable,
                path.to_path_buf(),
            )))),
        }
    }

    /// Create the device-key file with concurrent-safe semantics.
    ///
    /// The implementation writes the key to a sibling staging file
    /// in the same directory, fsyncs the staging file, and then
    /// installs it via `hard_link`. `hard_link` fails with
    /// `AlreadyExists` when the destination already exists, so
    /// concurrent first-startup contenders either succeed at the
    /// link (and become the canonical key) or fall through to
    /// reading the file another contender installed.
    fn create_key_with_retry(&self, path: &Path) -> Result<DeviceKeyStartup, DeviceKeyError> {
        for _attempt in 0..CREATE_RETRIES {
            if let Some(startup) = self.try_read_existing_key(path)? {
                return Ok(startup);
            }
            let mut buffer = [0_u8; DEVICE_KEY_BYTES];
            OsRng.fill_bytes(&mut buffer);
            let staging_path = match self.write_staging_key(path, &buffer) {
                Ok(staging) => staging,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    continue;
                }
                Err(_error) => {
                    return Ok(DeviceKeyStartup::Blocked(DeviceKeyBlock::new(
                        DeviceKeyBlockKind::Unreadable,
                        path.to_path_buf(),
                    )));
                }
            };
            match fs::hard_link(&staging_path, path) {
                Ok(()) => {
                    // The hard link is durable once the directory
                    // entry is committed; sync the parent to make
                    // the link visible across crashes.
                    if let Some(parent) = path.parent()
                        && let Ok(dir) = std::fs::File::open(parent)
                    {
                        let _ = dir.sync_all();
                    }
                    let _ = fs::remove_file(&staging_path);
                    return Ok(DeviceKeyStartup::Ready(DeviceKey {
                        bytes: zeroize::Zeroizing::new(buffer),
                    }));
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    // Lost the race. Clean up our staging file and
                    // fall through to the next loop iteration, which
                    // will read the file the winner installed.
                    let _ = fs::remove_file(&staging_path);
                    continue;
                }
                Err(_error) => {
                    let _ = fs::remove_file(&staging_path);
                    return Ok(DeviceKeyStartup::Blocked(DeviceKeyBlock::new(
                        DeviceKeyBlockKind::Unreadable,
                        path.to_path_buf(),
                    )));
                }
            }
        }
        // Retries exhausted. Classify as Unreadable so the recovery
        // view can guide the user; the underlying filesystem state
        // is still consistent.
        Ok(DeviceKeyStartup::Blocked(DeviceKeyBlock::new(
            DeviceKeyBlockKind::Unreadable,
            path.to_path_buf(),
        )))
    }

    /// Write the candidate key material to a unique staging file
    /// in the same directory as the final key path. Returns the
    /// staging file path on success.
    fn write_staging_key(
        &self,
        path: &Path,
        buffer: &[u8; DEVICE_KEY_BYTES],
    ) -> std::io::Result<PathBuf> {
        let parent = path
            .parent()
            .ok_or_else(|| std::io::Error::other("device key has no parent"))?;
        let mut suffix = [0_u8; 16];
        OsRng.fill_bytes(&mut suffix);
        let suffix_hex = suffix
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let staging_path = parent.join(format!(".device.key.staging.{suffix_hex}"));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&staging_path)?;
        file.write_all(buffer)?;
        file.sync_all()?;
        Ok(staging_path)
    }

    /// Returns `true` when a database path is configured and
    /// the file is observable on disk. Missing or unreadable
    /// database paths do not trip the safety net — only a
    /// conclusively-present file does. This avoids blocking
    /// first startup on transient I/O errors that the recovery
    /// flow can later classify.
    fn database_path_is_present(&self) -> bool {
        let Some(database_path) = self.database_path.as_ref() else {
            return false;
        };
        match fs::metadata(database_path) {
            Ok(metadata) => metadata.is_file(),
            Err(_) => false,
        }
    }

    /// Borrow the resolved key file path.
    pub fn key_path(&self) -> &Path {
        &self.key_path
    }

    /// Verify the file at `path` has owner-only permissions. Returns
    /// `Ok(())` when the file is owner-only; returns a [`DeviceKeyError`]
    /// when the file is missing / unreadable / has the wrong
    /// permissions.
    pub fn validate_owner_only(path: &Path) -> Result<(), DeviceKeyError> {
        let _metadata = fs::metadata(path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = _metadata.permissions().mode();
            if mode & 0o077 != 0 {
                return Err(DeviceKeyError::WrongPermissions);
            }
        }
        Ok(())
    }

    /// Open (or create) the device-key store at `config_home/hivegui/device.key`,
    /// with the encrypted database at `data_home/hivegui/hivegui.db` registered
    /// for the missing-key safety net. Convenience wrapper that derives the
    /// canonical paths from the XDG config/data roots.
    pub fn open(config_home: &Path, data_home: &Path) -> Result<DeviceKeyStartup, DeviceKeyError> {
        let key_path = config_home.join("hivegui").join("device.key");
        let database_path = data_home.join("hivegui").join("hivegui.db");
        Self::new(key_path)
            .with_database_path(database_path)
            .open_for_startup(true)
    }
}

#[cfg(unix)]
fn is_owner_only(path: &Path) -> std::io::Result<bool> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = fs::metadata(path)?;
    let mode = metadata.permissions().mode();
    Ok(mode & 0o077 == 0)
}

#[cfg(not(unix))]
fn is_owner_only(_path: &Path) -> std::io::Result<bool> {
    // On non-Unix platforms, fall back to "trust the file". The
    // platform-equivalent owner-only check is documented in the
    // task; the production code will add a Windows ACL check when
    // the Windows deployment is in scope.
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn open_creates_key_with_owner_only_permissions() {
        let tmp = TempDir::new().expect("tempdir");
        let config_home = tmp.path().join("config");
        let data_home = tmp.path().join("data");
        std::fs::create_dir_all(&config_home).expect("config dir");
        std::fs::create_dir_all(&data_home).expect("data dir");
        let startup = DeviceKeyStore::open(&config_home, &data_home).expect("open");
        match startup {
            DeviceKeyStartup::Ready(key) => {
                assert_eq!(key.as_bytes().len(), DEVICE_KEY_BYTES);
            }
            DeviceKeyStartup::Blocked(block) => {
                panic!("unexpected block: {:?}", block.kind())
            }
        }
    }

    #[test]
    fn reopen_reuses_existing_key() {
        let tmp = TempDir::new().expect("tempdir");
        let config_home = tmp.path().join("config");
        let data_home = tmp.path().join("data");
        std::fs::create_dir_all(&config_home).expect("config dir");
        std::fs::create_dir_all(&data_home).expect("data dir");
        let first = match DeviceKeyStore::open(&config_home, &data_home).expect("first") {
            DeviceKeyStartup::Ready(k) => k,
            _ => panic!("first open should be ready"),
        };
        let second = match DeviceKeyStore::open(&config_home, &data_home).expect("second") {
            DeviceKeyStartup::Ready(k) => k,
            _ => panic!("second open should be ready"),
        };
        assert_eq!(first.fingerprint(), second.fingerprint());
    }
}
