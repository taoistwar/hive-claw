//! T129/T130 [US13] Age-encrypted backup export + restore contract.
//!
//! Source of truth: `specs/011-hivegui-standalone-mode/tasks.md` §T129
//! and §T130 (export + restore / switch verification).
//!
//! Red public boundaries the T129/T130 implementation must satisfy:
//!   - `hivegui::datasource::backup::BackupExporter::export_age` writes
//!     a passphrase-encrypted age stream that round-trips through
//!     [`BackupImporter::import_age`] using a matching passphrase.
//!   - The portable export carries the exact v4 `manifest.json`, all 22 user
//!     entity JSON files (including empty types), and owned Plugin WASM. The
//!     separate restore instance carries the v1 six-tuple unarmed manifest.
//!   - The export refuses to overwrite an existing target file (the
//!     caller is responsible for choosing a fresh target).
//!   - The export refuses symlinks / hardlinks / device files /
//!     FIFOs / sockets and other non-regular entries.
//!   - Format 1/2/3 archives are accepted and upgraded once. Legacy integer
//!     kinds and dotted Builtins are mapped only for legacy formats; current
//!     archives reject them and all short `node_type` values.
//!   - The restore side writes to a staging directory and refuses to
//!     touch the canonical `datasources.db` directly.

#![allow(missing_docs)]

mod support;

use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Output},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use hivegui::datasource::backup::{
    BACKUP_CONFIRMATION_CRASH_POINTS, BackupCoordinator, BackupExporter, BackupImporter,
    ExportError, ImportError, PreparedRestore, RESTORE_SWITCH_CRASH_POINTS,
    RETIREMENT_CRASH_POINTS, RestoreConfirmation, RestoreCoordinator, RetirementOutcome,
    SIDECAR_CLEANUP_CRASH_POINTS,
};
use hivegui::datasource::entity_store::{AgentInput, AgentStore};
use hivegui::datasource::{
    Crypto, DatabaseFailureClass, Store, StoreOpenErrorKind, StoreOpenOptions,
};
use hivegui::ui::app::open_store_after_restore_recovery;
use sha2::{Digest, Sha256};
use support::TestWorkspace;
use support::sensitive_canary::{
    SensitiveField, place_canary_for_test, scan_all_mediums_for_test, unique_canary_payload,
};
use uuid::Uuid;

const RESTORE_STORE_LOCK_CHILD_MODE: &str = "HIVEGUI_T130_RESTORE_STORE_LOCK_CHILD_MODE";
const RESTORE_STORE_LOCK_CHILD_ROOT: &str = "HIVEGUI_T130_RESTORE_STORE_LOCK_CHILD_ROOT";
const PREPARED_RESTORE_NOT_ACTIVE: &str = "prepared_restore_not_active";

fn unique_target_path(workspace: &TestWorkspace, label: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    workspace.root().join(format!("{label}-{nanos}.age.tar"))
}

async fn write_seed_database(workspace: &TestWorkspace) -> std::path::PathBuf {
    let store = Store::open_local(StoreOpenOptions::new(
        workspace.database_path(),
        workspace.plugin_root(),
    ))
    .await
    .expect("open real v4 seed Store");
    store
        .create(
            "backup-seed",
            "127.0.0.1",
            3306,
            "seed-user",
            b"seed-password",
        )
        .await
        .expect("seed real user entity");
    store.pool().close().await;
    drop(store);
    workspace.database_path().to_path_buf()
}

async fn export_single_data_source_archive(
    workspace: &TestWorkspace,
    archive_label: &str,
    data_source_name: &str,
    passphrase: &str,
) -> std::path::PathBuf {
    let store = Store::open_local(StoreOpenOptions::new(
        workspace.database_path(),
        workspace.plugin_root(),
    ))
    .await
    .expect("open archive source Store");
    store
        .create(
            data_source_name,
            "127.0.0.1",
            3306,
            "archive-user",
            b"archive-password",
        )
        .await
        .expect("seed archive source state");
    store.pool().close().await;
    drop(store);

    let archive = unique_target_path(workspace, archive_label);
    BackupExporter::new(workspace.database_path())
        .export_age(&archive, passphrase)
        .await
        .expect("export archive source state");
    archive
}

async fn export_data_source_and_plugin_archive(
    workspace: &TestWorkspace,
    archive_label: &str,
    data_source_name: &str,
    plugin_identifier: &str,
    artifact_key: &str,
    wasm: &[u8],
    passphrase: &str,
) -> std::path::PathBuf {
    let store = Store::open_local(StoreOpenOptions::new(
        workspace.database_path(),
        workspace.plugin_root(),
    ))
    .await
    .expect("open archive source Store");
    store
        .create(
            data_source_name,
            "127.0.0.1",
            3306,
            "archive-user",
            b"archive-password",
        )
        .await
        .expect("seed archive source state");
    let artifact = workspace.plugin_root().join(artifact_key);
    fs::create_dir_all(artifact.parent().expect("new Plugin artifact parent"))
        .expect("create new Plugin artifact parent");
    fs::write(&artifact, wasm).expect("write new managed Plugin artifact");
    sqlx::query(
        "INSERT INTO plugins \
         (identifier,name,version,s3_key,sha256,size_bytes,runtime,created_at,updated_at) \
         VALUES (?,?,'1.0.0',?,?,?,'wasm32','2026-08-27T00:00:00Z','2026-08-27T00:00:00Z')",
    )
    .bind(plugin_identifier)
    .bind("New Restore Plugin")
    .bind(artifact_key)
    .bind(hex::encode(Sha256::digest(wasm)))
    .bind(i64::try_from(wasm.len()).expect("new WASM fixture size"))
    .execute(store.pool())
    .await
    .expect("seed new Plugin ownership");
    store.pool().close().await;
    drop(store);

    let archive = unique_target_path(workspace, archive_label);
    BackupExporter::new(workspace.database_path())
        .export_age(&archive, passphrase)
        .await
        .expect("export data source and Plugin archive");
    archive
}

async fn seed_data_source_and_plugin_fixture(
    store: &Store,
    plugin_root: &Path,
    data_source_name: &str,
    plugin_identifier: &str,
    artifact_key: &str,
    wasm: &[u8],
) {
    store
        .create(
            data_source_name,
            "127.0.0.1",
            3307,
            "old-user",
            b"old-password",
        )
        .await
        .expect("seed distinguishable current data source");
    let artifact = plugin_root.join(artifact_key);
    fs::create_dir_all(artifact.parent().expect("current Plugin artifact parent"))
        .expect("create current Plugin artifact parent");
    fs::write(&artifact, wasm).expect("write current Plugin artifact");
    sqlx::query(
        "INSERT INTO plugins \
         (identifier,name,version,s3_key,sha256,size_bytes,runtime,created_at,updated_at) \
         VALUES (?,?,'1.0.0',?,?,?,'wasm32','2026-08-27T00:00:00Z','2026-08-27T00:00:00Z')",
    )
    .bind(plugin_identifier)
    .bind(format!("{plugin_identifier} fixture"))
    .bind(artifact_key)
    .bind(hex::encode(Sha256::digest(wasm)))
    .bind(i64::try_from(wasm.len()).expect("current WASM fixture size"))
    .execute(store.pool())
    .await
    .expect("seed current Plugin ownership");
}

async fn write_restore_current_database(root: &Path) -> std::path::PathBuf {
    let database = root.join("datasources.db");
    let store = Store::open_local(StoreOpenOptions::new(&database, root.join("plugins")))
        .await
        .expect("open exact current restore layout");
    store
        .create(
            "sidecar-current-seed",
            "127.0.0.1",
            3306,
            "sidecar-user",
            b"sidecar-password",
        )
        .await
        .expect("seed exact current restore database");
    store.pool().close().await;
    drop(store);
    database
}

fn sha256_path(path: &Path) -> String {
    hex::encode(Sha256::digest(fs::read(path).expect("read hash input")))
}

async fn data_source_names_from_database(database: &Path) -> Vec<String> {
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(database)
        .create_if_missing(false)
        .read_only(true)
        .immutable(true)
        .foreign_keys(true);
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .min_connections(1)
        .max_connections(1)
        .connect_with(options)
        .await
        .expect("open database for raw state verification");
    let names = sqlx::query_scalar::<_, String>("SELECT name FROM data_sources ORDER BY name")
        .fetch_all(&pool)
        .await
        .expect("read raw data source names");
    pool.close().await;
    names
}

#[tokio::test(flavor = "current_thread")]
async fn child_store_bound_restore_lock_probe() {
    let Some(mode) = env::var_os(RESTORE_STORE_LOCK_CHILD_MODE) else {
        return;
    };
    let root = PathBuf::from(
        env::var_os(RESTORE_STORE_LOCK_CHILD_ROOT).expect("child restore root is configured"),
    );
    let result = Store::open_local(StoreOpenOptions::for_root(&root)).await;
    match mode.to_string_lossy().as_ref() {
        "expect-locked" => {
            let error = match result {
                Ok(_) => panic!("child opened a Store whose restore file lock must remain held"),
                Err(error) => error,
            };
            assert_eq!(error.kind(), StoreOpenErrorKind::AlreadyLocked);
        }
        "expect-open" => {
            let store = result.expect("child acquires the Store lock after startup recovery");
            store
                .list()
                .await
                .expect("child reads the recovered current Store");
            store.pool().close().await;
            drop(store);
        }
        other => panic!("unknown restore Store lock child mode: {other}"),
    }
}

fn run_store_bound_restore_lock_child(root: &Path, mode: &str) -> Output {
    Command::new(env::current_exe().expect("resolve backup_restore integration-test executable"))
        .args([
            "--exact",
            "child_store_bound_restore_lock_probe",
            "--nocapture",
        ])
        .env(RESTORE_STORE_LOCK_CHILD_MODE, mode)
        .env(RESTORE_STORE_LOCK_CHILD_ROOT, root)
        .output()
        .expect("run isolated restore Store lock child process")
}

fn assert_store_bound_restore_lock_child(output: &Output, context: &str) {
    assert!(
        output.status.success(),
        "{context}\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(target_os = "linux")]
struct DataIoWatch {
    descriptor: std::os::fd::RawFd,
    _watch_descriptor: i32,
}

#[cfg(target_os = "linux")]
impl DataIoWatch {
    fn new(path: &Path) -> Self {
        Self::with_mask(
            path,
            libc::IN_OPEN | libc::IN_ACCESS | libc::IN_MODIFY | libc::IN_CLOSE_WRITE,
        )
    }

    fn new_directory_tree(path: &Path) -> Self {
        Self::with_mask(
            path,
            libc::IN_OPEN
                | libc::IN_ACCESS
                | libc::IN_MODIFY
                | libc::IN_CLOSE_WRITE
                | libc::IN_CREATE
                | libc::IN_DELETE
                | libc::IN_DELETE_SELF,
        )
    }

    fn with_mask(path: &Path, mask: u32) -> Self {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt as _;

        let descriptor = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
        assert!(
            descriptor >= 0,
            "create nonblocking inotify descriptor: {}",
            std::io::Error::last_os_error()
        );
        let path = CString::new(path.as_os_str().as_bytes()).expect("inotify path has no NUL");
        let watch_descriptor = unsafe { libc::inotify_add_watch(descriptor, path.as_ptr(), mask) };
        if watch_descriptor < 0 {
            let error = std::io::Error::last_os_error();
            unsafe {
                libc::close(descriptor);
            }
            panic!("watch replacement database inode: {error}");
        }
        Self {
            descriptor,
            _watch_descriptor: watch_descriptor,
        }
    }

    fn drain_masks(&self) -> Vec<u32> {
        let mut masks = Vec::new();
        let mut buffer = [0_u8; 4096];
        loop {
            let read =
                unsafe { libc::read(self.descriptor, buffer.as_mut_ptr().cast(), buffer.len()) };
            if read < 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() == std::io::ErrorKind::WouldBlock {
                    break;
                }
                panic!("read replacement database inotify events: {error}");
            }
            if read == 0 {
                break;
            }
            let read = read as usize;
            let mut offset = 0_usize;
            while offset + std::mem::size_of::<libc::inotify_event>() <= read {
                let event = unsafe {
                    std::ptr::read_unaligned(
                        buffer.as_ptr().add(offset).cast::<libc::inotify_event>(),
                    )
                };
                masks.push(event.mask);
                offset += std::mem::size_of::<libc::inotify_event>() + event.len as usize;
            }
        }
        masks
    }
}

#[cfg(target_os = "linux")]
impl Drop for DataIoWatch {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.descriptor);
        }
    }
}

#[cfg(target_os = "linux")]
fn exchange_paths(left: &Path, right: &Path) {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt as _;

    let left = CString::new(left.as_os_str().as_bytes()).expect("left path has no NUL");
    let right = CString::new(right.as_os_str().as_bytes()).expect("right path has no NUL");
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            left.as_ptr(),
            libc::AT_FDCWD,
            right.as_ptr(),
            libc::RENAME_EXCHANGE,
        )
    };
    assert_eq!(
        result,
        0,
        "atomically exchange database leaves: {}",
        std::io::Error::last_os_error()
    );
}

#[cfg(target_os = "linux")]
struct PathExchangeGuard {
    left: std::path::PathBuf,
    right: std::path::PathBuf,
    restore_on_drop: bool,
}

#[cfg(target_os = "linux")]
impl PathExchangeGuard {
    fn new(left: &Path, right: &Path) -> Self {
        exchange_paths(left, right);
        Self {
            left: left.to_path_buf(),
            right: right.to_path_buf(),
            restore_on_drop: true,
        }
    }

    fn restore(mut self) {
        exchange_paths(&self.left, &self.right);
        self.restore_on_drop = false;
    }
}

#[cfg(target_os = "linux")]
impl Drop for PathExchangeGuard {
    fn drop(&mut self) {
        if self.restore_on_drop {
            exchange_paths(&self.left, &self.right);
        }
    }
}

#[cfg(target_os = "linux")]
struct RestorableDirectoryExchangeGuard {
    left: PathBuf,
    right: PathBuf,
    restore_on_drop: bool,
}

#[cfg(target_os = "linux")]
impl RestorableDirectoryExchangeGuard {
    fn new(left: &Path, right: &Path) -> Self {
        exchange_paths(left, right);
        Self {
            left: left.to_path_buf(),
            right: right.to_path_buf(),
            restore_on_drop: true,
        }
    }

    fn restore_paths(left: &Path, right: &Path) -> std::io::Result<()> {
        if left.exists() && right.exists() {
            exchange_paths(left, right);
            Ok(())
        } else if left.exists() && !right.exists() {
            fs::rename(left, right)
        } else {
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "foreign snapshot directory was removed",
            ))
        }
    }

    fn restore(mut self) {
        Self::restore_paths(&self.left, &self.right)
            .expect("restore exchanged foreign snapshot directory");
        self.restore_on_drop = false;
    }
}

#[cfg(target_os = "linux")]
impl Drop for RestorableDirectoryExchangeGuard {
    fn drop(&mut self) {
        if self.restore_on_drop {
            let _ = Self::restore_paths(&self.left, &self.right);
        }
    }
}

#[cfg(target_os = "linux")]
fn open_linux_fd_count_for_identity(expected: (u64, u64)) -> usize {
    use std::os::unix::fs::MetadataExt as _;

    fs::read_dir("/proc/self/fd")
        .expect("read Linux process descriptor table")
        .filter_map(Result::ok)
        .filter(|entry| {
            fs::metadata(entry.path())
                .is_ok_and(|metadata| (metadata.dev(), metadata.ino()) == expected)
        })
        .count()
}

#[cfg(target_os = "linux")]
async fn finish_with_snapshot_binding_interlocks(
    confirmation: RestoreConfirmation,
    barriers: [std::sync::Arc<tokio::sync::Barrier>; 4],
) -> Result<(), ImportError> {
    confirmation
        .finish_with_snapshot_binding_interlocks_for_test(barriers)
        .await
        .map(|_| ())
}

fn regular_tree_bytes(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, current: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        let mut entries = fs::read_dir(current)
            .expect("read regular fixture tree")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect regular fixture tree");
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).expect("inspect regular fixture entry");
            assert!(!metadata.file_type().is_symlink());
            if metadata.is_dir() {
                visit(root, &path, out);
            } else {
                assert!(metadata.is_file());
                out.insert(
                    path.strip_prefix(root)
                        .expect("fixture entry remains beneath root")
                        .to_path_buf(),
                    fs::read(path).expect("read regular fixture bytes"),
                );
            }
        }
    }

    let mut out = BTreeMap::new();
    visit(root, root, &mut out);
    out
}

#[cfg(target_os = "linux")]
fn watch_regular_tree(root: &Path) -> Vec<DataIoWatch> {
    fn visit(current: &Path, out: &mut Vec<DataIoWatch>) {
        out.push(DataIoWatch::new_directory_tree(current));
        let mut entries = fs::read_dir(current)
            .expect("read foreign watch tree")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect foreign watch tree");
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).expect("inspect foreign watch entry");
            assert!(!metadata.file_type().is_symlink());
            if metadata.is_dir() {
                visit(&path, out);
            } else {
                assert!(metadata.is_file());
                out.push(DataIoWatch::new(&path));
            }
        }
    }

    let mut watches = Vec::new();
    visit(root, &mut watches);
    for watch in &watches {
        let _ = watch.drain_masks();
    }
    watches
}

#[cfg(target_os = "linux")]
fn assert_current_snapshot_copy_seam_source_contract() {
    fn item_body<'a>(normalized: &'a str, signature: &str) -> &'a str {
        let start = normalized.find(signature).unwrap_or_else(|| {
            panic!("missing production source item: {signature}");
        });
        let body_start = normalized[start..]
            .find('{')
            .map(|offset| start + offset)
            .unwrap_or_else(|| panic!("missing body for production source item: {signature}"));
        let mut depth = 0_usize;
        for (offset, character) in normalized[body_start..].char_indices() {
            match character {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return &normalized[start..body_start + offset + character.len_utf8()];
                    }
                }
                _ => {}
            }
        }
        panic!("unterminated production source item: {signature}");
    }

    fn assert_single_finish_tail_call(item: &str, label: &str) {
        let body_start = item
            .find('{')
            .map(|offset| offset + 1)
            .expect("finish wrapper body");
        let call_start = item
            .find("self.finish_internal(")
            .expect("shared finish tail call");
        assert!(
            item[body_start..call_start].trim().is_empty(),
            "{label} must do no work before its shared finish tail-call"
        );
        assert_eq!(
            item.matches("self.finish_internal(").count(),
            1,
            "{label} must call the shared private finish exactly once: {item}"
        );
        assert_eq!(
            item.matches(".await").count(),
            1,
            "{label} must contain only the shared finish await: {item}"
        );
        for forbidden_control_flow in [" if ", " if let ", " match ", " loop ", " while ", " for "]
        {
            assert!(
                !item.contains(forbidden_control_flow),
                "{label} must not select a test-only I/O path: {forbidden_control_flow} in {item}"
            );
        }
        let await_end = item
            .rfind(".await")
            .map(|offset| offset + ".await".len())
            .expect("single finish await");
        assert_eq!(
            item[await_end..].trim(),
            "}",
            "{label} must tail-call the shared private finish with no fallback work"
        );
    }

    let source = include_str!("../src/datasource/backup.rs");
    let normalized = source.split_whitespace().collect::<Vec<_>>().join(" ");
    let ordinary_start = normalized
        .find("pub async fn finish(self)")
        .expect("production RestoreConfirmation::finish");
    let wrapper_start = normalized
        .find("pub async fn finish_with_current_snapshot_copy_interlock_for_test(")
        .expect("test-only current snapshot copy wrapper");
    let supplemental_wrapper = normalized
        .find("pub async fn finish_with_snapshot_binding_interlocks_for_test(")
        .expect("supplemental snapshot binding test wrapper");
    let private_start = normalized
        .find("async fn finish_internal(")
        .expect("shared private RestoreConfirmation finish implementation");
    assert!(
        ordinary_start < wrapper_start
            && wrapper_start < supplemental_wrapper
            && supplemental_wrapper < private_start,
        "ordinary finish, both test wrappers, and shared private internal must remain adjacent and ordered"
    );
    let ordinary = item_body(&normalized, "pub async fn finish(self)");
    let current_copy_wrapper = item_body(
        &normalized,
        "pub async fn finish_with_current_snapshot_copy_interlock_for_test(",
    );
    let snapshot_binding_wrapper = item_body(
        &normalized,
        "pub async fn finish_with_snapshot_binding_interlocks_for_test(",
    );
    assert_single_finish_tail_call(ordinary, "ordinary finish");
    assert_single_finish_tail_call(current_copy_wrapper, "current-copy test wrapper");
    assert_single_finish_tail_call(snapshot_binding_wrapper, "snapshot-binding test wrapper");
    assert!(
        current_copy_wrapper.contains("barriers") && snapshot_binding_wrapper.contains("barriers"),
        "each test wrapper may only thread its thin synchronization barriers into the shared finish"
    );

    let private_end = normalized[private_start..]
        .find("/// Execute one approved durability boundary")
        .map(|offset| private_start + offset)
        .expect("finish_internal must remain immediately before the crash harness");
    let private_finish = &normalized[private_start..private_end];
    assert!(
        private_finish.contains("current_copy_interlock:")
            && private_finish.contains("snapshot_binding_interlocks:"),
        "the shared private finish must own both thin interlocks without selecting a second I/O implementation"
    );
    for forbidden_interlock_branch in [
        "if current_copy_interlock.is_some()",
        "if let Some(current_copy_interlock)",
        "if snapshot_binding_interlocks.is_some()",
        "if let Some(snapshot_binding_interlocks)",
        "match current_copy_interlock",
        "match snapshot_binding_interlocks",
    ] {
        assert!(
            !private_finish.contains(forbidden_interlock_branch),
            "interlock presence may only pause the production path, never select its I/O: {forbidden_interlock_branch}"
        );
    }
    assert_eq!(
        private_finish.matches("current_copy_interlock").count(),
        2,
        "the current-copy interlock may occur only in the shared signature and ordinary copy call"
    );
    assert_eq!(
        private_finish
            .matches("snapshot_binding_interlocks")
            .count(),
        3,
        "the snapshot interlocks may occur only in the shared signature and two thin wait calls"
    );
    let checkpoint_complete = private_finish
        .find(".prepare_restore_for_safety(")
        .expect("shared finish must complete the normal checkpoint pre-safety phase");
    let pin_current = private_finish
        .find(".open_current_database_for_snapshot(")
        .expect("shared finish must pin current through the root capability");
    assert!(
        pin_current < checkpoint_complete,
        "the exact current leaf must be descriptor-pinned before SQLx checkpoint I/O begins"
    );

    let copy_call_start = private_finish
        .find("copy_owned_open_file( &mut held_current,")
        .expect("finish_internal must copy from its already-held current File");
    let copy_call_end = private_finish[copy_call_start..]
        .find(".await?")
        .map(|offset| copy_call_start + offset + ".await?".len())
        .expect("finish_internal must await the descriptor-bound copy helper");
    let copy_call = &private_finish[copy_call_start..copy_call_end];
    assert!(
        copy_call.contains("&canonical_current_identity")
            && copy_call.contains("snapshot_binding.directory()")
            && copy_call.contains("Path::new(DATABASE_FILENAME)")
            && copy_call.contains("current_copy_interlock"),
        "finish_internal must pass the held source plus the held snapshot directory and relative database name to the ordinary copy helper: {copy_call}"
    );
    let after_copy = &private_finish[copy_call_end..];
    for forbidden_reopen in [
        ".open(&current_database)",
        ".open(current_database)",
        "File::open(&current_database)",
        "File::open(current_database)",
        "fs::read(&current_database)",
        "fs::read(current_database)",
        "fs::copy(&current_database",
        "fs::copy(current_database",
        "read_verified_regular_file(&current_database",
        "copy_owned_regular_file( &current_database",
        "copy_owned_regular_file(&current_database",
    ] {
        assert!(
            !after_copy.contains(forbidden_reopen),
            "finish_internal must not reopen/read the current source path after the held-File copy returns: {forbidden_reopen}"
        );
    }

    let copy_start = normalized
        .find("fn copy_owned_open_file(")
        .expect("descriptor-bound safety database copy helper");
    let copy_signature_end = normalized[copy_start..]
        .find('{')
        .map(|offset| copy_start + offset)
        .expect("complete descriptor-bound copy helper signature");
    let copy_signature = &normalized[copy_start..copy_signature_end];
    assert!(
        copy_signature.contains("source_file: &mut File")
            && copy_signature.contains("expected_source_identity: &str")
            && copy_signature.contains("target_directory: &cap_std::fs::Dir")
            && copy_signature.contains("target_name: &Path")
            && copy_signature.contains("current_copy_interlock:")
            && !copy_signature.contains("snapshot_binding_interlocks:"),
        "the safety database copy helper must receive held source/target descriptors, a relative target name, and only the current-copy interlock: {copy_signature}"
    );
    let parameters_start = copy_signature
        .find('(')
        .map(|offset| offset + 1)
        .expect("copy helper parameters start");
    let parameters_end = copy_signature
        .rfind(')')
        .expect("copy helper parameters end");
    for parameter in copy_signature[parameters_start..parameters_end].split(',') {
        let Some((name, _parameter_type)) = parameter.split_once(':') else {
            continue;
        };
        if parameter.contains("Path") {
            let name = name.trim();
            assert!(
                name == "target_name" || name == "manifest_path",
                "copy_owned_open_file may receive relative target/manifest names, but no source, root, or absolute target path: {parameter}"
            );
        }
    }

    let copy_end = normalized[copy_start..]
        .find("fn copy_plugin_tree_with_descriptors(")
        .map(|offset| copy_start + offset)
        .expect("descriptor-bound database helper must remain separate from Plugin tree copying");
    let copy_helper = &normalized[copy_signature_end..copy_end];
    let source_barrier_arrive = copy_helper
        .find("current_copy_barriers[0].wait().await")
        .expect("held-File copy interlock arrival wait");
    let source_barrier_release = copy_helper
        .find("current_copy_barriers[1].wait().await")
        .expect("held-File copy interlock release wait");
    assert!(
        source_barrier_arrive < source_barrier_release,
        "the copy interlock must arrive before it waits for test release"
    );
    let before_barrier = &copy_helper[..source_barrier_arrive];
    let held_metadata = before_barrier
        .rfind("source_file .metadata()")
        .or_else(|| before_barrier.rfind("source_file.metadata()"))
        .expect("copy helper must take final held-File metadata before the interlock");
    let identity_match = before_barrier
        .rfind("stable_file_identity(&before)? != expected_source_identity")
        .expect("copy helper must compare final held-File metadata with canonical identity");
    assert!(
        held_metadata < identity_match,
        "the canonical identity comparison must use the final held-File metadata"
    );

    let first_seek = copy_helper
        .find("source_file .seek(")
        .or_else(|| copy_helper.find("source_file.seek("))
        .expect("descriptor-bound copy must explicitly seek its held File");
    let first_read = copy_helper
        .find("source_file .read(")
        .or_else(|| copy_helper.find("source_file.read("))
        .expect("descriptor-bound copy must read its held File");
    assert!(
        source_barrier_release < first_seek && source_barrier_release < first_read,
        "both interlock waits must occur after the final fd/canonical identity match and before the first held-File seek/read"
    );

    let target_create = copy_helper
        .find("target_directory .open_with(target_name")
        .or_else(|| copy_helper.find("target_directory.open_with(target_name"))
        .expect("database target must be created relative to the held snapshot directory");
    assert!(
        source_barrier_release < target_create,
        "the ordinary copy helper must create its target through the held snapshot directory"
    );

    let directory_wait_helper = item_body(
        &normalized,
        "async fn wait_snapshot_directory_binding_interlock(",
    );
    let post_safety_wait_helper =
        item_body(&normalized, "async fn wait_post_safety_binding_interlock(");
    for (helper, first, second, label) in [
        (directory_wait_helper, "[0]", "[1]", "snapshot directory"),
        (post_safety_wait_helper, "[2]", "[3]", "post-safety"),
    ] {
        assert!(
            helper.contains(first)
                && helper.contains(second)
                && helper.matches(".wait().await").count() == 2,
            "{label} interlock must be a two-wait synchronization-only helper: {helper}"
        );
        for forbidden in [
            "Path",
            "File",
            "cap_std::fs::Dir",
            "ImportError",
            "Result<",
            "return Err",
            "fs::",
            "open(",
            "read(",
            "write(",
        ] {
            assert!(
                !helper.contains(forbidden),
                "{label} interlock must not select, inspect, or mutate I/O: {forbidden}"
            );
        }
    }

    let initial_snapshot_match = private_finish
        .find("verify_snapshot_directory_binding(&snapshot_binding)")
        .or_else(|| private_finish.find("verify_snapshot_directory_binding( &snapshot_binding, )"))
        .expect("initial held snapshot-directory/canonical identity match");
    let snapshot_binding_match_count = private_finish
        .matches("verify_snapshot_directory_binding(&snapshot_binding)")
        .count()
        + private_finish
            .matches("verify_snapshot_directory_binding( &snapshot_binding, )")
            .count();
    assert_eq!(
        snapshot_binding_match_count, 2,
        "the descriptor-bound finish may compare the canonical snapshot directory only initially and once after the full held-directory build"
    );
    let directory_wait = private_finish
        .find("wait_snapshot_directory_binding_interlock( snapshot_binding_interlocks.as_ref(), ) .await")
        .or_else(|| {
            private_finish.find(
                "wait_snapshot_directory_binding_interlock(snapshot_binding_interlocks.as_ref()).await",
            )
        })
        .expect("thin snapshot-directory barrier 0/1 wait");
    assert!(
        initial_snapshot_match < directory_wait && directory_wait < copy_call_start,
        "barriers 0/1 must follow the initial binding match and immediately precede the ordinary dirfd-relative database copy"
    );
    let directory_release_to_copy = &private_finish[directory_wait..copy_call_start];
    for forbidden in [
        "verify_snapshot_directory_binding",
        "symlink_metadata",
        "fs::",
        "remove_tree_no_follow",
        "return Err",
    ] {
        assert!(
            !directory_release_to_copy.contains(forbidden),
            "barrier 1 must flow directly to the ordinary dirfd-relative copy: {forbidden}"
        );
    }

    let snapshot_finished = private_finish[copy_call_end..]
        .find("finish_restore_safety_snapshot( &snapshot_binding,")
        .or_else(|| {
            private_finish[copy_call_end..]
                .find("finish_restore_safety_snapshot(&snapshot_binding,")
        })
        .map(|offset| copy_call_end + offset)
        .expect("Plugin/manifest/sync completion on the same held snapshot binding");
    let snapshot_verified = private_finish[snapshot_finished..]
        .find("verify_restore_safety_snapshot(&snapshot_binding)")
        .or_else(|| {
            private_finish[snapshot_finished..]
                .find("verify_restore_safety_snapshot( &snapshot_binding, )")
        })
        .map(|offset| snapshot_finished + offset)
        .expect("complete held-directory safety snapshot verification");
    let final_current_match = private_finish[snapshot_verified..]
        .find(".verify_current_database_for_snapshot(")
        .map(|offset| snapshot_verified + offset)
        .expect("final held-current/canonical identity match");
    let owner_barrier_arrive = private_finish
        .find("wait_post_safety_binding_interlock( snapshot_binding_interlocks.as_ref(), ) .await")
        .or_else(|| {
            private_finish.find(
                "wait_post_safety_binding_interlock(snapshot_binding_interlocks.as_ref()).await",
            )
        })
        .expect("thin post-safety barrier 2/3 wait");
    let final_snapshot_match = private_finish[owner_barrier_arrive..]
        .find("verify_snapshot_directory_binding(&snapshot_binding)")
        .or_else(|| {
            private_finish[owner_barrier_arrive..]
                .find("verify_snapshot_directory_binding( &snapshot_binding, )")
        })
        .map(|offset| owner_barrier_arrive + offset)
        .expect("single final canonical snapshot-directory identity match");
    let apply_call = private_finish
        .find(".apply_restore_after_safety(")
        .expect("post-safety state-machine call");
    assert!(
        private_finish
            .contains("finish_or_cleanup_restore_safety_snapshot(snapshot_binding, safety_result)",)
            || private_finish.contains(
                "finish_or_cleanup_restore_safety_snapshot( snapshot_binding, safety_result, )",
            ),
        "the shared finish must transfer the exact binding into success/error cleanup"
    );
    assert!(
        copy_call_start < snapshot_finished
            && snapshot_finished < snapshot_verified
            && snapshot_verified < final_current_match
            && final_current_match < owner_barrier_arrive
            && owner_barrier_arrive < final_snapshot_match
            && final_snapshot_match < apply_call,
        "full DB+Plugin+manifest build and held verification must precede barriers 2/3; only then may the final canonical snapshot check precede apply"
    );
    let full_held_build = &private_finish[directory_wait..owner_barrier_arrive];
    assert!(
        !full_held_build.contains("verify_snapshot_directory_binding"),
        "barrier 1 through the complete held-directory build must not recheck the canonical snapshot path"
    );
    let release_to_apply = &private_finish[owner_barrier_arrive..apply_call];
    assert!(
        !release_to_apply.contains("verify_current_database_for_snapshot")
            && !release_to_apply.contains("symlink_metadata")
            && !release_to_apply.contains("open_current_database_for_snapshot")
            && (release_to_apply
                .matches("verify_snapshot_directory_binding(&snapshot_binding)")
                .count()
                + release_to_apply
                    .matches("verify_snapshot_directory_binding( &snapshot_binding, )")
                    .count()
                == 1),
        "barrier 3 may perform exactly the final snapshot binding check, then must enter apply without another source-path check"
    );
    let post_wait_to_final_match = &private_finish[owner_barrier_arrive..final_snapshot_match];
    for forbidden_short_circuit in ["?", "return ", "Err(", " if ", " if let ", " match "] {
        assert!(
            !post_wait_to_final_match.contains(forbidden_short_circuit),
            "barrier 3 must flow directly into the unique final canonical snapshot match: {forbidden_short_circuit}"
        );
    }
    let apply_invocation = &private_finish[apply_call..];
    assert!(
        apply_invocation.contains("RestoreOldDatabaseEvidence::Bound(bound_old_database)"),
        "the exact held File/identity/evidence object must cross into apply_after_safety"
    );

    let snapshot_binding = item_body(&normalized, "struct RestoreSafetySnapshotBinding");
    assert!(
        snapshot_binding.contains("directory: cap_std::fs::Dir")
            && snapshot_binding.contains("parent_directory: cap_std::fs::Dir")
            && snapshot_binding.contains("canonical_identity: String"),
        "one binding must retain the exact created snapshot directory, its parent, and canonical identity: {snapshot_binding}"
    );
    let finish_snapshot = item_body(&normalized, "fn finish_restore_safety_snapshot(");
    assert!(
        finish_snapshot.contains("snapshot_binding: &RestoreSafetySnapshotBinding")
            && finish_snapshot.contains("copy_plugin_tree_with_descriptors(")
            && finish_snapshot.contains("snapshot_binding.directory()")
            && finish_snapshot.contains("Path::new(\"plugins\")")
            && finish_snapshot.contains("write_staging_file_at(")
            && finish_snapshot.contains("Path::new(\"manifest.json\")")
            && (finish_snapshot.contains("sync_cap_directory(snapshot_binding.directory(),")
                || finish_snapshot.contains("sync_cap_directory( snapshot_binding.directory(),")),
        "Plugin copy, manifest publication, and sync must consume the same held snapshot binding and relative names: {finish_snapshot}"
    );
    let plugin_copy = item_body(&normalized, "fn copy_plugin_tree_with_descriptors(");
    assert!(
        plugin_copy.contains("target_directory: &cap_std::fs::Dir")
            && plugin_copy.contains("target_name: &Path")
            && !plugin_copy.contains("target: &Path"),
        "Plugin targets must be resolved only beneath the held snapshot directory: {plugin_copy}"
    );
    let manifest_write = item_body(&normalized, "fn write_staging_file_at(");
    assert!(
        manifest_write.contains("directory: &cap_std::fs::Dir")
            && manifest_write.contains("name: &Path")
            && !manifest_write.contains("target: &Path"),
        "manifest staging must be dirfd-relative: {manifest_write}"
    );
    let verify_snapshot = item_body(&normalized, "fn verify_restore_safety_snapshot(");
    assert!(
        verify_snapshot.contains("snapshot_binding: &RestoreSafetySnapshotBinding")
            && verify_snapshot.contains("snapshot_binding.directory()"),
        "snapshot verification must read the same held directory: {verify_snapshot}"
    );
    let cleanup = item_body(&normalized, "fn finish_or_cleanup_restore_safety_snapshot");
    assert!(
        cleanup.contains("snapshot_binding: RestoreSafetySnapshotBinding"),
        "cleanup must own the exact held snapshot binding: {cleanup}"
    );
    for (stage, body) in [
        ("finish", finish_snapshot),
        ("verify", verify_snapshot),
        ("cleanup", cleanup),
    ] {
        for forbidden_target_path in [
            "snapshot: &Path",
            "snapshot.join(",
            "snapshot_binding.canonical_path",
            "snapshot_binding.path()",
            "sync_directory(snapshot",
            "remove_tree_no_follow",
            "fs::remove_dir_all",
        ] {
            assert!(
                !body.contains(forbidden_target_path),
                "held snapshot {stage} must not reopen/create/remove through its canonical Path: {forbidden_target_path}"
            );
        }
    }

    let bound_old_database = item_body(&normalized, "struct BoundOldDatabase");
    assert!(
        bound_old_database.contains("held_file: File")
            && bound_old_database.contains("canonical_identity: String")
            && bound_old_database.contains("evidence: ControlledFileEvidence"),
        "the held File, final canonical identity, and copied evidence must cross arm/owner/apply as one by-value object: {bound_old_database}"
    );
    assert!(
        private_finish.contains("BoundOldDatabase {")
            && private_finish.contains("held_file: held_current")
            && private_finish.contains("canonical_identity: canonical_current_identity")
            && private_finish.contains("evidence: old_database_evidence")
            && apply_invocation.contains("RestoreOldDatabaseEvidence::Bound(bound_old_database)"),
        "finish_internal must transfer the exact held source object into apply instead of separating or reopening it"
    );

    let apply_start = normalized
        .find("async fn apply_restore_after_safety(")
        .expect("post-safety restore state machine");
    let apply_end = normalized[apply_start..]
        .find("/// Advance the real switch/retirement state machine")
        .map(|offset| apply_start + offset)
        .expect("post-safety state machine boundary");
    let apply_after_safety = &normalized[apply_start..apply_end];
    assert!(
        apply_after_safety.contains("old_database_source: RestoreOldDatabaseEvidence")
            && apply_after_safety.contains("RestoreOldDatabaseEvidence::Bound(bound_old_database)")
            && apply_after_safety.contains("bound.evidence.clone()"),
        "post-safety apply must retain the bound File/evidence object through owner publication"
    );
    let arm_assignment = apply_after_safety
        .find("instance_manifest.ownership_state = \"armed\".into()")
        .expect("manifest armed assignment");
    let first_arm = apply_after_safety[arm_assignment..]
        .find("publish_instance_manifest(")
        .map(|offset| arm_assignment + offset)
        .expect("first manifest arm publication");
    let prepared_phase = apply_after_safety
        .find("phase: \"prepared\".into()")
        .expect("prepared owner construction");
    let prepared_owner = apply_after_safety[prepared_phase..]
        .find("publish_restore_owner(")
        .map(|offset| prepared_phase + offset)
        .expect("prepared owner publication");
    let applying_phase = apply_after_safety
        .find("owner.phase = \"applying\".into()")
        .expect("owner applying transition");
    let applying_owner = apply_after_safety[applying_phase..]
        .find("publish_restore_owner(")
        .map(|offset| applying_phase + offset)
        .expect("durable applying owner publication");
    let bound_move = apply_after_safety
        .find("move_current_database_with_bound_identity(")
        .expect("identity-bound current move");
    assert!(
        arm_assignment < first_arm
            && first_arm < prepared_phase
            && prepared_phase < prepared_owner
            && prepared_owner < applying_phase
            && applying_phase < applying_owner
            && applying_owner < bound_move,
        "backup-package.md 67-68 require armed -> prepared -> applying to be durable before the first database move"
    );
    let bound_move_call_end = apply_after_safety[bound_move..]
        .find("?;")
        .map(|offset| bound_move + offset)
        .expect("complete identity-bound move call");
    let bound_move_call = &apply_after_safety[bound_move..bound_move_call_end];
    assert!(
        bound_move_call.contains("root_dir")
            && bound_move_call.contains("live_dir")
            && bound_move_call.contains("&mut bound_old_database"),
        "safe move must consume the opened root/live parents plus the same by-value held File/evidence object"
    );
    let owner_initializer =
        &apply_after_safety[prepared_phase.saturating_sub(1500)..prepared_owner];
    assert!(
        owner_initializer.contains("old_database:")
            && owner_initializer.contains("bound_old_database")
            && owner_initializer.contains("evidence.clone()"),
        "RestoreOwner.old_database must directly clone the same bound evidence"
    );
    let applying_publish_end = apply_after_safety[applying_owner..]
        .find("?;")
        .map(|offset| applying_owner + offset + 2)
        .expect("durable applying owner call boundary");
    let applying_to_move = &apply_after_safety[applying_publish_end..bound_move];
    for forbidden_pre_move_check in [
        "verify_current_database_for_snapshot",
        "optional_controlled_file_evidence( &current_database",
        "controlled_file_evidence( &current_database",
        "symlink_metadata(&current_database)",
        "File::open(&current_database)",
        "current_database.exists()",
    ] {
        assert!(
            !applying_to_move.contains(forbidden_pre_move_check),
            "durable applying must flow directly to the first owner-protected move, never a path-only precheck: {forbidden_pre_move_check}"
        );
    }
    assert!(
        !apply_after_safety.contains("publish_no_replace(&current_database, &old_database)"),
        "the post-safety state machine must not retain a second unbound current-path move"
    );

    let safe_move = item_body(&normalized, "fn move_current_database_with_bound_identity(");
    assert!(
        safe_move.contains("root_dir: &cap_std::fs::Dir")
            && safe_move.contains("live_dir: &cap_std::fs::Dir")
            && safe_move.contains("bound_old_database: &mut BoundOldDatabase")
            && !safe_move.contains("current_database: &Path")
            && !safe_move.contains("old_database: &Path"),
        "safe move must accept only opened parents, fixed basenames, and the held evidence object: {safe_move}"
    );
    let first_move = safe_move
        .find(
            "publish_no_replace_at( root_dir, Path::new(DATABASE_FILENAME), live_dir, Path::new(\"old-datasources.db\"), )",
        )
        .expect("root-dirfd-relative first database move after durable applying owner");
    let sync_root_after_move = safe_move[first_move..]
        .find("sync_cap_directory( root_dir,")
        .or_else(|| safe_move[first_move..].find("sync_cap_directory(root_dir,"))
        .map(|offset| first_move + offset)
        .expect("root parent sync after first move");
    let sync_live_after_move = safe_move[sync_root_after_move..]
        .find("sync_cap_directory( live_dir,")
        .or_else(|| safe_move[sync_root_after_move..].find("sync_cap_directory(live_dir,"))
        .map(|offset| sync_root_after_move + offset)
        .expect("live parent sync after first move");
    let moved_metadata = safe_move[sync_live_after_move..]
        .find("live_dir.symlink_metadata(Path::new(\"old-datasources.db\"))")
        .map(|offset| sync_live_after_move + offset)
        .expect("live-dirfd-relative moved inode metadata");
    let moved_identity = safe_move[moved_metadata..]
        .find("stable_file_identity")
        .map(|offset| moved_metadata + offset)
        .expect("post-move stable identity inspection");
    let mismatch = safe_move
        .find("!= bound_old_database.evidence.identity")
        .expect("moved inode versus bound evidence mismatch branch");
    let rollback = safe_move[mismatch..]
        .find(
            "publish_no_replace_at( live_dir, Path::new(\"old-datasources.db\"), root_dir, Path::new(DATABASE_FILENAME), )",
        )
        .map(|offset| mismatch + offset)
        .expect("dirfd-relative rollback of mismatched inode");
    let sync_root_after_rollback = safe_move[rollback..]
        .find("sync_cap_directory( root_dir,")
        .or_else(|| safe_move[rollback..].find("sync_cap_directory(root_dir,"))
        .map(|offset| rollback + offset)
        .expect("root parent sync after rollback");
    let sync_live_after_rollback = safe_move[sync_root_after_rollback..]
        .find("sync_cap_directory( live_dir,")
        .or_else(|| safe_move[sync_root_after_rollback..].find("sync_cap_directory(live_dir,"))
        .map(|offset| sync_root_after_rollback + offset)
        .expect("live parent sync after rollback");
    let restored_metadata = safe_move[sync_live_after_rollback..]
        .find("root_dir.symlink_metadata(Path::new(DATABASE_FILENAME))")
        .map(|offset| sync_live_after_rollback + offset)
        .expect("root-dirfd-relative rollback identity verification");
    let restored_identity_match = safe_move[restored_metadata..]
        .find("restored_identity != moved_identity")
        .map(|offset| restored_metadata + offset)
        .expect("rollback must verify the exact mismatched inode returned canonical");
    let rejection = safe_move[restored_identity_match..]
        .find("current_database_identity_changed()")
        .map(|offset| restored_identity_match + offset)
        .expect("exact current identity rejection after proven rollback");
    assert!(
        first_move < sync_root_after_move
            && sync_root_after_move < sync_live_after_move
            && sync_live_after_move < moved_metadata
            && moved_metadata < moved_identity
            && moved_identity < mismatch
            && mismatch < rollback
            && rollback < sync_root_after_rollback
            && sync_root_after_rollback < sync_live_after_rollback
            && sync_live_after_rollback < restored_metadata
            && restored_metadata < restored_identity_match
            && restored_identity_match < rejection,
        "dirfd move/sync/post-move compare/dirfd rollback/sync/exact rollback verification must precede rejection"
    );
    assert!(
        safe_move.contains("bound_old_database.held_file.metadata()")
            && !safe_move.contains("publish_no_replace(")
            && !safe_move.contains("libc::AT_FDCWD")
            && !safe_move[first_move..mismatch].contains("File::open")
            && !safe_move[first_move..mismatch].contains(".read(")
            && !safe_move[first_move..mismatch].contains("controlled_file_evidence"),
        "post-move mismatch detection must retain the held File and inspect moved metadata without ambient paths or competitor data I/O"
    );
    let publish_at = item_body(&normalized, "fn publish_no_replace_at(");
    assert!(
        publish_at.contains("from_dir: &cap_std::fs::Dir")
            && publish_at.contains("to_dir: &cap_std::fs::Dir")
            && publish_at.contains("from: &Path")
            && publish_at.contains("to: &Path")
            && publish_at.contains("libc::renameat2(")
            && publish_at.contains("from_dir.as_raw_fd()")
            && publish_at.contains("to_dir.as_raw_fd()")
            && publish_at.contains("libc::RENAME_NOREPLACE")
            && !publish_at.contains("libc::AT_FDCWD")
            && !publish_at.contains("fs::rename")
            && !publish_at.contains(".rename("),
        "Linux no-replace publication must use both opened parent dirfds with renameat2(RENAME_NOREPLACE): {publish_at}"
    );
}

#[cfg(target_os = "linux")]
fn assert_offline_apply_return_proof_source_contract() {
    fn item_body<'a>(normalized: &'a str, signature: &str) -> &'a str {
        let start = normalized
            .find(signature)
            .unwrap_or_else(|| panic!("missing production source item: {signature}"));
        let body_start = normalized[start..]
            .find('{')
            .map(|offset| start + offset)
            .unwrap_or_else(|| panic!("missing body for production source item: {signature}"));
        let mut depth = 0_usize;
        for (offset, character) in normalized[body_start..].char_indices() {
            match character {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return &normalized[start..body_start + offset + character.len_utf8()];
                    }
                }
                _ => {}
            }
        }
        panic!("unterminated production source item: {signature}");
    }

    fn assert_apply_tail_call(item: &str, arguments: &str, label: &str) {
        let body_start = item
            .find('{')
            .map(|offset| offset + 1)
            .expect("offline apply wrapper body");
        let call_start = item
            .find("self.apply_restore_internal(")
            .expect("shared offline apply tail call");
        assert!(
            item[body_start..call_start].trim().is_empty(),
            "{label} must do no work before its shared offline apply tail-call: {item}"
        );
        assert_eq!(
            item.matches("self.apply_restore_internal(").count(),
            1,
            "{label} must call the shared offline apply internal exactly once: {item}"
        );
        assert!(
            item.contains(arguments),
            "{label} must preserve the exact crash/interlock argument tuple: {item}"
        );
        assert_eq!(
            item.matches(".await").count(),
            1,
            "{label} must contain only the shared offline apply await: {item}"
        );
        for forbidden in [" if ", " if let ", " match ", " loop ", " while ", " for "] {
            assert!(
                !item.contains(forbidden),
                "{label} must not select a test-only apply/finalization path: {forbidden} in {item}"
            );
        }
        let await_end = item
            .rfind(".await")
            .map(|offset| offset + ".await".len())
            .expect("single offline apply await");
        assert_eq!(
            item[await_end..].trim(),
            "}",
            "{label} must tail-call the shared offline apply internal with no fallback work"
        );
    }

    let source = include_str!("../src/datasource/backup.rs");
    let normalized = source.split_whitespace().collect::<Vec<_>>().join(" ");
    let ordinary_start = normalized
        .find("pub async fn apply_restore(&self, plan: &RestorePlan)")
        .expect("ordinary offline apply entry");
    let wrapper_start = normalized
        .find("pub async fn apply_restore_with_safety_return_interlock_for_test(")
        .expect("test-only offline return-proof wrapper");
    let internal_start = normalized
        .find("async fn apply_restore_internal(")
        .expect("shared offline apply internal");
    assert!(
        ordinary_start < wrapper_start && wrapper_start < internal_start,
        "ordinary apply and the synchronization-only wrapper must remain adjacent to one shared internal"
    );
    let ordinary = item_body(
        &normalized,
        "pub async fn apply_restore(&self, plan: &RestorePlan)",
    );
    let wrapper = item_body(
        &normalized,
        "pub async fn apply_restore_with_safety_return_interlock_for_test(",
    );
    assert_apply_tail_call(
        ordinary,
        "self.apply_restore_internal(plan, None, None)",
        "ordinary offline apply",
    );
    assert_apply_tail_call(
        wrapper,
        "self.apply_restore_internal(plan, None, Some(barriers))",
        "offline return-proof test wrapper",
    );

    let internal = item_body(&normalized, "async fn apply_restore_internal(");
    assert!(
        internal.contains("safety_return_interlock: Option<[Arc<tokio::sync::Barrier>; 2]>",)
            && !internal.contains("keep_frozen:"),
        "one shared internal must own only crash injection plus the thin return-proof interlock: {internal}"
    );
    assert_eq!(
        internal.matches(".apply_restore_after_safety(").count(),
        1,
        "offline apply must execute the real restore state machine exactly once"
    );
    assert_eq!(
        internal.matches("create_restore_safety_snapshot(").count(),
        1,
        "offline apply must create exactly one held safety proof"
    );
    assert!(
        internal.contains("let verified_safety_snapshot = create_restore_safety_snapshot(",),
        "the unique safety snapshot result must remain the proof object used through return"
    );
    for forbidden_interlock_branch in [
        "if safety_return_interlock.is_some()",
        "if let Some(safety_return_interlock)",
        "match safety_return_interlock",
    ] {
        assert!(
            !internal.contains(forbidden_interlock_branch),
            "interlock presence may only pause the ordinary apply path: {forbidden_interlock_branch}"
        );
    }
    assert_eq!(
        internal
            .matches("wait_restore_safety_return_interlock(")
            .count(),
        1,
        "the shared internal must cross exactly one return-proof wait"
    );

    let saved_gate = internal
        .find("let write_gate_database = prepared.write_gate_database.clone()")
        .expect("save the exact frozen database key before moving prepared state");
    let outcome_binding = internal
        .find("let apply_outcome =")
        .expect("save the unique real apply outcome");
    let apply_call = internal
        .find(".apply_restore_after_safety(")
        .expect("unique real restore apply call");
    let apply_end = internal[apply_call..]
        .find(".await;")
        .map(|offset| apply_call + offset + ".await;".len())
        .expect("await and save the real apply outcome");
    let apply_invocation = &internal[apply_call..apply_end];
    let plan_argument = apply_invocation
        .find("plan")
        .expect("restore plan argument");
    let crash_argument = apply_invocation
        .find("crash_at")
        .expect("restore crash argument");
    let frozen_argument = apply_invocation[crash_argument..]
        .find("true")
        .map(|offset| crash_argument + offset)
        .expect("offline state machine must retain the write gate");
    let prepared_argument = apply_invocation
        .find("prepared")
        .expect("prepared restore state argument");
    let proof_argument = apply_invocation
        .find("&verified_safety_snapshot")
        .expect("same held safety proof argument");
    assert!(
        saved_gate < outcome_binding
            && outcome_binding < apply_call
            && plan_argument < crash_argument
            && crash_argument < frozen_argument
            && frozen_argument < prepared_argument
            && prepared_argument < proof_argument
            && !apply_invocation.contains("false"),
        "offline apply must run its one real outcome with keep_frozen=true and the held safety proof: {apply_invocation}"
    );

    let wait_start = internal[apply_end..]
        .find("wait_restore_safety_return_interlock(")
        .map(|offset| apply_end + offset)
        .expect("post-outcome return-proof wait");
    let wait_end = internal[wait_start..]
        .find(".await;")
        .map(|offset| wait_start + offset + ".await;".len())
        .expect("complete return-proof wait");
    let wait_call = &internal[wait_start..wait_end];
    assert!(
        wait_call.contains("safety_return_interlock.as_ref()"),
        "the seam may only forward the thin optional barriers: {wait_call}"
    );
    let revalidate = internal[wait_end..]
        .find("let safety_binding = verified_safety_snapshot.revalidate()")
        .map(|offset| wait_end + offset)
        .expect("return-time revalidation of the same held safety proof");
    let classify = internal[revalidate..]
        .find("match (apply_outcome, safety_binding)")
        .map(|offset| revalidate + offset)
        .expect("classify the saved apply outcome with its return-time proof");
    assert!(
        apply_end < wait_start
            && wait_start < wait_end
            && wait_end < revalidate
            && revalidate < classify,
        "the real outcome must finish frozen, then wait, revalidate the same held proof, and only then classify"
    );
    let wait_to_proof = &internal[wait_end..revalidate];
    for forbidden_short_circuit in ["?", "return ", "Err(", " if ", " if let ", " match "] {
        assert!(
            !wait_to_proof.contains(forbidden_short_circuit),
            "barrier release must flow directly into the held proof revalidation: {forbidden_short_circuit}"
        );
    }

    let success_arm = internal[classify..]
        .find("(Ok(mut recovery), Ok(())) => {")
        .map(|offset| classify + offset)
        .expect("only real-success plus valid-proof may finalize");
    let invalid_success_arm = internal[success_arm..]
        .find("(Ok(_), Err(error)) => Err(error)")
        .map(|offset| success_arm + offset)
        .expect("successful apply with invalid proof must fail closed");
    let error_arm = internal[invalid_success_arm..]
        .find("(Err(error), _) => Err(error)")
        .map(|offset| invalid_success_arm + offset)
        .expect("every real apply error, including InjectedCrash, must remain fail-closed");
    let valid_success = &internal[success_arm..invalid_success_arm];
    assert_eq!(
        internal.matches(".remove(&write_gate_database)").count(),
        1,
        "one valid-success arm must be the only write-gate finalizer"
    );
    let gate_open = valid_success
        .find(".remove(&write_gate_database)")
        .expect("valid success removes the exact frozen gate key");
    let may_open = valid_success
        .find("recovery.store_may_open = true")
        .expect("valid success marks Store reopen safe");
    let write_open = valid_success
        .find("recovery.write_gate_open = true")
        .expect("valid success reports the gate open");
    let success_return = valid_success
        .rfind("Ok(recovery)")
        .expect("valid success returns the finalized real outcome");
    assert!(
        may_open < write_open && write_open < gate_open && gate_open < success_return,
        "valid proof must preconstruct its success result, then open the gate as the last action before return"
    );
    for forbidden_after_gate in [
        "?",
        ".await",
        "fs::",
        ".is_file()",
        ".is_dir()",
        ".revalidate()",
        ".open(",
        ".read(",
    ] {
        assert!(
            !valid_success[gate_open..].contains(forbidden_after_gate),
            "once the gate opens, success may perform no fallible/I/O/proof work: {forbidden_after_gate}"
        );
    }
    let gate_open_absolute = success_arm + gate_open;
    assert!(
        success_arm < gate_open_absolute
            && gate_open_absolute < invalid_success_arm
            && invalid_success_arm < error_arm,
        "invalid success and every Error/InjectedCrash path must remain outside the unique gate-opening arm"
    );

    let proof_revalidate = item_body(
        &normalized,
        "fn revalidate(&self) -> Result<(), ImportError>",
    );
    let first_binding_match = proof_revalidate
        .find("verify_snapshot_directory_binding(&self.binding)")
        .expect("initial held/canonical safety binding match");
    let fresh_evidence = proof_revalidate
        .find("controlled_tree_evidence_at(self.binding.directory()")
        .expect("fresh evidence must be read through the held safety directory");
    let evidence_match = proof_revalidate
        .find("fresh_evidence != self.evidence")
        .expect("fresh held-tree evidence must match the original proof");
    let final_binding_match = proof_revalidate
        .rfind("verify_snapshot_directory_binding(&self.binding)")
        .expect("final held/canonical safety binding match");
    assert_eq!(
        proof_revalidate
            .matches("verify_snapshot_directory_binding(&self.binding)")
            .count(),
        2,
        "return proof must bracket held-tree evidence with exactly two binding checks"
    );
    assert!(
        first_binding_match < fresh_evidence
            && fresh_evidence < evidence_match
            && evidence_match < final_binding_match,
        "return proof must check binding, read/compare through the held directory, then check binding again"
    );
    for forbidden_path_reopen in [
        "self.binding.canonical_path",
        "open_restore_safety_snapshot_binding",
        "verify_restore_safety_snapshot_path",
        "fs::",
        "File::open",
    ] {
        assert!(
            !proof_revalidate.contains(forbidden_path_reopen),
            "return proof must never rebuild itself from the ambient snapshot path: {forbidden_path_reopen}"
        );
    }

    let wait_helper = item_body(
        &normalized,
        "async fn wait_restore_safety_return_interlock(",
    );
    assert!(
        wait_helper.contains("[0]")
            && wait_helper.contains("[1]")
            && wait_helper.matches(".wait().await").count() == 2,
        "return-proof interlock must be a two-wait synchronization-only helper: {wait_helper}"
    );
    for forbidden in [
        "Path",
        "File",
        "ImportError",
        "Result<",
        "return Err",
        "fs::",
        "open(",
        "read(",
        "write(",
    ] {
        assert!(
            !wait_helper.contains(forbidden),
            "return-proof interlock must never select, inspect, or mutate production state: {forbidden}"
        );
    }

    let crash_harness = item_body(
        &normalized,
        "pub async fn apply_with_crash( &self, plan: &RestorePlan, crash_point: BackupCrashPoint, )",
    );
    assert_eq!(
        crash_harness.matches(".apply_restore_internal(").count(),
        1,
        "the crash harness must execute the same shared offline internal exactly once"
    );
    assert!(
        crash_harness.contains("plan, Some(crash_point), None")
            && crash_harness.contains(
                "Err(ImportError::InjectedCrash(actual)) if actual == crash_point.as_str()",
            ),
        "the shared internal must preserve the exact InjectedCrash contract: {crash_harness}"
    );
}

fn sidecar_file_identity(path: &Path) -> String {
    let metadata = fs::metadata(path).expect("read sidecar identity");
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;

        format!("unix:{}:{}", metadata.dev(), metadata.ino())
    }
    #[cfg(not(unix))]
    {
        let modified = metadata
            .modified()
            .expect("sidecar modified time")
            .duration_since(UNIX_EPOCH)
            .expect("sidecar modified after epoch")
            .as_nanos();
        format!("portable:{}:{modified}", metadata.len())
    }
}

fn sidecar_cleanup_token(db_id: &str) -> String {
    let bytes = db_id.as_bytes();
    let mut digest = Sha256::new();
    digest.update(b"hivegui-sidecar-cleanup-v1");
    digest.update([0]);
    digest.update((bytes.len() as u32).to_be_bytes());
    digest.update(bytes);
    hex::encode(digest.finalize())
}

struct SidecarCleanupJournalFixture<'a> {
    artifact: &'a str,
    canonical_name: &'a str,
    sidecar_identity: &'a str,
    sidecar_size: u64,
    sidecar_sha256: &'a str,
    cleanup_operation_id: Uuid,
    state: &'a str,
}

fn write_sidecar_cleanup_journal(
    root: &Path,
    database: &Path,
    fixture: SidecarCleanupJournalFixture<'_>,
) -> (std::path::PathBuf, std::path::PathBuf) {
    let SidecarCleanupJournalFixture {
        artifact,
        canonical_name,
        sidecar_identity,
        sidecar_size,
        sidecar_sha256,
        cleanup_operation_id,
        state,
    } = fixture;
    let token = sidecar_cleanup_token("current");
    let journal = root.join(format!(
        ".hivegui-sidecar-cleanup-v1-{token}-{artifact}.json"
    ));
    let quarantine_name =
        format!(".hivegui-sidecar-quarantine-v1-{cleanup_operation_id}-{artifact}");
    let quarantine = root.join(&quarantine_name);
    let record = serde_json::json!({
        "schema_version": 1,
        "cleanup_operation_id": cleanup_operation_id,
        "db_id": "current",
        "database_identity": sidecar_file_identity(database),
        "artifact": artifact,
        "canonical_name": canonical_name,
        "expected_identity": sidecar_identity,
        "expected_size_bytes": sidecar_size,
        "expected_sha256": sidecar_sha256,
        "quarantine_name": quarantine_name,
        "state": state,
    });
    fs::write(
        &journal,
        serde_json::to_vec(&record).expect("encode cleanup journal"),
    )
    .expect("write cleanup journal fixture");
    (journal, quarantine)
}

fn write_authenticated_tar_fixture(path: &Path, passphrase: &str, entries: &[(&str, &[u8])]) {
    use age::Encryptor;
    use age::secrecy::SecretString;
    use flate2::write::GzEncoder;

    let output = fs::File::create(path).expect("create authenticated fixture");
    let encryptor =
        Encryptor::with_user_passphrase(SecretString::new(passphrase.to_string().into_boxed_str()));
    let age = encryptor.wrap_output(output).expect("wrap age output");
    let gzip = GzEncoder::new(age, flate2::Compression::default());
    let mut tar = tar::Builder::new(gzip);
    for (name, bytes) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o600);
        header.set_path(name).expect("fixture path");
        header.set_cksum();
        tar.append(&header, *bytes).expect("append fixture entry");
    }
    let gzip = tar.into_inner().expect("finish fixture tar");
    let age = gzip.finish().expect("finish fixture gzip");
    age.finish().expect("finish fixture age");
}

fn write_authenticated_entry_map(
    path: &Path,
    passphrase: &str,
    entries: &BTreeMap<String, Vec<u8>>,
) {
    let borrowed = entries
        .iter()
        .map(|(name, bytes)| (name.as_str(), bytes.as_slice()))
        .collect::<Vec<_>>();
    write_authenticated_tar_fixture(path, passphrase, &borrowed);
}

fn rewrite_portable_entity_archive(
    source: &Path,
    target: &Path,
    passphrase: &str,
    format_version: u64,
    mut rewrite: impl FnMut(&str, &mut serde_json::Value),
) {
    let mut entries = read_authenticated_tar_entries(source, passphrase);
    let mut manifest: serde_json::Value =
        serde_json::from_slice(entries.get("manifest.json").expect("source manifest"))
            .expect("parse source manifest");
    manifest["format_version"] = serde_json::json!(format_version);
    for descriptor in manifest["entity_files"]
        .as_array_mut()
        .expect("entity descriptors")
    {
        let name = descriptor["name"]
            .as_str()
            .expect("entity name")
            .to_string();
        let path = descriptor["path"]
            .as_str()
            .expect("entity path")
            .to_string();
        let mut body: serde_json::Value =
            serde_json::from_slice(entries.get(&path).expect("entity body"))
                .expect("parse entity body");
        rewrite(&name, &mut body);
        let bytes = serde_json::to_vec(&body).expect("encode rewritten entity body");
        descriptor["count"] = serde_json::json!(
            body["rows"]
                .as_array()
                .expect("rewritten entity rows")
                .len()
        );
        descriptor["sha256"] = serde_json::json!(hex::encode(Sha256::digest(&bytes)));
        entries.insert(path, bytes);
    }
    entries.insert(
        "manifest.json".into(),
        serde_json::to_vec(&manifest).expect("encode rewritten manifest"),
    );
    write_authenticated_entry_map(target, passphrase, &entries);
}

fn read_authenticated_tar_entries(path: &Path, passphrase: &str) -> BTreeMap<String, Vec<u8>> {
    use age::Decryptor;
    use age::secrecy::SecretString;
    use flate2::read::GzDecoder;

    let decryptor = Decryptor::new(fs::File::open(path).expect("open authenticated archive"))
        .expect("parse age envelope");
    let identity =
        age::scrypt::Identity::new(SecretString::new(passphrase.to_string().into_boxed_str()));
    let reader = decryptor
        .decrypt(std::iter::once(&identity as &dyn age::Identity))
        .expect("authenticate age envelope");
    let mut archive = tar::Archive::new(GzDecoder::new(reader));
    let mut entries = BTreeMap::new();
    for entry in archive.entries().expect("read tar entries") {
        let mut entry = entry.expect("read tar entry");
        let path = entry
            .path()
            .expect("read UTF-8 archive path")
            .to_str()
            .expect("archive paths are UTF-8")
            .to_string();
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).expect("read archive entry");
        assert!(
            entries.insert(path, bytes).is_none(),
            "archive paths are unique"
        );
    }
    entries
}

async fn seed_complete_portable_fixture(
    workspace: &TestWorkspace,
    store: &Store,
) -> BTreeMap<&'static str, Vec<u8>> {
    const AT: &str = "2026-08-26T00:00:00Z";
    let device_key: [u8; 32] = fs::read(
        workspace
            .database_path()
            .parent()
            .expect("source database parent")
            .join("encryption.key"),
    )
    .expect("read source device key")
    .try_into()
    .expect("source device key is exactly 32 bytes");
    let crypto = Crypto::new(&device_key);
    let secrets = BTreeMap::from([
        ("datasource", b"portable-datasource-secret".to_vec()),
        ("provider", b"portable-provider-token".to_vec()),
        ("session", b"portable-session-title".to_vec()),
        ("message", b"portable-message-content".to_vec()),
        ("tool_calls", br#"[{"name":"portable-tool"}]"#.to_vec()),
        ("execution", br#"{"step":"portable-state"}"#.to_vec()),
    ]);

    store
        .create(
            "portable-complete-datasource",
            "127.0.0.1",
            3307,
            "portable-user",
            secrets["datasource"].as_slice(),
        )
        .await
        .expect("seed portable DataSource");

    let category_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO categories (parent_id,name,slug,description,created_at,updated_at) \
         VALUES (NULL,'Portable Root','portable-root','root category',?,?) RETURNING id",
    )
    .bind(AT)
    .bind(AT)
    .fetch_one(store.pool())
    .await
    .expect("seed root Category");
    sqlx::query(
        "INSERT INTO categories (parent_id,name,slug,description,created_at,updated_at) \
         VALUES (?,'Portable Child','portable-child','child category',?,?)",
    )
    .bind(category_id)
    .bind(AT)
    .bind(AT)
    .execute(store.pool())
    .await
    .expect("seed child Category relation");
    sqlx::query(
        "INSERT INTO capabilities (name,description,is_dangerous,category_id,normalized_name,created_at) \
         VALUES ('backup.full.capability','portable capability',1,?,'backup.full.capability',?)",
    )
    .bind(category_id)
    .bind(AT)
    .execute(store.pool())
    .await
    .expect("seed Capability");
    sqlx::query(
        "INSERT INTO global_configs (name,key,type,data,created_at,updated_at) \
         VALUES ('Portable Config','portable.complete','json','{\"enabled\":true}',?,?)",
    )
    .bind(AT)
    .bind(AT)
    .execute(store.pool())
    .await
    .expect("seed GlobalConfig");
    let provider_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO llm_providers (name,category,base_url,token_env,token_encrypted,created_at,updated_at) \
         VALUES ('portable-provider','openai','https://portable.invalid/v1','',?,?,?) RETURNING id",
    )
    .bind(crypto.encrypt(&secrets["provider"]).expect("encrypt Provider token"))
    .bind(AT)
    .bind(AT)
    .fetch_one(store.pool())
    .await
    .expect("seed LlmProvider");
    let preset_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO llm_presets (name,description,is_default,max_tokens,temperature,created_at,updated_at) \
         VALUES ('portable-preset','portable preset',0,4097,0.25,?,?) RETURNING id",
    )
    .bind(AT)
    .bind(AT)
    .fetch_one(store.pool())
    .await
    .expect("seed LlmPreset");
    sqlx::query(
        "INSERT INTO models (name,preset_id,provider_id,priority,created_at,updated_at) \
         VALUES ('portable-model',?,?,7,?,?)",
    )
    .bind(preset_id)
    .bind(provider_id)
    .bind(AT)
    .bind(AT)
    .execute(store.pool())
    .await
    .expect("seed Model relation");
    sqlx::query(
        "INSERT INTO tags (name,color,normalized_name,created_at,updated_at) \
         VALUES ('Portable Tag','#123456','portable tag',?,?)",
    )
    .bind(AT)
    .bind(AT)
    .execute(store.pool())
    .await
    .expect("seed Tag");

    let wasm_fixtures = [
        ("portable-one", b"\0asmT129-complete-one".as_slice()),
        ("portable-two", b"\0asmT129-complete-two".as_slice()),
    ];
    let mut plugin_ids = Vec::new();
    for (index, (identifier, wasm)) in wasm_fixtures.into_iter().enumerate() {
        let key = format!("complete/{identifier}/plugin.wasm");
        let path = workspace.plugin_root().join(&key);
        fs::create_dir_all(path.parent().expect("Plugin artifact parent"))
            .expect("create Plugin artifact parent");
        fs::write(&path, wasm).expect("write managed Plugin artifact");
        let plugin_id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO plugins \
             (identifier,name,description,manifest,version,author,repository_url,s3_key,sha256,size_bytes,runtime,category_id,capabilities,resource_limits,row_revision,created_at,updated_at,deleted_at) \
             VALUES (?,?,?,?,'1.2.3','portable-author','https://portable.invalid/repo',?,?,?,'wasm32',?,'[\"backup.full.capability\"]','{\"timeout_ms\":30000,\"memory_mib\":128,\"max_output_bytes\":1048576}',?,?,?,NULL) \
             RETURNING id",
        )
        .bind(identifier)
        .bind(format!("Portable Plugin {index}"))
        .bind(format!("portable Plugin description {index}"))
        .bind(format!("{{\"name\":\"{identifier}\",\"abi\":\"hive-extism/v1\"}}"))
        .bind(&key)
        .bind(hex::encode(Sha256::digest(wasm)))
        .bind(i64::try_from(wasm.len()).expect("WASM fixture size"))
        .bind(category_id)
        .bind(i64::try_from(index + 1).expect("row revision"))
        .bind(AT)
        .bind(AT)
        .fetch_one(store.pool())
        .await
        .expect("seed Plugin row");
        plugin_ids.push(plugin_id);
    }

    let function_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO functions \
         (identifier,name,description,kind,input_schema,output_schema,plugin_id,plugin_export,category_id,required_capabilities,created_at,updated_at) \
         VALUES ('portable-custom','Portable Custom','portable function','custom','{\"type\":\"object\"}','{\"type\":\"object\"}',?,'echo',?,'[\"backup.full.capability\"]',?,?) RETURNING id",
    )
    .bind(plugin_ids[0])
    .bind(category_id)
    .bind(AT)
    .bind(AT)
    .fetch_one(store.pool())
    .await
    .expect("seed Function relation");
    let workflow_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO workflows \
         (identifier,name,description,timeout_ms,category_id,input_schema,start_description,output_schema,required_capabilities,created_at,updated_at) \
         VALUES ('portable-workflow','Portable Workflow','portable workflow',45678,?,'{\"type\":\"object\"}','portable start','{\"type\":\"object\"}','[\"backup.full.capability\"]',?,?) RETURNING id",
    )
    .bind(category_id)
    .bind(AT)
    .bind(AT)
    .fetch_one(store.pool())
    .await
    .expect("seed Workflow");
    for (node_key, node_type, linked_function, x, y) in [
        ("start", "start_node", None, 1.5, 2.5),
        ("function", "function_node", Some(function_id), 3.5, 4.5),
        ("answer", "generate_answer_node", None, 5.5, 6.5),
        ("end", "end_node", None, 7.5, 8.5),
    ] {
        sqlx::query(
            "INSERT INTO workflow_nodes \
             (workflow_id,node_key,node_type,function_id,position_x,position_y,node_config,created_at) \
             VALUES (?,?,?,?,?,?,?,?)",
        )
        .bind(workflow_id)
        .bind(node_key)
        .bind(node_type)
        .bind(linked_function)
        .bind(x)
        .bind(y)
        .bind(format!("{{\"fixture\":\"{node_key}\"}}"))
        .bind(AT)
        .execute(store.pool())
        .await
        .expect("seed WorkflowNode");
    }
    for (source, target) in [
        ("start", "function"),
        ("function", "answer"),
        ("answer", "end"),
    ] {
        sqlx::query(
            "INSERT INTO workflow_edges (workflow_id,src_node_key,dst_node_key,mapping) \
             VALUES (?,?,?,'{\"value\":\"result\"}')",
        )
        .bind(workflow_id)
        .bind(source)
        .bind(target)
        .execute(store.pool())
        .await
        .expect("seed WorkflowEdge");
    }
    let function_tool_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO tools \
         (identifier,name,description,kind,source,is_always,function_id,workflow_id,input_schema,output_schema,category_id,required_capabilities,created_at,updated_at) \
         VALUES ('portable-function-tool','Portable Function Tool','function tool','function-wrap','workspace',1,?,NULL,'{\"type\":\"object\"}','{\"type\":\"object\"}',?,'[\"backup.full.capability\"]',?,?) RETURNING id",
    )
    .bind(function_id)
    .bind(category_id)
    .bind(AT)
    .bind(AT)
    .fetch_one(store.pool())
    .await
    .expect("seed Function Tool");
    sqlx::query(
        "INSERT INTO tools \
         (identifier,name,description,kind,source,is_always,function_id,workflow_id,input_schema,output_schema,category_id,required_capabilities,created_at,updated_at) \
         VALUES ('portable-workflow-tool','Portable Workflow Tool','workflow tool','workflow-wrap','workspace',0,NULL,?,'{\"type\":\"object\"}','{\"type\":\"object\"}',?,NULL,?,?)",
    )
    .bind(workflow_id)
    .bind(category_id)
    .bind(AT)
    .bind(AT)
    .execute(store.pool())
    .await
    .expect("seed Workflow Tool");
    let skill_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO skills \
         (identifier,name,description,frontmatter,content,source,is_always,category_id,required_capabilities,created_at,updated_at) \
         VALUES ('portable-skill','Portable Skill','portable skill','---\ntitle: portable\n---','portable skill content','workspace',1,?,'[\"backup.full.capability\"]',?,?) RETURNING id",
    )
    .bind(category_id)
    .bind(AT)
    .bind(AT)
    .fetch_one(store.pool())
    .await
    .expect("seed Skill");
    let root_agent_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO agents \
         (identifier,name,description,system_prompt,parent_agent_id,depth,is_default,model_preset,category_id,name_normalized,created_at,updated_at) \
         VALUES ('portable-root-agent','Portable Root Agent','root agent','portable root prompt',NULL,0,1,'portable-preset',?,'portable root agent',?,?) RETURNING id",
    )
    .bind(category_id)
    .bind(AT)
    .bind(AT)
    .fetch_one(store.pool())
    .await
    .expect("seed root Agent");
    let child_agent_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO agents \
         (identifier,name,description,system_prompt,parent_agent_id,depth,is_default,model_preset,category_id,name_normalized,created_at,updated_at) \
         VALUES ('portable-child-agent','Portable Child Agent','child agent','portable child prompt',?,1,0,'portable-preset',?,'portable child agent',?,?) RETURNING id",
    )
    .bind(root_agent_id)
    .bind(category_id)
    .bind(AT)
    .bind(AT)
    .fetch_one(store.pool())
    .await
    .expect("seed child Agent relation");
    sqlx::query("INSERT INTO agent_tools (agent_id,tool_id,created_at) VALUES (?,?,?)")
        .bind(root_agent_id)
        .bind(function_tool_id)
        .bind(AT)
        .execute(store.pool())
        .await
        .expect("seed Agent Tool relation");
    sqlx::query("INSERT INTO agent_skills (agent_id,skill_id,created_at) VALUES (?,?,?)")
        .bind(child_agent_id)
        .bind(skill_id)
        .bind(AT)
        .execute(store.pool())
        .await
        .expect("seed Agent Skill relation");
    sqlx::query(
        "INSERT INTO agent_capabilities (agent_id,capability_name,created_at) VALUES (?,'backup.full.capability',?)",
    )
    .bind(root_agent_id)
    .bind(AT)
    .execute(store.pool())
    .await
    .expect("seed Agent Capability relation");

    let session_id = "00000000-0000-4000-8000-000000001219";
    let execution_id = "00000000-0000-4000-8000-000000001220";
    sqlx::query(
        "INSERT INTO chat_sessions \
         (id,entry_agent_id,current_agent_id,title_encrypted,status,execution_id,created_at,updated_at,expires_at) \
         VALUES (?,?,?,?, 'completed', ?,?,?, '2126-08-26T00:00:00Z')",
    )
    .bind(session_id)
    .bind(root_agent_id)
    .bind(child_agent_id)
    .bind(crypto.encrypt(&secrets["session"]).expect("encrypt session title"))
    .bind(execution_id)
    .bind(AT)
    .bind(AT)
    .execute(store.pool())
    .await
    .expect("seed ChatSession");
    sqlx::query(
        "INSERT INTO chat_messages \
         (session_id,seq,role,content_encrypted,tool_calls_encrypted,created_at) \
         VALUES (?,1,'assistant',?,?,?)",
    )
    .bind(session_id)
    .bind(
        crypto
            .encrypt(&secrets["message"])
            .expect("encrypt message content"),
    )
    .bind(
        crypto
            .encrypt(&secrets["tool_calls"])
            .expect("encrypt tool calls"),
    )
    .bind(AT)
    .execute(store.pool())
    .await
    .expect("seed ChatMessage");
    sqlx::query(
        "INSERT INTO agent_executions \
         (execution_id,session_id,current_agent_id,status,state_encrypted,started_at,finished_at,error_kind) \
         VALUES (?,?,?,'completed',?,?,?,'portable_terminal')",
    )
    .bind(execution_id)
    .bind(session_id)
    .bind(child_agent_id)
    .bind(
        crypto
            .encrypt(&secrets["execution"])
            .expect("encrypt execution state"),
    )
    .bind(AT)
    .bind(AT)
    .execute(store.pool())
    .await
    .expect("seed AgentExecution");

    secrets
}

#[tokio::test(flavor = "current_thread")]
async fn export_then_import_round_trip_recovers_seed_database() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let database = write_seed_database(&workspace).await;
    let target = unique_target_path(&workspace, "roundtrip");

    let exporter = BackupExporter::new(&database);
    let passphrase = "T129-roundtrip-passphrase";
    exporter
        .export_age(&target, passphrase)
        .await
        .expect("export must succeed for a fresh target with a valid source");

    assert!(target.exists(), "export target must be created");
    assert!(
        target.metadata().expect("metadata").len() > 0,
        "export target must contain ciphertext"
    );

    let importer = BackupImporter::new(workspace.root().join("restore-staging"));
    let restored_database = importer
        .import_age(&target, passphrase, workspace.root().join("restore-out"))
        .await
        .expect("import must succeed with matching passphrase");
    let restored = Store::open_local(StoreOpenOptions::new(
        &restored_database,
        restored_database
            .parent()
            .expect("restore parent")
            .join("plugins"),
    ))
    .await
    .expect("open restored v4 Store");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM data_sources WHERE name='backup-seed'")
            .fetch_one(restored.pool())
            .await
            .expect("restored seed count"),
        1,
        "round-trip must preserve the user entity"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn export_refuses_overwrite_of_existing_target() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let database = write_seed_database(&workspace).await;
    let target = unique_target_path(&workspace, "overwrite");
    fs::write(&target, b"existing-artifact").expect("write target");

    let exporter = BackupExporter::new(&database);
    let result = exporter
        .export_age(&target, "T129-overwrite-passphrase")
        .await;
    match result {
        Err(ExportError::TargetExists(_)) => {}
        other => panic!("expected TargetExists, got {other:?}"),
    }
    // Original target must remain unchanged.
    let bytes = fs::read(&target).expect("read target");
    assert_eq!(
        bytes, b"existing-artifact",
        "target must not be overwritten"
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn export_refuses_symlinked_source() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let database = write_seed_database(&workspace).await;
    // Replace the database with a symlink to a writable file.
    fs::remove_file(&database).expect("remove seed");
    let target_link = workspace.root().join("redirected-seed");
    fs::write(&target_link, b"redirected-bytes").expect("write link target");
    std::os::unix::fs::symlink(&target_link, &database).expect("symlink");

    let exporter = BackupExporter::new(&database);
    let result = exporter
        .export_age(
            &unique_target_path(&workspace, "symlink"),
            "T129-symlink-passphrase",
        )
        .await;
    match result {
        Err(ExportError::UnsafeSource(_)) => {}
        other => panic!("expected UnsafeSource, got {other:?}"),
    }
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn export_refuses_an_intermediate_symlink_in_the_database_source_path() {
    use std::os::unix::fs::symlink;

    let workspace = TestWorkspace::new().expect("workspace");
    let outside = workspace.root().join("outside-source");
    fs::create_dir(&outside).expect("create outside source");
    let database = outside.join("datasources.db");
    fs::write(&database, b"SQLite format 3\0outside-source-canary\0")
        .expect("write outside database canary");
    let alias = workspace.root().join("source-alias");
    symlink(&outside, &alias).expect("plant source parent symlink");
    let archive = unique_target_path(&workspace, "source-intermediate-symlink");

    let error = BackupExporter::new(alias.join("datasources.db"))
        .export_age(&archive, "T129-source-parent-passphrase")
        .await
        .expect_err("source intermediate symlink must be rejected before reading");
    assert!(matches!(error, ExportError::UnsafeSource(_)));
    assert!(!archive.exists());
    assert_eq!(
        fs::read(database).expect("outside database canary remains"),
        b"SQLite format 3\0outside-source-canary\0"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn import_with_wrong_passphrase_is_rejected() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let database = write_seed_database(&workspace).await;
    let target = unique_target_path(&workspace, "wrongpass");

    let exporter = BackupExporter::new(&database);
    exporter
        .export_age(&target, "T129-correct-passphrase")
        .await
        .expect("export");

    let importer = BackupImporter::new(workspace.root().join("restore-staging"));
    let result = importer
        .import_age(
            &target,
            "T129-wrong-passphrase",
            workspace.root().join("restore-out"),
        )
        .await;
    match result {
        Err(ImportError::AuthenticationFailed) => {}
        other => panic!("expected AuthenticationFailed, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn authenticated_stream_truncation_and_tail_tamper_publish_no_target() {
    let workspace = TestWorkspace::new().expect("workspace");
    let database = write_seed_database(&workspace).await;
    let archive = unique_target_path(&workspace, "authenticated-stream");
    BackupExporter::new(database)
        .export_age(&archive, "T129-authenticated-stream-passphrase")
        .await
        .expect("create valid authenticated archive");
    let original = fs::read(&archive).expect("read authenticated archive");
    assert!(
        original.len() > 64,
        "fixture must contain an authenticated body"
    );

    let mut tampered = original.clone();
    let tail_index = tampered.len() - 17;
    tampered[tail_index] ^= 0x80;
    let variants = [
        ("truncated", original[..original.len() - 1].to_vec()),
        ("tampered", tampered),
    ];
    let importer = BackupImporter::new(workspace.root().join("auth-failure-staging"));
    for (label, bytes) in variants {
        let invalid = unique_target_path(&workspace, &format!("authenticated-{label}"));
        fs::write(&invalid, bytes).expect("write private invalid archive copy");
        let target = workspace
            .root()
            .join(format!("authenticated-{label}-target"));
        let error = importer
            .import_age(&invalid, "T129-authenticated-stream-passphrase", &target)
            .await
            .expect_err("authentication end failure must reject the whole archive");
        assert!(
            matches!(
                error,
                ImportError::AuthenticationFailed | ImportError::Age(_)
            ),
            "authenticated {label} error must stay in the authentication envelope: {error:?}"
        );
        assert!(!target.exists());
    }
}

#[tokio::test(flavor = "current_thread")]
async fn import_rejects_duplicate_manifest_and_database_entries_before_staging() {
    let workspace = TestWorkspace::new().expect("workspace");
    let manifest = serde_json::to_vec(&serde_json::json!({
        "schema_version": 4,
        "role": "primary",
        "uuid": "00000000-0000-4000-8000-000000000129",
        "restore_db_id": "00000000-0000-4000-8000-000000000130",
        "database_name": "datasources.db",
        "ownership_state": "unarmed",
        "format": 3
    }))
    .expect("manifest fixture");
    let database = b"SQLite format 3\0duplicate-entry-fixture\0";
    let importer = BackupImporter::new(workspace.root().join("duplicate-entry-staging"));
    for (label, entries) in [
        (
            "manifest",
            vec![
                ("manifest.json", manifest.as_slice()),
                ("manifest.json", manifest.as_slice()),
                ("datasources.db", database.as_slice()),
            ],
        ),
        (
            "database",
            vec![
                ("manifest.json", manifest.as_slice()),
                ("datasources.db", database.as_slice()),
                ("datasources.db", database.as_slice()),
            ],
        ),
    ] {
        let archive = unique_target_path(&workspace, &format!("duplicate-{label}"));
        write_authenticated_tar_fixture(&archive, "T129-duplicate-entry-passphrase", &entries);
        let target = workspace.root().join(format!("duplicate-{label}-target"));
        let error = importer
            .import_age(&archive, "T129-duplicate-entry-passphrase", &target)
            .await
            .expect_err("duplicate canonical entry must be rejected");
        assert!(matches!(error, ImportError::InvalidManifest(_)));
        assert!(!target.exists(), "invalid archive must create no target");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn import_missing_manifested_entity_cleans_every_staged_and_target_byte() {
    let workspace = TestWorkspace::new().expect("workspace");
    let database_path = write_seed_database(&workspace).await;
    let valid = unique_target_path(&workspace, "missing-entity-source");
    let passphrase = "T129-missing-entity-passphrase";
    BackupExporter::new(database_path)
        .export_age(&valid, passphrase)
        .await
        .expect("export valid entity archive");
    let mut entries = read_authenticated_tar_entries(&valid, passphrase);
    entries.remove("entities/data_sources.json");
    let archive = unique_target_path(&workspace, "missing-entity");
    write_authenticated_entry_map(&archive, passphrase, &entries);
    let staging = workspace.root().join("missing-entity-staging");
    let target = workspace.root().join("missing-entity-target");
    let error = BackupImporter::new(&staging)
        .import_age(&archive, passphrase, &target)
        .await
        .expect_err("a current portable archive missing a manifested entity must fail closed");
    assert!(matches!(error, ImportError::InvalidManifest(_)));
    assert!(
        !target.exists(),
        "failed import must publish no target tree"
    );
    assert!(
        !staging.exists()
            || fs::read_dir(&staging)
                .expect("staging directory")
                .next()
                .is_none(),
        "failed import must leave no staged instance"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn import_rejects_noncanonical_or_mismatched_portable_manifest_fields() {
    let workspace = TestWorkspace::new().expect("workspace");
    let database = write_seed_database(&workspace).await;
    let passphrase = "T129-manifest-identity-passphrase";
    let valid_archive = unique_target_path(&workspace, "manifest-valid-source");
    BackupExporter::new(database)
        .export_age(&valid_archive, passphrase)
        .await
        .expect("export valid portable manifest source");
    let valid_entries = read_authenticated_tar_entries(&valid_archive, passphrase);
    let valid: serde_json::Value =
        serde_json::from_slice(valid_entries.get("manifest.json").expect("valid manifest"))
            .expect("parse valid manifest");
    let importer = BackupImporter::new(workspace.root().join("manifest-identity-staging"));
    for (label, field, value) in [
        ("schema", "schema_version", serde_json::json!(3)),
        ("format-name", "format", serde_json::json!("other-backup")),
        (
            "exported-at",
            "exported_at",
            serde_json::json!("not-rfc3339"),
        ),
    ] {
        let mut manifest = valid.clone();
        manifest[field] = value;
        let mut entries = valid_entries.clone();
        entries.insert(
            "manifest.json".into(),
            serde_json::to_vec(&manifest).expect("encode invalid manifest fixture"),
        );
        let archive = unique_target_path(&workspace, &format!("manifest-{label}"));
        write_authenticated_entry_map(&archive, passphrase, &entries);
        let target = workspace.root().join(format!("manifest-{label}-target"));
        let error = importer
            .import_age(&archive, passphrase, &target)
            .await
            .expect_err("invalid manifest identity must fail before staging");
        assert!(matches!(error, ImportError::InvalidManifest(_)));
        assert!(!target.exists());
    }
}

#[tokio::test(flavor = "current_thread")]
async fn format_2_archive_is_accepted_and_normalised() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let database = write_seed_database(&workspace).await;
    let target = unique_target_path(&workspace, "format2");

    let exporter = BackupExporter::with_format(&database, 2);
    exporter
        .export_age(&target, "T129-format2-passphrase")
        .await
        .expect("export format 2 must succeed");

    let importer = BackupImporter::new(workspace.root().join("restore-staging"));
    let restored_database = importer
        .import_age(
            &target,
            "T129-format2-passphrase",
            workspace.root().join("restore-out"),
        )
        .await
        .expect("import format 2 must succeed");
    let restored = Store::open_local(StoreOpenOptions::new(
        &restored_database,
        restored_database
            .parent()
            .expect("restore parent")
            .join("plugins"),
    ))
    .await
    .expect("format 2 archive must materialize a current Store");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM data_sources WHERE name='backup-seed'")
            .fetch_one(restored.pool())
            .await
            .expect("format 2 restored seed count"),
        1
    );
}

#[tokio::test(flavor = "current_thread")]
async fn current_writer_uses_format_3_and_version_bounds_fail_closed() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let database = write_seed_database(&workspace).await;
    let current = unique_target_path(&workspace, "format-current");
    BackupExporter::new(&database)
        .export_age(&current, "T129-format-current-passphrase")
        .await
        .expect("current export");
    let importer = BackupImporter::new(workspace.root().join("format-staging"));
    assert_eq!(
        importer
            .inspect_manifest(&current, "T129-format-current-passphrase")
            .await
            .expect("inspect current manifest")
            .format_version,
        3,
        "new archives must be written at the current format"
    );

    for (format, expected) in [(0, "backup_too_old"), (4, "backup_newer_version")] {
        let archive = unique_target_path(&workspace, &format!("format-{format}"));
        BackupExporter::with_format(&database, format)
            .export_age(&archive, "T129-format-bound-passphrase")
            .await
            .expect("construct authenticated version-bound fixture");
        let error = importer
            .import_age(
                &archive,
                "T129-format-bound-passphrase",
                workspace.root().join(format!("format-{format}-out")),
            )
            .await
            .expect_err("unsupported version must fail before publishing a target");
        assert_eq!(error.to_string(), expected);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn portable_manifest_carries_current_format_and_complete_entity_inventory() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let database = write_seed_database(&workspace).await;
    let target = unique_target_path(&workspace, "manifest");

    let exporter = BackupExporter::new(&database);
    exporter
        .export_age(&target, "T129-manifest-passphrase")
        .await
        .expect("export");

    let importer = BackupImporter::new(workspace.root().join("restore-staging"));
    let manifest = importer
        .inspect_manifest(&target, "T129-manifest-passphrase")
        .await
        .expect("manifest inspect must succeed");
    assert_eq!(manifest.schema_version, 4);
    assert_eq!(manifest.format, "hivegui-backup");
    assert_eq!(manifest.format_version, 3);
    assert_eq!(manifest.entity_files.len(), 22);
    assert!(manifest.artifacts.is_empty());
    assert!(chrono::DateTime::parse_from_rfc3339(&manifest.exported_at).is_ok());
}

#[tokio::test(flavor = "current_thread")]
async fn open_store_backup_includes_committed_wal_and_reencrypts_for_the_target_device() {
    let source = TestWorkspace::new().expect("source workspace");
    let target_device = TestWorkspace::new().expect("target device workspace");
    let source_store = Store::open_local(StoreOpenOptions::new(
        source.database_path(),
        source.plugin_root(),
    ))
    .await
    .expect("open source Store");
    let password = b"T119-cross-device-password-canary";
    source_store
        .create(
            "committed-after-open",
            "127.0.0.1",
            3306,
            "backup-user",
            password,
        )
        .await
        .expect("commit source row while Store remains open");

    let archive = unique_target_path(&source, "open-store-wal");
    BackupExporter::new(source.database_path())
        .export_age(&archive, "T119-open-store-passphrase")
        .await
        .expect("export open Store through the safe snapshot boundary");
    let restored_database = BackupImporter::new(target_device.root().join("restore-staging"))
        .import_age(
            &archive,
            "T119-open-store-passphrase",
            target_device.data_home().join("hivegui"),
        )
        .await
        .expect("authenticated import on the target device");
    let restored_store = Store::open_local(StoreOpenOptions::new(
        &restored_database,
        target_device.plugin_root(),
    ))
    .await
    .expect("open restored Store with the target device key");
    let rows = restored_store.list().await.expect("list restored rows");
    assert_eq!(
        rows.len(),
        1,
        "the committed WAL frame must be in the backup"
    );
    assert_eq!(rows[0].name, "committed-after-open");
    assert_eq!(
        restored_store
            .decrypt_password(&rows[0].encrypted_password)
            .expect("target key decrypts re-encrypted secret"),
        password,
        "portable restore must re-encrypt sensitive fields with the target device key"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn portable_archive_carries_managed_wasm_but_rebuilds_derived_search_and_excludes_ledgers() {
    let source = TestWorkspace::new().expect("source workspace");
    let target_device = TestWorkspace::new().expect("target workspace");
    let source_store = Store::open_local(StoreOpenOptions::new(
        source.database_path(),
        source.plugin_root(),
    ))
    .await
    .expect("open source Store");
    let artifact_key = "portable-fixture/1.0.0/plugin.wasm";
    let artifact_path = source.plugin_root().join(artifact_key);
    fs::create_dir_all(artifact_path.parent().expect("artifact parent")).expect("mkdir artifact");
    let wasm = b"\0asmT119-managed-wasm-fixture";
    fs::write(&artifact_path, wasm).expect("write managed WASM fixture");
    let unowned_artifact = source.plugin_root().join("unowned/plugin.wasm");
    fs::create_dir_all(unowned_artifact.parent().expect("unowned parent"))
        .expect("mkdir unowned artifact");
    fs::write(&unowned_artifact, b"must-not-enter-portable-backup")
        .expect("write unowned artifact");
    let wasm_sha256 = hex::encode(Sha256::digest(wasm));
    sqlx::query(
        "INSERT INTO plugins (identifier,name,version,s3_key,sha256,size_bytes,runtime,created_at,updated_at) \
         VALUES ('portable-fixture','Portable Fixture','1.0.0',?,?,?,'wasm32','2026-08-25T00:00:00Z','2026-08-25T00:00:00Z')",
    )
    .bind(artifact_key)
    .bind(wasm_sha256)
    .bind(wasm.len() as i64)
    .execute(source_store.pool())
    .await
    .expect("seed Plugin row");
    sqlx::query(
        "INSERT INTO plugin_artifact_operations \
         (operation_id,kind,staging_name,state,created_at,updated_at) \
         VALUES ('00000000-0000-4000-8000-000000000119','create','t119-ledger','done','2026-08-25T00:00:00Z','2026-08-25T00:00:00Z')",
    )
    .execute(source_store.pool())
    .await
    .expect("seed operation ledger");
    sqlx::query(
        "INSERT INTO plugin_artifact_gc \
         (artifact_key,source_operation_id,last_attempt_at,reason,state) \
         VALUES ('obsolete/plugin.wasm','00000000-0000-4000-8000-000000000119',0,'test','pending')",
    )
    .execute(source_store.pool())
    .await
    .expect("seed GC ledger");
    sqlx::query(
        "INSERT INTO search_documents (entity_type,entity_key,field,normalized_text) \
         VALUES ('plugin','nonexistent-derived-row','name','must-not-survive-portable-restore')",
    )
    .execute(source_store.pool())
    .await
    .expect("seed deliberately stale derived search row");

    let archive = unique_target_path(&source, "portable-structure");
    BackupExporter::new(source.database_path())
        .export_age(&archive, "T119-portable-structure-passphrase")
        .await
        .expect("export portable archive");
    let restored_database = BackupImporter::new(target_device.root().join("restore-staging"))
        .import_age(
            &archive,
            "T119-portable-structure-passphrase",
            target_device.data_home().join("hivegui"),
        )
        .await
        .expect("restore portable archive");
    let restored_store = Store::open_local(StoreOpenOptions::new(
        &restored_database,
        target_device.plugin_root(),
    ))
    .await
    .expect("open restored Store");
    let internal_counts: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM plugin_artifact_operations), \
                (SELECT COUNT(*) FROM plugin_artifact_gc)",
    )
    .fetch_one(restored_store.pool())
    .await
    .expect("portable ledger counts");
    assert_eq!(
        internal_counts,
        (0, 0),
        "portable archives must exclude operation and GC ledgers"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM search_documents WHERE entity_key = 'nonexistent-derived-row'",
        )
        .fetch_one(restored_store.pool())
        .await
        .expect("derived row count"),
        0,
        "portable restore must rebuild derived search from entities"
    );
    assert_eq!(
        fs::read(target_device.plugin_root().join(artifact_key)).expect("restored managed WASM"),
        wasm,
        "every managed Plugin artifact must round-trip with its entity"
    );
    assert!(
        !target_device
            .plugin_root()
            .join("unowned/plugin.wasm")
            .exists(),
        "portable archive must be driven by active Plugin ownership rows, not directory traversal"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn every_portable_entity_relation_sensitive_field_and_managed_wasm_round_trips_exactly() {
    let source = TestWorkspace::new().expect("source workspace");
    let target = TestWorkspace::new().expect("target workspace");
    let source_store = Store::open_local(StoreOpenOptions::new(
        source.database_path(),
        source.plugin_root(),
    ))
    .await
    .expect("open source Store");
    let expected_secrets = seed_complete_portable_fixture(&source, &source_store).await;
    let source_device_key = fs::read(
        source
            .database_path()
            .parent()
            .expect("source database parent")
            .join("encryption.key"),
    )
    .expect("source device key");

    let passphrase = "T129-complete-entity-roundtrip-passphrase";
    let source_archive = unique_target_path(&source, "complete-entities-source");
    BackupExporter::new(source.database_path())
        .export_age(&source_archive, passphrase)
        .await
        .expect("export complete portable fixture");
    let source_entries = read_authenticated_tar_entries(&source_archive, passphrase);
    let source_manifest: serde_json::Value = serde_json::from_slice(
        source_entries
            .get("manifest.json")
            .expect("source portable manifest"),
    )
    .expect("parse source portable manifest");
    let descriptors = source_manifest["entity_files"]
        .as_array()
        .expect("complete entity descriptor array");
    assert_eq!(descriptors.len(), 22);
    for descriptor in descriptors {
        let name = descriptor["name"].as_str().expect("entity name");
        let count = descriptor["count"].as_u64().expect("entity row count");
        assert!(
            count > 0,
            "portable entity {name} must be physically nonempty"
        );
    }
    assert_eq!(source_manifest["artifacts"].as_array().unwrap().len(), 2);

    let restored_database = BackupImporter::new(target.root().join("complete-import-staging"))
        .import_age(
            &source_archive,
            passphrase,
            target.data_home().join("hivegui"),
        )
        .await
        .expect("import all entity and relation rows");
    let target_device_key = fs::read(
        restored_database
            .parent()
            .expect("restored database parent")
            .join("encryption.key"),
    )
    .expect("target device key");
    assert_ne!(
        source_device_key, target_device_key,
        "cross-device restore must use a distinct target device key"
    );

    let target_archive = unique_target_path(&target, "complete-entities-target");
    BackupExporter::new(&restored_database)
        .export_age(&target_archive, passphrase)
        .await
        .expect("re-export restored portable fixture");
    let target_entries = read_authenticated_tar_entries(&target_archive, passphrase);
    let portable_paths = source_entries
        .keys()
        .filter(|path| path.starts_with("entities/") || path.starts_with("plugins/"))
        .cloned()
        .collect::<BTreeSet<_>>();
    assert_eq!(
        portable_paths,
        target_entries
            .keys()
            .filter(|path| path.starts_with("entities/") || path.starts_with("plugins/"))
            .cloned()
            .collect(),
        "the restored archive must carry the exact same entity and artifact inventory"
    );
    for path in portable_paths {
        assert_eq!(
            source_entries.get(&path),
            target_entries.get(&path),
            "every portable field/relation/artifact byte must round-trip at {path}"
        );
    }

    let target_options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&restored_database)
        .create_if_missing(false)
        .foreign_keys(true);
    let target_pool = sqlx::sqlite::SqlitePoolOptions::new()
        .min_connections(1)
        .max_connections(1)
        .connect_with(target_options)
        .await
        .expect("open restored database for physical verification");
    let target_crypto = Crypto::new(
        &target_device_key
            .as_slice()
            .try_into()
            .expect("target device key is exactly 32 bytes"),
    );
    for (label, query) in [
        (
            "datasource",
            "SELECT encrypted_password FROM data_sources WHERE name='portable-complete-datasource'",
        ),
        (
            "provider",
            "SELECT token_encrypted FROM llm_providers WHERE name='portable-provider'",
        ),
        (
            "session",
            "SELECT title_encrypted FROM chat_sessions WHERE id='00000000-0000-4000-8000-000000001219'",
        ),
        (
            "message",
            "SELECT content_encrypted FROM chat_messages WHERE session_id='00000000-0000-4000-8000-000000001219'",
        ),
        (
            "tool_calls",
            "SELECT tool_calls_encrypted FROM chat_messages WHERE session_id='00000000-0000-4000-8000-000000001219'",
        ),
        (
            "execution",
            "SELECT state_encrypted FROM agent_executions WHERE execution_id='00000000-0000-4000-8000-000000001220'",
        ),
    ] {
        let source_ciphertext: Vec<u8> = sqlx::query_scalar(query)
            .fetch_one(source_store.pool())
            .await
            .unwrap_or_else(|_| panic!("read source ciphertext for {label}"));
        let target_ciphertext: Vec<u8> = sqlx::query_scalar(query)
            .fetch_one(&target_pool)
            .await
            .unwrap_or_else(|_| panic!("read target ciphertext for {label}"));
        assert_ne!(
            source_ciphertext, target_ciphertext,
            "{label} must be freshly encrypted under the target device key"
        );
        assert_eq!(
            target_crypto
                .decrypt(&target_ciphertext)
                .unwrap_or_else(|_| panic!("decrypt restored {label}")),
            expected_secrets[label],
            "{label} plaintext must round-trip exactly"
        );
    }
    assert!(
        sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(&target_pool)
            .await
            .expect("target foreign_key_check")
            .is_empty(),
        "every restored relationship must satisfy the target v4 foreign keys"
    );
    let search_rows: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM search_documents WHERE entity_type IN ('agent','function','plugin','workflow')",
    )
    .fetch_one(&target_pool)
    .await
    .expect("count rebuilt derived search rows");
    assert!(
        search_rows > 0,
        "derived search rows must be rebuilt from entities"
    );
    let internal_ledgers: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM plugin_artifact_operations), \
                (SELECT COUNT(*) FROM plugin_artifact_gc)",
    )
    .fetch_one(&target_pool)
    .await
    .expect("read restored internal ledgers");
    assert_eq!(internal_ledgers, (0, 0));
    target_pool.close().await;
}

#[tokio::test(flavor = "current_thread")]
async fn portable_archive_is_manifested_entity_json_without_a_sqlite_or_local_key_payload() {
    let source = TestWorkspace::new().expect("source workspace");
    let database = write_seed_database(&source).await;
    let archive = unique_target_path(&source, "portable-entity-layout");
    BackupExporter::new(database)
        .export_age(&archive, "T129-portable-entity-layout-passphrase")
        .await
        .expect("export current portable archive");

    let entries =
        read_authenticated_tar_entries(&archive, "T129-portable-entity-layout-passphrase");
    let manifest: serde_json::Value = serde_json::from_slice(
        entries
            .get("manifest.json")
            .expect("portable manifest entry"),
    )
    .expect("parse portable manifest");
    assert_eq!(
        manifest
            .as_object()
            .expect("manifest object")
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>(),
        [
            "artifacts",
            "entity_files",
            "exported_at",
            "format",
            "format_version",
            "schema_version",
        ]
        .into_iter()
        .map(str::to_string)
        .collect(),
        "portable manifest has one exact versioned schema"
    );
    assert_eq!(manifest["format"], "hivegui-backup");
    assert_eq!(manifest["format_version"], 3);
    assert_eq!(manifest["schema_version"], 4);
    assert!(
        manifest["exported_at"]
            .as_str()
            .is_some_and(|value| chrono::DateTime::parse_from_rfc3339(value).is_ok()),
        "exported_at is RFC3339"
    );

    let expected_entities = [
        "agent_capabilities",
        "agent_executions",
        "agent_skills",
        "agent_tools",
        "agents",
        "capabilities",
        "categories",
        "chat_messages",
        "chat_sessions",
        "data_sources",
        "functions",
        "global_configs",
        "llm_presets",
        "llm_providers",
        "models",
        "plugins",
        "skills",
        "tags",
        "tools",
        "workflow_edges",
        "workflow_nodes",
        "workflows",
    ]
    .into_iter()
    .map(str::to_string)
    .collect::<BTreeSet<_>>();
    let entity_files = manifest["entity_files"]
        .as_array()
        .expect("entity file inventory");
    assert_eq!(
        entity_files
            .iter()
            .map(|entry| entry["name"].as_str().expect("entity name").to_string())
            .collect::<BTreeSet<_>>(),
        expected_entities,
        "empty entity types remain explicitly represented"
    );
    for entity in entity_files {
        let path = entity["path"].as_str().expect("entity path");
        let bytes = entries.get(path).expect("manifested entity file exists");
        assert_eq!(
            hex::encode(Sha256::digest(bytes)),
            entity["sha256"].as_str().expect("entity digest")
        );
        let body: serde_json::Value = serde_json::from_slice(bytes).expect("entity JSON");
        assert_eq!(body["schema_version"], 4);
        assert_eq!(body["entity"], entity["name"]);
        assert_eq!(
            body["rows"].as_array().expect("entity rows").len() as u64,
            entity["count"].as_u64().expect("entity row count")
        );
    }
    assert!(!entries.contains_key("datasources.db"));
    assert!(!entries.contains_key("portable-field-key.bin"));
    assert!(entries.keys().all(|path| {
        path == "manifest.json" || path.starts_with("entities/") || path.starts_with("plugins/")
    }));
}

#[tokio::test(flavor = "current_thread")]
async fn legacy_entity_archives_upgrade_integer_kinds_and_dotted_builtins_once() {
    let source = TestWorkspace::new().expect("source workspace");
    let database = write_seed_database(&source).await;
    let current = unique_target_path(&source, "legacy-upgrade-source");
    let passphrase = "T129-legacy-entity-upgrade-passphrase";
    BackupExporter::new(database)
        .export_age(&current, passphrase)
        .await
        .expect("export current entity archive");
    let legacy = unique_target_path(&source, "legacy-upgrade-fixture");
    rewrite_portable_entity_archive(&current, &legacy, passphrase, 2, |name, body| {
        if name != "functions" {
            return;
        }
        let columns = body["columns"].as_array().expect("Function columns");
        let identifier = columns
            .iter()
            .position(|column| column == "identifier")
            .expect("identifier column");
        let kind = columns
            .iter()
            .position(|column| column == "kind")
            .expect("kind column");
        for row in body["rows"].as_array_mut().expect("Function rows") {
            let row = row.as_array_mut().expect("Function row");
            if row[kind]["value"] == "builtin" {
                row[kind] = serde_json::json!({"type":"integer","value":1});
                let legacy_identifier = match row[identifier]["value"].as_str().expect("Builtin") {
                    "format_template" => "format.template",
                    "json_parse" => "json.parse",
                    "json_stringify" => "json.stringify",
                    "text_regex_match" => "text.regex_match",
                    other => panic!("unexpected Builtin {other}"),
                };
                row[identifier] = serde_json::json!({"type":"text","value":legacy_identifier});
            }
        }
    });

    let target = TestWorkspace::new().expect("target workspace");
    let restored = BackupImporter::new(target.root().join("legacy-upgrade-staging"))
        .import_age(&legacy, passphrase, target.data_home().join("hivegui"))
        .await
        .expect("trusted format 2 upgrade");
    let store = Store::open_local(StoreOpenOptions::new(&restored, target.plugin_root()))
        .await
        .expect("open upgraded Store");
    let builtins = sqlx::query_as::<_, (String, String)>(
        "SELECT identifier, kind FROM functions WHERE kind='builtin' ORDER BY identifier",
    )
    .fetch_all(store.pool())
    .await
    .expect("read upgraded Builtins");
    assert_eq!(
        builtins,
        vec![
            ("format_template".into(), "builtin".into()),
            ("json_parse".into(), "builtin".into()),
            ("json_stringify".into(), "builtin".into()),
            ("text_regex_match".into(), "builtin".into()),
        ]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn current_integer_kind_and_legacy_builtin_collision_fail_before_target_creation() {
    let source = TestWorkspace::new().expect("source workspace");
    let database = write_seed_database(&source).await;
    let current = unique_target_path(&source, "format-validation-source");
    let passphrase = "T129-format-validation-passphrase";
    BackupExporter::new(database)
        .export_age(&current, passphrase)
        .await
        .expect("export current entity archive");

    for (label, format_version, collision) in [
        ("current-integer-kind", 3, false),
        ("legacy-builtin-collision", 2, true),
    ] {
        let invalid = unique_target_path(&source, label);
        rewrite_portable_entity_archive(
            &current,
            &invalid,
            passphrase,
            format_version,
            |name, body| {
                if name != "functions" {
                    return;
                }
                let columns = body["columns"].as_array().expect("Function columns");
                let id = columns
                    .iter()
                    .position(|column| column == "id")
                    .expect("id column");
                let identifier = columns
                    .iter()
                    .position(|column| column == "identifier")
                    .expect("identifier column");
                let kind = columns
                    .iter()
                    .position(|column| column == "kind")
                    .expect("kind column");
                let rows = body["rows"].as_array_mut().expect("Function rows");
                if collision {
                    let mut duplicate = rows[0].clone();
                    let duplicate = duplicate.as_array_mut().expect("duplicate Function row");
                    duplicate[id] = serde_json::json!({"type":"integer","value":999999});
                    duplicate[identifier] =
                        serde_json::json!({"type":"text","value":"format.template"});
                    duplicate[kind] = serde_json::json!({"type":"integer","value":1});
                    rows.push(serde_json::Value::Array(duplicate.clone()));
                } else {
                    rows[0].as_array_mut().expect("Function row")[kind] =
                        serde_json::json!({"type":"integer","value":1});
                }
            },
        );
        let target = source.root().join(format!("{label}-target"));
        let error = BackupImporter::new(source.root().join(format!("{label}-staging")))
            .import_age(&invalid, passphrase, &target)
            .await
            .expect_err("invalid or colliding portable values must fail closed");
        assert!(matches!(error, ImportError::InvalidManifest(_)));
        assert!(!target.exists(), "validation must precede target creation");
    }
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn portable_export_rejects_an_intermediate_symlink_before_reading_outside_plugin_root() {
    use std::os::unix::fs::symlink;

    let source = TestWorkspace::new().expect("source workspace");
    let source_store = Store::open_local(StoreOpenOptions::new(
        source.database_path(),
        source.plugin_root(),
    ))
    .await
    .expect("open source Store");
    let outside = source.root().join("outside-plugin-root");
    fs::create_dir(&outside).expect("create outside directory");
    let outside_artifact = outside.join("plugin.wasm");
    let bytes = b"\0asmT129-intermediate-symlink-canary";
    fs::write(&outside_artifact, bytes).expect("write outside canary");
    symlink(&outside, source.plugin_root().join("escaped"))
        .expect("plant intermediate directory symlink");
    let artifact_key = "escaped/plugin.wasm";
    sqlx::query(
        "INSERT INTO plugins (identifier,name,version,s3_key,sha256,size_bytes,runtime,created_at,updated_at) \
         VALUES ('escaped-plugin','Escaped Plugin','1.0.0',?,?,?,'wasm32','2026-08-25T00:00:00Z','2026-08-25T00:00:00Z')",
    )
    .bind(artifact_key)
    .bind(hex::encode(Sha256::digest(bytes)))
    .bind(i64::try_from(bytes.len()).expect("fixture size"))
    .execute(source_store.pool())
    .await
    .expect("seed escaped Plugin row");

    let archive = unique_target_path(&source, "intermediate-symlink");
    let error = BackupExporter::new(source.database_path())
        .export_age(&archive, "T129-intermediate-symlink-passphrase")
        .await
        .expect_err("an intermediate symlink must fail before outside bytes are read");
    assert!(matches!(error, ExportError::UnsafeSource(_)));
    assert!(!archive.exists(), "unsafe export must publish no archive");
    assert_eq!(
        fs::read(outside_artifact).expect("outside canary remains readable"),
        bytes,
        "outside bytes must remain untouched"
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn portable_export_rejects_a_symlinked_managed_plugin_root() {
    use std::os::unix::fs::symlink;

    let source = TestWorkspace::new().expect("source workspace");
    let source_store = Store::open_local(StoreOpenOptions::new(
        source.database_path(),
        source.plugin_root(),
    ))
    .await
    .expect("open source Store");
    let outside = source.root().join("outside-managed-root");
    fs::create_dir_all(outside.join("fixture")).expect("create outside Plugin tree");
    let bytes = b"\0asmT129-plugin-root-symlink-canary";
    fs::write(outside.join("fixture/plugin.wasm"), bytes).expect("write outside artifact");
    sqlx::query(
        "INSERT INTO plugins (identifier,name,version,s3_key,sha256,size_bytes,runtime,created_at,updated_at) \
         VALUES ('root-link-plugin','Root Link Plugin','1.0.0','fixture/plugin.wasm',?,?,\
                 'wasm32','2026-08-25T00:00:00Z','2026-08-25T00:00:00Z')",
    )
    .bind(hex::encode(Sha256::digest(bytes)))
    .bind(i64::try_from(bytes.len()).expect("fixture size"))
    .execute(source_store.pool())
    .await
    .expect("seed Plugin row");
    fs::remove_dir(source.plugin_root()).expect("remove empty managed root");
    symlink(&outside, source.plugin_root()).expect("replace managed root with symlink");

    let archive = unique_target_path(&source, "plugin-root-symlink");
    let error = BackupExporter::new(source.database_path())
        .export_age(&archive, "T129-plugin-root-symlink-passphrase")
        .await
        .expect_err("managed Plugin root symlink must be rejected");
    assert!(matches!(error, ExportError::UnsafeSource(_)));
    assert!(!archive.exists());
    assert_eq!(
        fs::read(outside.join("fixture/plugin.wasm")).expect("outside artifact remains"),
        bytes
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn export_rejects_a_symlinked_target_parent_without_writing_outside() {
    use std::os::unix::fs::symlink;

    let workspace = TestWorkspace::new().expect("workspace");
    let database = write_seed_database(&workspace).await;
    let outside = workspace.root().join("outside-target");
    fs::create_dir(&outside).expect("create outside target");
    let canary = outside.join("must-remain-only-entry");
    fs::write(&canary, b"T129-target-parent-canary").expect("write outside canary");
    let target_alias = workspace.root().join("target-alias");
    symlink(&outside, &target_alias).expect("plant target parent symlink");
    let target = target_alias.join("backup.age");

    let error = BackupExporter::new(database)
        .export_age(&target, "T129-target-parent-passphrase")
        .await
        .expect_err("target parent symlink must fail before staging creation");
    assert!(matches!(error, ExportError::UnsafeSource(_)));
    assert_eq!(
        fs::read_dir(&outside)
            .expect("outside directory")
            .map(|entry| entry.expect("outside entry").file_name())
            .collect::<Vec<_>>(),
        vec![canary.file_name().expect("canary name").to_os_string()],
        "unsafe target resolution must perform zero writes outside the selected parent"
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn startup_replay_rejects_a_symlinked_restore_registry_without_external_io() {
    use std::os::unix::fs::symlink;

    let target = TestWorkspace::new().expect("target workspace");
    let outside = TestWorkspace::new().expect("outside workspace");
    let canary = outside.root().join("must-not-touch");
    fs::write(&canary, b"T130-registry-symlink-canary").expect("write outside canary");
    let empty_external_registry = outside.root().join("empty-registry");
    fs::create_dir(&empty_external_registry).expect("create empty external registry");
    symlink(
        &empty_external_registry,
        target.root().join(".hivegui-db-staging-v1"),
    )
    .expect("plant registry symlink");

    let error = match RestoreCoordinator::new(target.root()) {
        Ok(coordinator) => coordinator
            .recover_startup()
            .await
            .expect_err("startup replay must reject a symlinked registry root"),
        Err(error) => error,
    };
    assert!(matches!(error, ImportError::UnsafeArchiveEntry(_)));
    assert_eq!(
        fs::read(canary).expect("outside canary remains"),
        b"T130-registry-symlink-canary",
        "startup replay must perform no external I/O"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn production_startup_replays_restore_before_creating_or_opening_store() {
    const UNKNOWN_ENTRY: &str = "unknown-startup-control";
    const CANARY: &[u8] = b"T130-production-startup-unknown-registry-canary";

    let workspace = TestWorkspace::new().expect("workspace");
    let root = workspace.root().join("blocked-production-startup-root");
    let registry = root.join(".hivegui-db-staging-v1");
    fs::create_dir_all(&registry).expect("create controlled restore registry");
    let unknown_entry = registry.join(UNKNOWN_ENTRY);
    fs::write(&unknown_entry, CANARY).expect("write fixed unknown registry canary");
    let registry_identity = sidecar_file_identity(&registry);
    let unknown_entry_identity = sidecar_file_identity(&unknown_entry);

    let error = open_store_after_restore_recovery(&root)
        .await
        .expect_err("ambiguous restore registry must block production Store startup");
    let import_error = error
        .downcast_ref::<ImportError>()
        .expect("production startup must preserve the typed restore error");
    match import_error {
        ImportError::UnsafeArchiveEntry(detail) => assert_eq!(
            detail.as_str(),
            "unknown registry entry unknown-startup-control",
            "production startup must expose the exact fail-closed registry detail"
        ),
        other => panic!("expected UnsafeArchiveEntry, got {other:?}"),
    }

    assert_eq!(
        sidecar_file_identity(&registry),
        registry_identity,
        "blocked startup must preserve the restore registry identity"
    );
    assert_eq!(
        sidecar_file_identity(&unknown_entry),
        unknown_entry_identity,
        "blocked startup must preserve the unknown entry identity"
    );
    assert_eq!(
        fs::read(&unknown_entry).expect("read retained registry canary"),
        CANARY,
        "blocked startup must preserve the unknown entry bytes"
    );
    for relative in [
        "datasources.db",
        "datasources.db-wal",
        "datasources.db-shm",
        "datasources.db-journal",
        "encryption.key",
        "plugins",
    ] {
        assert!(
            !root.join(relative).exists(),
            "restore ambiguity must block before production creates {relative}"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn production_startup_opens_a_clean_root_with_owner_aware_store() {
    let workspace = TestWorkspace::new().expect("workspace");
    let root = workspace.root().join("clean-production-startup-root");

    let store = open_store_after_restore_recovery(&root)
        .await
        .expect("clean production root must recover and open");
    let database = root.join("datasources.db");
    let plugins = root.join("plugins");
    assert_eq!(store.database_path(), database);
    assert!(
        database.is_file(),
        "production startup must create the database"
    );
    assert!(
        plugins.is_dir(),
        "production startup must create the Plugin root"
    );

    let second_open = Store::open_local(StoreOpenOptions::for_root(&root))
        .await
        .expect_err("the returned production Store must own its database path");
    assert_eq!(
        second_open.kind(),
        StoreOpenErrorKind::AlreadyLocked,
        "production startup must use the owner-aware Store boundary"
    );

    store.pool().close().await;
    drop(store);
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn startup_replay_rejects_a_symlinked_live_instance_before_manifest_io() {
    use std::os::unix::fs::symlink;

    let target = TestWorkspace::new().expect("target workspace");
    let registry = target.root().join(".hivegui-db-staging-v1");
    fs::create_dir(&registry).expect("create registry");
    let outside = target.root().join("outside-live-instance");
    fs::create_dir(&outside).expect("create outside live directory");
    let canary = outside.join("must-not-touch");
    fs::write(&canary, b"T130-live-symlink-canary").expect("write outside canary");
    symlink(
        &outside,
        registry.join("restore-00000000-0000-4000-8000-000000000130"),
    )
    .expect("plant live instance symlink");

    let error = RestoreCoordinator::new(target.root())
        .expect("open coordinator")
        .recover_startup()
        .await
        .expect_err("live instance symlink must fail before manifest lookup");
    assert!(matches!(error, ImportError::UnsafeArchiveEntry(_)));
    assert_eq!(
        fs::read(canary).expect("outside canary remains"),
        b"T130-live-symlink-canary"
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn startup_replay_keeps_using_the_open_root_after_ambient_path_replacement() {
    use std::os::unix::fs::symlink;

    let workspace = TestWorkspace::new().expect("workspace");
    let coordinator_root = workspace.root().join("coordinator-root");
    fs::create_dir(&coordinator_root).expect("create coordinator root");
    let coordinator =
        RestoreCoordinator::new(&coordinator_root).expect("open stable coordinator root");

    let retained_root = workspace.root().join("retained-root");
    fs::rename(&coordinator_root, &retained_root)
        .expect("move opened root without replacing inode");
    let outside = workspace.root().join("outside-root");
    let outside_registry = outside.join(".hivegui-db-staging-v1");
    fs::create_dir_all(&outside_registry).expect("create outside registry");
    let canary = outside_registry.join("must-not-read-or-delete");
    fs::write(&canary, b"T130-open-root-canary").expect("write outside canary");
    symlink(&outside, &coordinator_root).expect("replace ambient root path with symlink");

    let recovery = coordinator
        .recover_startup()
        .await
        .expect("replay must remain rooted at the descriptor opened by new");
    assert!(recovery.store_may_open());
    assert_eq!(
        fs::read(&canary).expect("outside canary remains"),
        b"T130-open-root-canary",
        "root replacement must cause zero I/O beneath the ambient symlink target"
    );
    assert!(
        !retained_root.join(".hivegui-db-staging-v1").exists(),
        "an empty retained root needs no synthetic registry"
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn import_rejects_symlinked_and_hardlinked_archive_descriptors() {
    use std::os::unix::fs::symlink;

    let source = TestWorkspace::new().expect("source workspace");
    let database = write_seed_database(&source).await;
    let archive = unique_target_path(&source, "archive-descriptor");
    BackupExporter::new(database)
        .export_age(&archive, "T129-archive-descriptor-passphrase")
        .await
        .expect("create valid source archive");
    let symlink_alias = source.root().join("archive-symlink.age");
    symlink(&archive, &symlink_alias).expect("create archive symlink");
    let importer = BackupImporter::new(source.root().join("archive-input-staging"));
    let symlink_error = importer
        .inspect_manifest(&symlink_alias, "T129-archive-descriptor-passphrase")
        .await
        .expect_err("archive symlink must be rejected before decryption");
    assert!(matches!(symlink_error, ImportError::UnsafeArchiveEntry(_)));

    fs::remove_file(&symlink_alias).expect("remove symlink alias");
    let hardlink_alias = source.root().join("archive-hardlink.age");
    fs::hard_link(&archive, &hardlink_alias).expect("create archive hardlink");
    let hardlink_error = importer
        .inspect_manifest(&hardlink_alias, "T129-archive-descriptor-passphrase")
        .await
        .expect_err("archive hardlink must be rejected before decryption");
    assert!(matches!(hardlink_error, ImportError::UnsafeArchiveEntry(_)));
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn import_rejects_an_intermediate_symlink_in_the_archive_source_path() {
    use std::os::unix::fs::symlink;

    let workspace = TestWorkspace::new().expect("workspace");
    let database = write_seed_database(&workspace).await;
    let outside = workspace.root().join("outside-archive-source");
    fs::create_dir(&outside).expect("create outside archive directory");
    let archive = outside.join("backup.age");
    BackupExporter::new(database)
        .export_age(&archive, "T129-archive-parent-passphrase")
        .await
        .expect("create authenticated outside archive fixture");
    let alias = workspace.root().join("archive-source-alias");
    symlink(&outside, &alias).expect("plant archive parent symlink");

    let error = BackupImporter::new(workspace.root().join("archive-parent-staging"))
        .inspect_manifest(&alias.join("backup.age"), "T129-archive-parent-passphrase")
        .await
        .expect_err("archive intermediate symlink must fail before decryption");
    assert!(matches!(error, ImportError::UnsafeArchiveEntry(_)));
    assert!(archive.exists(), "outside archive must remain untouched");
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn import_rejects_an_intermediate_symlink_in_the_final_target_parent() {
    use std::os::unix::fs::symlink;

    let workspace = TestWorkspace::new().expect("workspace");
    let database = write_seed_database(&workspace).await;
    let archive = unique_target_path(&workspace, "import-target-parent");
    BackupExporter::new(database)
        .export_age(&archive, "T129-import-target-parent-passphrase")
        .await
        .expect("create authenticated archive");

    let outside = workspace.root().join("outside-import-target");
    fs::create_dir_all(outside.join("nested")).expect("create outside nested target");
    let canary = outside.join("must-remain-only-entry");
    fs::write(&canary, b"T129-import-target-canary").expect("write outside canary");
    let alias = workspace.root().join("import-target-alias");
    symlink(&outside, &alias).expect("plant intermediate target symlink");
    let importer = BackupImporter::new(workspace.root().join("safe-import-staging"));
    let error = importer
        .import_age(
            &archive,
            "T129-import-target-parent-passphrase",
            alias.join("nested/restore"),
        )
        .await
        .expect_err("intermediate target symlink must fail before target creation");
    assert!(matches!(error, ImportError::UnsafeArchiveEntry(_)));
    assert_eq!(
        fs::read_dir(&outside)
            .expect("outside directory")
            .map(|entry| entry.expect("outside entry").file_name())
            .collect::<std::collections::BTreeSet<_>>(),
        [
            canary.file_name().expect("canary name").to_os_string(),
            std::ffi::OsString::from("nested"),
        ]
        .into_iter()
        .collect(),
        "unsafe import target must perform zero writes outside the controlled root"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn confirmation_freezes_after_the_last_legal_write_and_exports_that_write() {
    let source = TestWorkspace::new().expect("source workspace");
    let source_store = Store::open_local(StoreOpenOptions::new(
        source.database_path(),
        source.plugin_root(),
    ))
    .await
    .expect("open source Store");
    source_store
        .create("before-preview", "127.0.0.1", 3306, "user", b"first")
        .await
        .expect("seed before preview");
    let archive = unique_target_path(&source, "confirmation-freeze");
    let coordinator = BackupCoordinator::from_store(source_store.clone())
        .expect("single production backup coordinator");
    let preview = coordinator
        .preview_export(&archive)
        .await
        .expect("preview backup");
    assert_eq!(preview.entity_count("data_sources"), 1);

    source_store
        .create("last-legal-write", "127.0.0.1", 3307, "user", b"last")
        .await
        .expect("the write between preview and confirmation is legal");
    let confirmed = coordinator
        .confirm_export(preview, "T119-confirmation-passphrase")
        .await
        .expect("confirmation freezes and exports");
    let blocked = source_store
        .create("after-confirmation", "127.0.0.1", 3308, "user", b"blocked")
        .await
        .expect_err("writes stay frozen through checkpoint/close/export verification");
    assert!(blocked.to_string().contains("write_gate_closed"));
    assert_eq!(confirmed.entity_count("data_sources"), 2);
    assert!(confirmed.includes_entity("data_sources", "last-legal-write"));
    assert!(confirmed.current_checkpoint_complete());
    assert!(confirmed.sidecars_converged());
}

#[tokio::test(flavor = "current_thread")]
async fn write_gate_close_blocks_shared_pool_business_writes_before_checkpoint() {
    let source = TestWorkspace::new().expect("source workspace");
    let source_store = Store::open_local(StoreOpenOptions::new(
        source.database_path(),
        source.plugin_root(),
    ))
    .await
    .expect("open source Store");
    let agent_store = AgentStore::from_store(&source_store).expect("open production Agent Store");
    let archive = unique_target_path(&source, "shared-pool-write-gate");
    let coordinator = BackupCoordinator::from_store(source_store.clone())
        .expect("single production backup coordinator");
    let preview = coordinator
        .preview_export(&archive)
        .await
        .expect("preview backup");
    let write_gate_close = BACKUP_CONFIRMATION_CRASH_POINTS
        .iter()
        .copied()
        .find(|point| point.as_str() == "write_gate_close")
        .expect("write-gate crash point");

    let interrupted = coordinator
        .confirm_with_crash(
            preview,
            "T119-shared-pool-write-gate-passphrase",
            write_gate_close,
        )
        .await
        .expect_err("the exact write-gate boundary must interrupt");
    assert!(interrupted.reached_requested_boundary());
    let shared_pool_probe = sqlx::query_scalar::<_, i64>("SELECT 1")
        .fetch_one(source_store.pool())
        .await;
    let shared_pool_probe_is_closed = matches!(&shared_pool_probe, Err(sqlx::Error::PoolClosed));
    assert_eq!(
        (source_store.pool().is_closed(), shared_pool_probe_is_closed,),
        (true, true),
        "the first write-gate crash boundary must close the shared SQLx Pool; probe={shared_pool_probe:?}"
    );

    let input = AgentInput::new_root(
        "must_not_commit_after_backup_freeze",
        "Must Not Commit After Backup Freeze",
        "shared-pool write-gate contract",
    )
    .expect("valid Agent input");
    agent_store
        .create(input)
        .await
        .expect_err("every business writer sharing the production Pool must be closed at the gate");

    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(source.database_path())
        .create_if_missing(false)
        .foreign_keys(true);
    let read_pool = sqlx::sqlite::SqlitePoolOptions::new()
        .min_connections(1)
        .max_connections(1)
        .connect_with(options)
        .await
        .expect("open frozen current for verification");
    let count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM agents WHERE identifier = 'must_not_commit_after_backup_freeze'",
    )
    .fetch_one(&read_pool)
    .await
    .expect("read post-freeze Agent count");
    read_pool.close().await;
    assert_eq!(
        count, 0,
        "the write-gate boundary must be zero-modification"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn store_bound_restore_preview_confirmation_drains_pool_and_transfers_owner() {
    const PASSPHRASE: &str = "T130-store-bound-success-passphrase";

    let source = TestWorkspace::new().expect("source workspace");
    let archive = export_single_data_source_archive(
        &source,
        "store-bound-success",
        "restored-new-current",
        PASSPHRASE,
    )
    .await;

    let target = TestWorkspace::new().expect("target workspace");
    let root = target.root().join("store-bound-success-root");
    fs::create_dir_all(&root).expect("create target root");
    let current = Store::open_local(StoreOpenOptions::for_root(&root))
        .await
        .expect("open owner-aware current Store");
    current
        .create(
            "old-before-restore",
            "127.0.0.1",
            3307,
            "old-user",
            b"old-password",
        )
        .await
        .expect("seed old current");
    let stale = current.clone();
    let agent_store = AgentStore::from_store(&stale).expect("share the production Store pool");
    let coordinator =
        RestoreCoordinator::from_store(current).expect("bind restore to the live Store owner");

    let prepared: PreparedRestore = coordinator
        .preview_restore(&archive, PASSPHRASE)
        .await
        .expect("fully authenticate and validate an unarmed preview");
    let operation_id = prepared.db_instance_operation_id().to_owned();
    let safety_snapshot = prepared.safety_backup_path().to_path_buf();
    let expected_safety_snapshot = root
        .join("backups")
        .join(format!("restore-safety-{operation_id}"));
    assert_eq!(safety_snapshot, expected_safety_snapshot);
    assert!(
        !safety_snapshot.exists(),
        "preview must display the exact future path without claiming a safety backup exists"
    );
    assert!(
        !prepared.owner_exists(),
        "preview must not publish a restore owner"
    );

    let live = root
        .join(".hivegui-db-staging-v1")
        .join(format!("restore-{operation_id}"));
    let instance_manifest: serde_json::Value = serde_json::from_slice(
        &fs::read(live.join(".hivegui-db-instance-v1.json"))
            .expect("preview must publish the unarmed instance manifest"),
    )
    .expect("parse preview instance manifest");
    assert_eq!(instance_manifest["ownership_state"], "unarmed");
    assert_eq!(instance_manifest["role"], "restore");
    assert_eq!(instance_manifest["db_instance_operation_id"], operation_id);
    assert!(!live.join(".hivegui-db-recovery-v1.json").exists());
    assert!(!live.join(".hivegui-db-recovery-v1.json.staging").exists());
    let preview_locked = run_store_bound_restore_lock_child(&root, "expect-locked");
    assert_store_bound_restore_lock_child(
        &preview_locked,
        "preview must retain the production Store process lock",
    );

    stale
        .create(
            "last-legal-write",
            "127.0.0.1",
            3308,
            "last-user",
            b"last-password",
        )
        .await
        .expect("current remains writable between preview and final confirmation");
    let held_connection = stale
        .pool()
        .acquire()
        .await
        .expect("hold one pre-confirmation pooled connection");

    let confirmation = coordinator
        .begin_confirmation(&prepared)
        .expect("synchronous confirmation boundary");
    assert!(
        stale.pool().is_closed(),
        "begin_confirmation must terminal-close the shared Pool before returning"
    );
    let drain_locked = run_store_bound_restore_lock_child(&root, "expect-locked");
    assert_store_bound_restore_lock_child(
        &drain_locked,
        "begin_confirmation must retain the process lock while a connection drains",
    );
    let duplicate_begin = match coordinator.begin_confirmation(&prepared) {
        Ok(_) => panic!("one prepared restore must not start two confirmations"),
        Err(error) => error,
    };
    assert_eq!(duplicate_begin.to_string(), PREPARED_RESTORE_NOT_ACTIVE);
    let cancel_while_confirming = coordinator
        .cancel_preview(&prepared)
        .await
        .expect_err("an in-flight confirmation must not be canceled through its preview handle");
    assert_eq!(
        cancel_while_confirming.to_string(),
        PREPARED_RESTORE_NOT_ACTIVE
    );
    assert!(
        live.is_dir(),
        "rejected duplicate use must preserve the active live instance"
    );
    assert!(stale.pool().is_closed());
    let blocked_data_source = stale
        .create(
            "must-not-write-after-confirmation",
            "127.0.0.1",
            3309,
            "blocked-user",
            b"blocked-password",
        )
        .await
        .expect_err("DataSource writes must stop at the synchronous confirmation boundary");
    assert!(
        blocked_data_source
            .to_string()
            .contains("write_gate_closed")
    );
    let blocked_agent = AgentInput::new_root(
        "must_not_write_after_restore_confirmation",
        "Must Not Write After Restore Confirmation",
        "shared Store pool must already be terminal closed",
    )
    .expect("valid blocked Agent input");
    agent_store
        .create(blocked_agent)
        .await
        .expect_err("Agent writes sharing the Store pool must stop at confirmation");

    let mut finish = Box::pin(confirmation.finish());
    tokio::select! {
        result = &mut finish => {
            let _ = result;
            panic!("finish must drain the held pre-confirmation connection");
        }
        _ = tokio::time::sleep(Duration::from_millis(100)) => {}
    }
    drop(held_connection);
    let applied = tokio::time::timeout(Duration::from_secs(5), finish)
        .await
        .expect("finish must complete after the held connection drains")
        .expect("finish complete replacement");
    assert_eq!(applied.retirement_outcome(), RetirementOutcome::New);
    assert!(applied.retirement_is_done());
    assert!(applied.has_exactly_one_live_database());
    assert!(applied.has_no_mixed_database_or_plugin_tree());
    assert!(
        !applied.store_may_open() && !applied.write_gate_is_open(),
        "successful in-process restore stays terminal until startup recovery"
    );
    assert!(stale.pool().is_closed());
    let terminal_locked = run_store_bound_restore_lock_child(&root, "expect-locked");
    assert_store_bound_restore_lock_child(
        &terminal_locked,
        "finish must retain the process lock until startup recovery",
    );
    let terminal_gate = stale
        .create(
            "must-not-write-after-finish",
            "127.0.0.1",
            3310,
            "blocked-user",
            b"blocked-password",
        )
        .await
        .expect_err("finish must not reopen the old Store write gate");
    assert!(terminal_gate.to_string().contains("write_gate_closed"));

    assert_eq!(
        data_source_names_from_database(&root.join("datasources.db")).await,
        vec!["restored-new-current"]
    );
    assert!(safety_snapshot.join("manifest.json").is_file());
    assert_eq!(
        data_source_names_from_database(&safety_snapshot.join("datasources.db")).await,
        vec!["last-legal-write", "old-before-restore"]
    );
    assert_eq!(
        fs::read_dir(root.join(".hivegui-db-staging-v1"))
            .expect("read retired restore registry")
            .count(),
        0,
        "successful finish must durably retire its live instance"
    );

    drop(prepared);
    let startup = coordinator
        .recover_startup()
        .await
        .expect("startup recovery reopens the terminal gate");
    assert!(startup.store_may_open());
    assert!(startup.write_gate_is_open());
    assert!(startup.retirement_is_done());
    let recovery_open = run_store_bound_restore_lock_child(&root, "expect-open");
    assert_store_bound_restore_lock_child(
        &recovery_open,
        "startup recovery must release the process lock for a fresh owner",
    );
    drop(coordinator);

    let reopened = Store::open_local(StoreOpenOptions::for_root(&root))
        .await
        .expect("startup recovery must release the old owner and OS lock");
    let reopened_names = reopened
        .list()
        .await
        .expect("read reopened new current")
        .into_iter()
        .map(|row| row.name)
        .collect::<Vec<_>>();
    assert_eq!(reopened_names, vec!["restored-new-current"]);

    let new_owner_locked_before_stale_drop =
        run_store_bound_restore_lock_child(&root, "expect-locked");
    assert_store_bound_restore_lock_child(
        &new_owner_locked_before_stale_drop,
        "the fresh parent owner must exclude child processes while a stale old clone remains",
    );
    drop(stale);
    let new_owner_locked_after_stale_drop =
        run_store_bound_restore_lock_child(&root, "expect-locked");
    assert_store_bound_restore_lock_child(
        &new_owner_locked_after_stale_drop,
        "dropping the stale old clone must not release the fresh parent's process lock",
    );
    let third_open = Store::open_local(StoreOpenOptions::for_root(&root))
        .await
        .expect_err("dropping the stale old owner must not remove the new owner's registry entry");
    assert_eq!(third_open.kind(), StoreOpenErrorKind::AlreadyLocked);
    reopened.pool().close().await;
    drop(reopened);
}

#[cfg(target_os = "linux")]
#[tokio::test(flavor = "current_thread")]
async fn closed_current_snapshot_copy_rejects_a_leaf_exchange_without_competitor_data_io() {
    use std::os::unix::fs::MetadataExt as _;

    const PASSPHRASE: &str = "T130-current-snapshot-copy-binding-passphrase";

    assert_current_snapshot_copy_seam_source_contract();

    let source = TestWorkspace::new().expect("restore source workspace");
    let archive = export_single_data_source_archive(
        &source,
        "current-snapshot-copy-binding",
        "replacement-after-current-copy",
        PASSPHRASE,
    )
    .await;

    let competitor_source = TestWorkspace::new().expect("competitor source workspace");
    let competitor_store = Store::open_local(StoreOpenOptions::new(
        competitor_source.database_path(),
        competitor_source.plugin_root(),
    ))
    .await
    .expect("open valid competitor Store");
    competitor_store
        .create(
            "competitor-must-never-be-copied",
            "127.0.0.1",
            3306,
            "competitor-user",
            b"competitor-password",
        )
        .await
        .expect("seed distinguishable competitor database");
    competitor_store.pool().close().await;
    drop(competitor_store);

    let target = TestWorkspace::new().expect("restore target workspace");
    let root = target.root().join("current-snapshot-copy-binding-root");
    fs::create_dir_all(&root).expect("create restore target root");
    let current_database = root.join("datasources.db");
    let current_plugins = root.join("plugins");
    let current = Store::open_local(StoreOpenOptions::for_root(&root))
        .await
        .expect("open owner-aware current Store");
    current
        .create(
            "old-current-must-survive-copy-exchange",
            "127.0.0.1",
            3307,
            "old-user",
            b"old-password",
        )
        .await
        .expect("seed old current state");
    let stale = current.clone();
    let coordinator =
        RestoreCoordinator::from_store(current).expect("bind restore to exact current Store");
    let prepared = coordinator
        .preview_restore(&archive, PASSPHRASE)
        .await
        .expect("prepare authenticated unarmed replacement");
    let operation_id = prepared.db_instance_operation_id().to_owned();
    let safety_snapshot = prepared.safety_backup_path().to_path_buf();
    let live = root
        .join(".hivegui-db-staging-v1")
        .join(format!("restore-{operation_id}"));
    let staging_database = live.join("datasources.db");
    let staging_plugins = live.join("plugins");
    let staging_database_metadata =
        fs::metadata(&staging_database).expect("record staged database identity before finish");
    let staging_database_identity = (
        staging_database_metadata.dev(),
        staging_database_metadata.ino(),
    );
    let current_plugins_metadata =
        fs::metadata(&current_plugins).expect("record old Plugin root identity");
    let current_plugins_identity = (
        current_plugins_metadata.dev(),
        current_plugins_metadata.ino(),
    );
    let staging_plugins_metadata =
        fs::metadata(&staging_plugins).expect("record staged Plugin root identity");
    let staging_plugins_identity = (
        staging_plugins_metadata.dev(),
        staging_plugins_metadata.ino(),
    );

    let competitor = root.join("current-copy-competitor.db");
    fs::copy(competitor_source.database_path(), &competitor)
        .expect("copy a valid sibling competitor database");
    let competitor_sha256_before = sha256_path(&competitor);
    let competitor_metadata = fs::metadata(&competitor).expect("record competitor inode identity");
    let competitor_identity = (competitor_metadata.dev(), competitor_metadata.ino());

    let confirmation = coordinator
        .begin_confirmation(&prepared)
        .expect("synchronously close the Store and transfer its owner");
    assert!(
        stale.pool().is_closed(),
        "confirmation must terminal-close the shared Store before offline checkpoint/copy"
    );

    // This future test interlock owns two architecture-neutral, two-party
    // barriers. Production and tests must share the same private finish
    // implementation. The private hook may only wait at this exact boundary:
    //
    //   0 = the closed-current checkpoint completed and a held current-file
    //       descriptor was identity-matched to the canonical leaf; the ordinary
    //       safety-snapshot database copy has not started;
    //   1 = the test releases that same production copy path after exchanging
    //       the canonical current leaf with a pre-watched competitor.
    //
    // The hook must not receive a path, choose a file, perform validation, or
    // bypass the normal safety-snapshot copy.
    let copy_barriers: [std::sync::Arc<tokio::sync::Barrier>; 2] =
        std::array::from_fn(|_| std::sync::Arc::new(tokio::sync::Barrier::new(2)));
    let finish_barriers = copy_barriers.clone();
    let mut finish_task = tokio::spawn(async move {
        confirmation
            .finish_with_current_snapshot_copy_interlock_for_test(finish_barriers)
            .await
    });

    tokio::time::timeout(Duration::from_secs(10), copy_barriers[0].wait())
        .await
        .expect("reach the post-checkpoint, post-identity, pre-copy boundary");
    for suffix in ["-wal", "-shm", "-journal"] {
        assert!(
            !PathBuf::from(format!("{}{suffix}", current_database.display())).exists(),
            "barrier 0 requires the closed current sidecars to be fully converged: {suffix}"
        );
    }
    assert_eq!(
        data_source_names_from_database(&current_database).await,
        vec!["old-current-must-survive-copy-exchange"],
        "barrier 0 requires the immutable checkpointed main file to contain the complete old state"
    );
    let current_metadata =
        fs::metadata(&current_database).expect("record pinned old-current identity at boundary");
    let current_identity = (current_metadata.dev(), current_metadata.ino());
    assert!(
        open_linux_fd_count_for_identity(current_identity) >= 1,
        "barrier 0 requires production to retain an open descriptor for the exact current dev/inode"
    );
    let current_sha256_after_checkpoint = sha256_path(&current_database);
    assert_ne!(
        current_sha256_after_checkpoint, competitor_sha256_before,
        "old current and competitor fixtures must remain distinguishable"
    );

    let competitor_watch = DataIoWatch::new(&competitor);
    assert!(
        competitor_watch.drain_masks().is_empty(),
        "installing the competitor inode watch must perform no data I/O"
    );
    let exchange = PathExchangeGuard::new(&current_database, &competitor);
    let exchanged_current =
        fs::metadata(&current_database).expect("read exchanged canonical current identity");
    assert_eq!(
        (exchanged_current.dev(), exchanged_current.ino()),
        competitor_identity,
        "the canonical current path must resolve to the pre-watched competitor"
    );
    copy_barriers[1].wait().await;

    let finish_outcome = tokio::time::timeout(Duration::from_secs(10), &mut finish_task)
        .await
        .expect("finish must fail closed after releasing the ordinary-copy boundary")
        .expect("finish task must not panic");
    let competitor_data_io_masks = competitor_watch.drain_masks();
    drop(competitor_watch);
    let competitor_sha256_after = fs::read(&current_database)
        .ok()
        .map(|bytes| hex::encode(Sha256::digest(bytes)));
    let rejected_for_current_identity = matches!(
        &finish_outcome,
        Err(ImportError::UnsafeArchiveEntry(reason))
            if reason == "current database identity changed"
    );
    let finish_diagnostic = finish_outcome
        .as_ref()
        .err()
        .map(ToString::to_string)
        .unwrap_or_else(|| "restore unexpectedly succeeded".into());

    let instance_manifest_unarmed = fs::read(live.join(".hivegui-db-instance-v1.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .is_some_and(|manifest| manifest["ownership_state"] == "unarmed");
    let staging_database_unchanged = fs::metadata(&staging_database)
        .is_ok_and(|metadata| (metadata.dev(), metadata.ino()) == staging_database_identity);
    let current_plugins_unchanged = fs::metadata(&current_plugins)
        .is_ok_and(|metadata| (metadata.dev(), metadata.ino()) == current_plugins_identity);
    let staging_plugins_unchanged = fs::metadata(&staging_plugins)
        .is_ok_and(|metadata| (metadata.dev(), metadata.ino()) == staging_plugins_identity);
    let exchanged_leaf_pair_unchanged = fs::metadata(&current_database)
        .is_ok_and(|metadata| (metadata.dev(), metadata.ino()) == competitor_identity)
        && fs::metadata(&competitor)
            .is_ok_and(|metadata| (metadata.dev(), metadata.ino()) == current_identity);
    let owner_absent = !live.join(".hivegui-db-recovery-v1.json").exists()
        && !live.join(".hivegui-db-recovery-v1.json.staging").exists();
    let safety_database = safety_snapshot.join("datasources.db");
    let safety_is_absent_or_exact_old = !safety_snapshot.exists()
        || safety_snapshot.join("manifest.json").is_file()
            && safety_database.is_file()
            && sha256_path(&safety_database) == current_sha256_after_checkpoint;
    let terminal_probe = sqlx::query_scalar::<_, i64>("SELECT 1")
        .fetch_one(stale.pool())
        .await;
    let terminal_probe_is_closed = matches!(&terminal_probe, Err(sqlx::Error::PoolClosed));

    assert_eq!(
        (
            rejected_for_current_identity,
            competitor_data_io_masks.is_empty(),
            competitor_sha256_after.as_deref() == Some(competitor_sha256_before.as_str()),
            exchanged_leaf_pair_unchanged,
            instance_manifest_unarmed,
            owner_absent,
            staging_database_unchanged,
            current_plugins_unchanged,
            staging_plugins_unchanged,
            safety_is_absent_or_exact_old,
            stale.pool().is_closed(),
            terminal_probe_is_closed,
        ),
        (
            true, true, true, true, true, true, true, true, true, true, true, true
        ),
        "post-checkpoint current-leaf exchange must reject before competitor data I/O, owner publication, or database/Plugin switch; finish={finish_diagnostic}; competitor_masks={competitor_data_io_masks:?}; terminal_probe={terminal_probe:?}"
    );

    let terminal_locked = run_store_bound_restore_lock_child(&root, "expect-locked");
    assert_store_bound_restore_lock_child(
        &terminal_locked,
        "fail-closed current-copy rejection must retain the Store process lock until startup replay",
    );
    exchange.restore();
    assert_eq!(
        sha256_path(&current_database),
        current_sha256_after_checkpoint,
        "swapping back must restore the exact checkpointed old current"
    );
    drop(prepared);
    let recovered = coordinator
        .recover_startup()
        .await
        .expect("startup replay retires the unarmed instance and releases the old owner");
    assert_eq!(
        recovered.retirement_outcome(),
        RetirementOutcome::AbortedPreSwitch
    );
    assert!(recovered.store_may_open() && recovered.write_gate_is_open());
    let recovery_open = run_store_bound_restore_lock_child(&root, "expect-open");
    assert_store_bound_restore_lock_child(
        &recovery_open,
        "successful startup replay must release the current-copy failure process lock",
    );
    let reopened = Store::open_local(StoreOpenOptions::for_root(&root))
        .await
        .expect("reopen old current after fail-closed startup replay");
    let reopened_names = reopened
        .list()
        .await
        .expect("read preserved old current")
        .into_iter()
        .map(|row| row.name)
        .collect::<Vec<_>>();
    assert_eq!(
        reopened_names,
        vec!["old-current-must-survive-copy-exchange"]
    );
    reopened.pool().close().await;
    drop(reopened);
    drop(stale);
}

mod t119_leaf_owner_red_contracts {
    const BACKUP_SOURCE: &str = include_str!("../src/datasource/backup.rs");

    fn normalized_source() -> String {
        BACKUP_SOURCE
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn item_body<'a>(source: &'a str, marker: &str) -> &'a str {
        let start = source
            .find(marker)
            .unwrap_or_else(|| panic!("missing production boundary: {marker}"));
        let body_start = source[start..]
            .find('{')
            .map(|offset| start + offset)
            .unwrap_or_else(|| panic!("missing production body: {marker}"));
        let mut depth = 0_u32;
        for (offset, ch) in source[body_start..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return &source[start..=body_start + offset];
                    }
                }
                _ => {}
            }
        }
        panic!("unterminated production body: {marker}");
    }

    #[test]
    fn t119_sqlx_checkpoint_and_staging_share_one_bound_leaf_vfs() {
        let source = normalized_source();
        assert!(
            source.contains("struct BoundSqliteLeaf"),
            "T119 Red: production needs an owned BoundSqliteLeaf that pins the exact database object before SQLx checkpoint/open"
        );
        assert!(
            source.contains("struct BoundSqliteLeafVfs")
                && source.contains("fn checkpoint_bound_sqlite_leaf(")
                && source.contains(".vfs("),
            "T119 Red: current and staging SQLx connections must use one registered leaf-bound VFS instead of reopening an ambient path"
        );
        let checkpoint = item_body(&source, "fn checkpoint_bound_sqlite_leaf(");
        assert!(
            checkpoint.contains("leaf: &BoundSqliteLeaf")
                && !checkpoint.contains("database_path: &Path")
                && !checkpoint.contains("filename("),
            "T119 Red: checkpoint must consume the held leaf and may not select a database by path: {checkpoint}"
        );
    }

    #[test]
    fn t119_plugin_switch_rollback_and_cleanup_are_descriptor_bound() {
        let source = normalized_source();
        let copy = item_body(&source, "fn copy_plugin_tree_with_descriptors(");
        assert!(
            copy.contains("source_directory: &cap_std::fs::Dir")
                && !copy.contains("source: &Path")
                && !copy.contains("fs::read_dir(")
                && !copy.contains("fs::symlink_metadata("),
            "T119 Red: Plugin snapshot traversal must consume a held source directory and relative names only: {copy}"
        );
        let cleanup = item_body(&source, "fn remove_tree_no_follow(");
        assert!(
            cleanup.contains("root_directory: &cap_std::fs::Dir")
                && !cleanup.contains("root: &Path")
                && !cleanup.contains("fs::read_dir("),
            "T119 Red: rollback/cleanup must retain a root descriptor instead of reopening ambient paths: {cleanup}"
        );
    }

    #[test]
    fn t119_windows_identity_rejects_reparse_and_aba_replacement() {
        let source = normalized_source();
        assert!(
            source.contains("struct WindowsFileIdentity")
                && source.contains("FILE_ID_INFO")
                && source.contains("GetFileInformationByHandleEx"),
            "T119 Red: Windows identity must bind volume/file ID from the already-open handle"
        );
        assert!(
            source.contains("FILE_FLAG_OPEN_REPARSE_POINT")
                && source.contains("FSCTL_GET_REPARSE_POINT"),
            "T119 Red: Windows opens must inspect and reject reparse points without following them"
        );
    }

    #[test]
    fn t119_non_unix_nofollow_uses_one_owned_leaf_api() {
        let source = normalized_source();
        assert!(
            source.contains("struct NoFollowLeaf")
                && source.contains("fn open_no_follow_leaf_at(")
                && source.contains("#[cfg(not(unix))]"),
            "T119 Red: non-Unix backup, staging, Plugin, rollback, and cleanup paths need one owned no-follow leaf API"
        );
        let open = item_body(&source, "fn open_no_follow_leaf_at(");
        assert!(
            open.contains("root: &cap_std::fs::Dir")
                && open.contains("relative: &Path")
                && open.contains("Result<NoFollowLeaf")
                && !open.contains("canonicalize("),
            "T119 Red: the common no-follow API must resolve a relative leaf beneath an already-open root: {open}"
        );
    }
}

#[cfg(target_os = "linux")]
#[tokio::test(flavor = "current_thread")]
async fn verified_safety_snapshot_current_exchange_is_owner_bound_before_rejection() {
    use std::os::unix::fs::MetadataExt as _;

    const PASSPHRASE: &str = "T130-post-safety-current-binding-passphrase";
    const OLD_ARTIFACT: &str = "post-safety-old/1.0.0/plugin.wasm";
    const OLD_WASM: &[u8] = b"\0asmT130-post-safety-old-plugin";
    const NEW_ARTIFACT: &str = "post-safety-new/1.0.0/plugin.wasm";
    const NEW_WASM: &[u8] = b"\0asmT130-post-safety-new-plugin";

    assert_current_snapshot_copy_seam_source_contract();

    let source = TestWorkspace::new().expect("replacement source workspace");
    let archive = export_data_source_and_plugin_archive(
        &source,
        "post-safety-current-binding",
        "new-must-not-switch-after-post-safety-exchange",
        "post-safety-new",
        NEW_ARTIFACT,
        NEW_WASM,
        PASSPHRASE,
    )
    .await;

    let competitor_source = TestWorkspace::new().expect("competitor source workspace");
    let competitor_store = Store::open_local(StoreOpenOptions::new(
        competitor_source.database_path(),
        competitor_source.plugin_root(),
    ))
    .await
    .expect("open competitor Store");
    competitor_store
        .create(
            "competitor-must-not-be-read-post-safety",
            "127.0.0.1",
            3308,
            "competitor-user",
            b"competitor-password",
        )
        .await
        .expect("seed competitor database");
    competitor_store.pool().close().await;
    drop(competitor_store);

    let target = TestWorkspace::new().expect("restore target workspace");
    let root = target.root().join("post-safety-current-binding-root");
    fs::create_dir_all(&root).expect("create restore root");
    let current_database = root.join("datasources.db");
    let current_plugins = root.join("plugins");
    let current = Store::open_local(StoreOpenOptions::for_root(&root))
        .await
        .expect("open owner-aware current Store");
    seed_data_source_and_plugin_fixture(
        &current,
        &current_plugins,
        "old-must-survive-post-safety-exchange",
        "post-safety-old",
        OLD_ARTIFACT,
        OLD_WASM,
    )
    .await;
    let stale = current.clone();
    let coordinator =
        RestoreCoordinator::from_store(current).expect("bind restore to exact current Store");
    let prepared = coordinator
        .preview_restore(&archive, PASSPHRASE)
        .await
        .expect("prepare authenticated unarmed replacement");
    let operation_id = prepared.db_instance_operation_id().to_owned();
    let safety_snapshot = prepared.safety_backup_path().to_path_buf();
    let live = root
        .join(".hivegui-db-staging-v1")
        .join(format!("restore-{operation_id}"));
    let staging_database = live.join("datasources.db");
    let staging_metadata =
        fs::metadata(&staging_database).expect("record staged replacement identity");
    let staging_identity = (staging_metadata.dev(), staging_metadata.ino());
    let staging_sha256_before = sha256_path(&staging_database);
    let staging_identity_evidence = sidecar_file_identity(&staging_database);
    let staging_size_bytes = staging_metadata.len();
    let staging_plugins = live.join("plugins");
    let staging_plugins_metadata =
        fs::metadata(&staging_plugins).expect("record staged Plugin root identity");
    let staging_plugins_identity = (
        staging_plugins_metadata.dev(),
        staging_plugins_metadata.ino(),
    );
    let staging_plugin_tree_before = regular_tree_bytes(&staging_plugins);
    let instance_manifest_path = live.join(".hivegui-db-instance-v1.json");
    let instance_manifest_before =
        fs::read(&instance_manifest_path).expect("record exact unarmed instance manifest");
    let current_plugins_metadata =
        fs::metadata(&current_plugins).expect("record current Plugin root identity");
    let current_plugins_identity = (
        current_plugins_metadata.dev(),
        current_plugins_metadata.ino(),
    );

    let competitor = root.join("post-safety-current-competitor.db");
    fs::copy(competitor_source.database_path(), &competitor)
        .expect("copy distinguishable sibling competitor");
    let competitor_sha256_before = sha256_path(&competitor);
    let competitor_metadata = fs::metadata(&competitor).expect("record competitor identity");
    let competitor_identity = (competitor_metadata.dev(), competitor_metadata.ino());

    let confirmation = coordinator
        .begin_confirmation(&prepared)
        .expect("freeze current and transfer exact Store owner");
    assert!(stale.pool().is_closed());
    let barriers: [std::sync::Arc<tokio::sync::Barrier>; 4] =
        std::array::from_fn(|_| std::sync::Arc::new(tokio::sync::Barrier::new(2)));
    let finish_barriers = barriers.clone();
    let mut finish_task = tokio::spawn(async move {
        finish_with_snapshot_binding_interlocks(confirmation, finish_barriers).await
    });

    // Barriers 0/1 are inside the ordinary safety-copy path after the newly
    // created snapshot directory's final dirfd/identity match and before its
    // first dirfd-relative database target create.
    tokio::time::timeout(Duration::from_secs(10), barriers[0].wait())
        .await
        .expect("reach pinned fresh-snapshot-directory boundary");
    assert!(safety_snapshot.is_dir());
    assert!(!safety_snapshot.join("datasources.db").exists());
    barriers[1].wait().await;

    // Barriers 2/3 are after the complete DB+Plugin+manifest snapshot verify
    // and the last held-current/canonical match, but before manifest arm.
    // Releasing them must enter the normal armed -> prepared -> applying
    // state machine; it may not use another path-only pre-move check.
    tokio::time::timeout(Duration::from_secs(10), barriers[2].wait())
        .await
        .expect("reach final held-current match before durable arm/owner evidence");
    let current_metadata =
        fs::metadata(&current_database).expect("record verified old-current identity");
    let current_identity = (current_metadata.dev(), current_metadata.ino());
    assert!(
        open_linux_fd_count_for_identity(current_identity) >= 1,
        "the exact old current File must remain held through the pre-arm boundary"
    );
    let current_identity_evidence = sidecar_file_identity(&current_database);
    let current_size_bytes = current_metadata.len();
    let current_sha256 = sha256_path(&current_database);
    assert_eq!(
        data_source_names_from_database(&safety_snapshot.join("datasources.db")).await,
        vec!["old-must-survive-post-safety-exchange"]
    );
    assert_eq!(
        sha256_path(&safety_snapshot.join("datasources.db")),
        current_sha256
    );
    assert_eq!(
        fs::read(safety_snapshot.join("plugins").join(OLD_ARTIFACT))
            .expect("read fully copied old Plugin artifact"),
        OLD_WASM
    );
    assert!(safety_snapshot.join("manifest.json").is_file());
    let safety_snapshot_identity = sidecar_file_identity(&safety_snapshot);
    assert!(
        !live.join(".hivegui-db-recovery-v1.json").exists(),
        "the verified-safety seam must precede owner publication"
    );
    let instance_unarmed = fs::read(live.join(".hivegui-db-instance-v1.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .is_some_and(|manifest| manifest["ownership_state"] == "unarmed");
    assert!(instance_unarmed);

    let exchange = PathExchangeGuard::new(&current_database, &competitor);
    assert_eq!(
        fs::metadata(&current_database)
            .map(|metadata| (metadata.dev(), metadata.ino()))
            .expect("read exchanged canonical identity"),
        competitor_identity
    );
    // Install the watch only after the test's exchange. A legitimate Green
    // must really move B under durable owner=applying, observe its moved inode
    // mismatch, and return that same inode to canonical current without data
    // I/O. A path-only metadata recheck cannot synthesize IN_MOVE_SELF.
    let competitor_watch = DataIoWatch::with_mask(
        &current_database,
        libc::IN_OPEN
            | libc::IN_ACCESS
            | libc::IN_MODIFY
            | libc::IN_CLOSE_WRITE
            | libc::IN_MOVE_SELF,
    );
    assert!(competitor_watch.drain_masks().is_empty());
    barriers[3].wait().await;

    let finish_outcome = tokio::time::timeout(Duration::from_secs(10), &mut finish_task)
        .await
        .expect("post-safety exchange must fail through owner-protected move/compare")
        .expect("finish task must not panic");
    let competitor_masks = competitor_watch.drain_masks();
    drop(competitor_watch);
    let competitor_sha256_after = sha256_path(&current_database);
    let competitor_was_moved = competitor_masks
        .iter()
        .any(|mask| mask & libc::IN_MOVE_SELF != 0);
    let competitor_data_io_absent = competitor_masks.iter().all(|mask| {
        mask & (libc::IN_OPEN | libc::IN_ACCESS | libc::IN_MODIFY | libc::IN_CLOSE_WRITE) == 0
    });
    let exact_identity_rejection = matches!(
        &finish_outcome,
        Err(ImportError::UnsafeArchiveEntry(reason))
            if reason == "current database identity changed"
    );
    let owner_path = live.join(".hivegui-db-recovery-v1.json");
    let owner = fs::read(&owner_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
    let owner_staging_absent = !live.join(".hivegui-db-recovery-v1.json.staging").exists();
    let mut expected_armed_manifest: serde_json::Value =
        serde_json::from_slice(&instance_manifest_before).expect("parse unarmed instance manifest");
    expected_armed_manifest["ownership_state"] = serde_json::Value::String("armed".into());
    let instance_armed_exact = fs::read(&instance_manifest_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .is_some_and(|manifest| manifest == expected_armed_manifest);
    let safety_manifest: serde_json::Value = fs::read(safety_snapshot.join("manifest.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .expect("parse verified safety manifest");
    let owner_uses_held_safety_evidence = owner.as_ref().is_some_and(|owner| {
        owner["phase"] == "applying"
            && owner["ownership_state"] == "armed"
            && owner["old_database"]["relative_path"] == "datasources.db"
            && owner["old_database"]["identity"] == current_identity_evidence
            && owner["old_database"]["identity"] == safety_manifest["database"]["source_identity"]
            && owner["old_database"]["size_bytes"].as_u64() == Some(current_size_bytes)
            && owner["old_database"]["size_bytes"] == safety_manifest["database"]["size_bytes"]
            && owner["old_database"]["sha256"] == current_sha256
            && owner["old_database"]["sha256"] == safety_manifest["database"]["sha256"]
            && owner["new_database"]["identity"] == staging_identity_evidence
            && owner["new_database"]["size_bytes"].as_u64() == Some(staging_size_bytes)
            && owner["new_database"]["sha256"] == staging_sha256_before
            && owner["safety_snapshot"]["identity"] == safety_snapshot_identity
            && owner["safety_snapshot"]["relative_path"]
                == format!("backups/restore-safety-{operation_id}")
            && owner["safety_snapshot"]["sha256"]
                .as_str()
                .is_some_and(|hash| !hash.is_empty())
    });
    let staged_unchanged = fs::metadata(&staging_database)
        .is_ok_and(|metadata| (metadata.dev(), metadata.ino()) == staging_identity)
        && sha256_path(&staging_database) == staging_sha256_before
        && fs::metadata(&staging_plugins)
            .is_ok_and(|metadata| (metadata.dev(), metadata.ino()) == staging_plugins_identity)
        && regular_tree_bytes(&staging_plugins) == staging_plugin_tree_before;
    let current_plugins_unchanged = fs::metadata(&current_plugins)
        .is_ok_and(|metadata| (metadata.dev(), metadata.ino()) == current_plugins_identity)
        && fs::read(current_plugins.join(OLD_ARTIFACT)).is_ok_and(|bytes| bytes == OLD_WASM);
    let exchanged_pair_unchanged = fs::metadata(&current_database)
        .is_ok_and(|metadata| (metadata.dev(), metadata.ino()) == competitor_identity)
        && fs::metadata(&competitor)
            .is_ok_and(|metadata| (metadata.dev(), metadata.ino()) == current_identity);
    let owner_old_slot_absent = !live.join("old-datasources.db").exists();
    let safety_still_complete = safety_snapshot.join("manifest.json").is_file()
        && sha256_path(&safety_snapshot.join("datasources.db")) == current_sha256
        && fs::read(safety_snapshot.join("plugins").join(OLD_ARTIFACT))
            .is_ok_and(|bytes| bytes == OLD_WASM);
    assert_eq!(
        [
            exact_identity_rejection,
            competitor_was_moved,
            competitor_data_io_absent,
            competitor_sha256_after == competitor_sha256_before,
            owner_uses_held_safety_evidence,
            owner_staging_absent,
            instance_armed_exact,
            staged_unchanged,
            current_plugins_unchanged,
            exchanged_pair_unchanged,
            owner_old_slot_absent,
            safety_still_complete,
            stale.pool().is_closed(),
        ],
        [true; 13],
        "post-safety current exchange must be rejected only after durable owner applying and a real identity-bound move/rollback, with zero competitor data I/O; outcome={finish_outcome:?}; owner={owner:?}; competitor_masks={competitor_masks:?}"
    );

    let terminal_locked = run_store_bound_restore_lock_child(&root, "expect-locked");
    assert_store_bound_restore_lock_child(
        &terminal_locked,
        "post-safety identity failure must retain the Store OS lock",
    );
    exchange.restore();
    assert_eq!(sha256_path(&current_database), current_sha256);
    drop(prepared);
    let recovered = coordinator
        .recover_startup()
        .await
        .expect("startup owner replay restores and retires exact old current");
    assert_eq!(recovered.retirement_outcome(), RetirementOutcome::Old);
    assert!(recovered.retirement_is_done());
    assert!(recovered.write_gate_is_open());
    assert!(recovered.store_may_open());
    assert!(
        !live.exists(),
        "old retirement must consume the live owner instance"
    );
    let recovery_open = run_store_bound_restore_lock_child(&root, "expect-open");
    assert_store_bound_restore_lock_child(
        &recovery_open,
        "successful applying-owner replay must release the Store OS lock",
    );
    let reopened = Store::open_local(StoreOpenOptions::for_root(&root))
        .await
        .expect("reopen exact old current after startup replay");
    assert_eq!(
        reopened
            .list()
            .await
            .expect("read recovered old current")
            .into_iter()
            .map(|row| row.name)
            .collect::<Vec<_>>(),
        vec!["old-must-survive-post-safety-exchange"]
    );
    reopened.pool().close().await;
    drop(reopened);
    drop(stale);
}

#[cfg(target_os = "linux")]
#[tokio::test(flavor = "current_thread")]
async fn snapshot_directory_exchange_never_touches_or_deletes_the_foreign_tree() {
    use std::os::unix::fs::MetadataExt as _;

    const PASSPHRASE: &str = "T130-snapshot-directory-binding-passphrase";
    const OLD_ARTIFACT: &str = "snapshot-dir-old/1.0.0/plugin.wasm";
    const OLD_WASM: &[u8] = b"\0asmT130-snapshot-dir-old-plugin";

    assert_current_snapshot_copy_seam_source_contract();

    let source = TestWorkspace::new().expect("replacement source workspace");
    let archive = export_single_data_source_archive(
        &source,
        "snapshot-directory-binding",
        "new-must-not-enter-foreign-snapshot-tree",
        PASSPHRASE,
    )
    .await;

    let target = TestWorkspace::new().expect("restore target workspace");
    let root = target.root().join("snapshot-directory-binding-root");
    fs::create_dir_all(&root).expect("create restore root");
    let current_database = root.join("datasources.db");
    let current_plugins = root.join("plugins");
    let current = Store::open_local(StoreOpenOptions::for_root(&root))
        .await
        .expect("open owner-aware current Store");
    seed_data_source_and_plugin_fixture(
        &current,
        &current_plugins,
        "old-must-survive-snapshot-directory-exchange",
        "snapshot-dir-old",
        OLD_ARTIFACT,
        OLD_WASM,
    )
    .await;
    let stale = current.clone();
    let coordinator =
        RestoreCoordinator::from_store(current).expect("bind restore to exact current Store");
    let prepared = coordinator
        .preview_restore(&archive, PASSPHRASE)
        .await
        .expect("prepare authenticated unarmed replacement");
    let operation_id = prepared.db_instance_operation_id().to_owned();
    let safety_snapshot = prepared.safety_backup_path().to_path_buf();
    let snapshots = safety_snapshot
        .parent()
        .expect("safety snapshot parent")
        .to_path_buf();
    fs::create_dir_all(&snapshots).expect("create controlled backups directory");
    let foreign = snapshots.join("foreign-regular-directory-tree");
    let foreign_nested = foreign.join("nested");
    fs::create_dir_all(&foreign_nested).expect("create foreign sibling directory tree");
    fs::write(
        foreign.join("root-canary.bin"),
        b"T130-foreign-root-canary-must-not-change",
    )
    .expect("write foreign root canary");
    let foreign_nested_canary = foreign_nested.join("nested-canary.bin");
    fs::write(
        &foreign_nested_canary,
        b"T130-foreign-nested-canary-must-not-change",
    )
    .expect("write foreign nested canary");
    let foreign_tree_before = regular_tree_bytes(&foreign);
    let foreign_metadata = fs::metadata(&foreign).expect("record foreign directory identity");
    let foreign_identity = (foreign_metadata.dev(), foreign_metadata.ino());

    let live = root
        .join(".hivegui-db-staging-v1")
        .join(format!("restore-{operation_id}"));
    let staging_database = live.join("datasources.db");
    let staging_metadata =
        fs::metadata(&staging_database).expect("record staged replacement identity");
    let staging_identity = (staging_metadata.dev(), staging_metadata.ino());
    let staging_sha256_before = sha256_path(&staging_database);
    let staging_plugins = live.join("plugins");
    let staging_plugins_metadata =
        fs::metadata(&staging_plugins).expect("record staged Plugin root identity");
    let staging_plugins_identity = (
        staging_plugins_metadata.dev(),
        staging_plugins_metadata.ino(),
    );
    let staging_plugin_tree_before = regular_tree_bytes(&staging_plugins);
    let instance_manifest_path = live.join(".hivegui-db-instance-v1.json");
    let instance_manifest_before =
        fs::read(&instance_manifest_path).expect("record exact unarmed instance manifest");

    let confirmation = coordinator
        .begin_confirmation(&prepared)
        .expect("freeze current and transfer exact Store owner");
    assert!(stale.pool().is_closed());
    let barriers: [std::sync::Arc<tokio::sync::Barrier>; 4] =
        std::array::from_fn(|_| std::sync::Arc::new(tokio::sync::Barrier::new(2)));
    let finish_barriers = barriers.clone();
    let mut finish_task = tokio::spawn(async move {
        finish_with_snapshot_binding_interlocks(confirmation, finish_barriers).await
    });

    // This seam is inside the ordinary full snapshot builder: after the final
    // held-dirfd/canonical identity match, immediately before the first
    // dirfd-relative database target create. After release, database, Plugin
    // tree, manifest, sync, verification, and exact-object cleanup must all
    // stay on that same held directory; only the final canonical identity
    // check may observe the exchange.
    tokio::time::timeout(Duration::from_secs(10), barriers[0].wait())
        .await
        .expect("reach fresh pinned snapshot directory before DB target creation");
    let created_metadata =
        fs::metadata(&safety_snapshot).expect("record exact created snapshot directory identity");
    let created_identity = (created_metadata.dev(), created_metadata.ino());
    assert!(created_metadata.is_dir());
    assert!(!safety_snapshot.join("datasources.db").exists());
    assert!(!safety_snapshot.join("manifest.json").exists());
    assert!(!safety_snapshot.join("plugins").exists());
    assert!(
        open_linux_fd_count_for_identity(created_identity) >= 1,
        "production must retain a descriptor for the exact newly-created snapshot directory"
    );
    let current_metadata =
        fs::metadata(&current_database).expect("record post-checkpoint old current identity");
    let current_identity = (current_metadata.dev(), current_metadata.ino());
    let current_sha256 = sha256_path(&current_database);
    assert!(
        open_linux_fd_count_for_identity(current_identity) >= 1,
        "the exact post-checkpoint old current must remain descriptor-pinned during the full safety build"
    );
    let current_plugins_metadata =
        fs::metadata(&current_plugins).expect("record old current Plugin root identity");
    let current_plugins_identity = (
        current_plugins_metadata.dev(),
        current_plugins_metadata.ino(),
    );
    let current_plugin_tree_before = regular_tree_bytes(&current_plugins);

    let foreign_watches = watch_regular_tree(&foreign);
    assert!(
        foreign_watches
            .iter()
            .all(|watch| watch.drain_masks().is_empty()),
        "setup enumeration events must be drained before the exchange"
    );
    let exchange = RestorableDirectoryExchangeGuard::new(&safety_snapshot, &foreign);
    assert_eq!(
        fs::metadata(&safety_snapshot)
            .map(|metadata| (metadata.dev(), metadata.ino()))
            .expect("read exchanged canonical snapshot identity"),
        foreign_identity
    );
    assert_eq!(
        fs::metadata(&foreign)
            .map(|metadata| (metadata.dev(), metadata.ino()))
            .expect("read moved created snapshot identity"),
        created_identity
    );
    barriers[1].wait().await;

    tokio::time::timeout(Duration::from_secs(10), barriers[2].wait())
        .await
        .expect("full held-directory snapshot build must finish before canonical recheck");
    let held_snapshot_database = foreign.join("datasources.db");
    let held_snapshot_manifest = foreign.join("manifest.json");
    assert_eq!(
        data_source_names_from_database(&held_snapshot_database).await,
        vec!["old-must-survive-snapshot-directory-exchange"],
        "the database copy must target the held created directory, not its exchanged canonical path"
    );
    assert_eq!(sha256_path(&held_snapshot_database), current_sha256);
    assert_eq!(
        fs::read(foreign.join("plugins").join(OLD_ARTIFACT))
            .expect("read Plugin canary from held created snapshot directory"),
        OLD_WASM
    );
    let held_manifest: serde_json::Value = serde_json::from_slice(
        &fs::read(&held_snapshot_manifest).expect("read held-directory safety manifest"),
    )
    .expect("parse held-directory safety manifest");
    assert_eq!(held_manifest["database"]["sha256"], current_sha256);
    assert_eq!(held_manifest["database"]["path"], "datasources.db");
    assert_eq!(held_manifest["plugin_root"]["path"], "plugins");
    assert!(
        held_manifest["plugin_root"]["sha256"]
            .as_str()
            .is_some_and(|hash| !hash.is_empty())
    );
    assert!(
        held_manifest["plugin_files"]
            .as_array()
            .is_some_and(|files| files.iter().any(|file| {
                file["path"] == format!("plugins/{OLD_ARTIFACT}")
                    && file["sha256"] == hex::encode(Sha256::digest(OLD_WASM))
            }))
    );
    barriers[3].wait().await;

    let finish_outcome = tokio::time::timeout(Duration::from_secs(10), &mut finish_task)
        .await
        .expect("snapshot directory exchange must fail before the later owner seam")
        .expect("finish task must not panic");
    let foreign_masks = foreign_watches
        .iter()
        .flat_map(DataIoWatch::drain_masks)
        .collect::<Vec<_>>();
    drop(foreign_watches);
    let foreign_tree_after = regular_tree_bytes(&safety_snapshot);
    let exact_identity_rejection = matches!(
        &finish_outcome,
        Err(ImportError::UnsafeArchiveEntry(reason))
            if reason == "restore safety snapshot identity changed"
    );
    let foreign_still_canonical = fs::metadata(&safety_snapshot)
        .is_ok_and(|metadata| (metadata.dev(), metadata.ino()) == foreign_identity);
    let created_exact_object_retained_or_removed = if foreign.exists() {
        fs::metadata(&foreign).is_ok_and(|metadata| {
            metadata.is_dir()
                && !metadata.file_type().is_symlink()
                && (metadata.dev(), metadata.ino()) == created_identity
        })
    } else {
        true
    };
    let owner_absent = !live.join(".hivegui-db-recovery-v1.json").exists()
        && !live.join(".hivegui-db-recovery-v1.json.staging").exists();
    let instance_unarmed = fs::read(live.join(".hivegui-db-instance-v1.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .is_some_and(|manifest| manifest["ownership_state"] == "unarmed");
    let staging_unchanged = fs::metadata(&staging_database)
        .is_ok_and(|metadata| (metadata.dev(), metadata.ino()) == staging_identity)
        && sha256_path(&staging_database) == staging_sha256_before
        && fs::metadata(&staging_plugins)
            .is_ok_and(|metadata| (metadata.dev(), metadata.ino()) == staging_plugins_identity)
        && regular_tree_bytes(&staging_plugins) == staging_plugin_tree_before;
    let control_unchanged =
        fs::read(&instance_manifest_path).is_ok_and(|bytes| bytes == instance_manifest_before);
    let current_unchanged = fs::metadata(&current_database)
        .is_ok_and(|metadata| (metadata.dev(), metadata.ino()) == current_identity)
        && sha256_path(&current_database) == current_sha256
        && fs::metadata(&current_plugins)
            .is_ok_and(|metadata| (metadata.dev(), metadata.ino()) == current_plugins_identity)
        && regular_tree_bytes(&current_plugins) == current_plugin_tree_before;
    assert_eq!(
        (
            exact_identity_rejection,
            foreign_masks.is_empty(),
            foreign_tree_after == foreign_tree_before,
            foreign_still_canonical,
            created_exact_object_retained_or_removed,
            owner_absent,
            instance_unarmed,
            staging_unchanged,
            control_unchanged,
            current_unchanged,
            stale.pool().is_closed(),
        ),
        (
            true, true, true, true, true, true, true, true, true, true, true
        ),
        "foreign directory tree must receive zero I/O/deletion while the full builder remains on its held directory, and no owner/switch may begin; outcome={finish_outcome:?}; foreign_masks={foreign_masks:?}"
    );

    let terminal_locked = run_store_bound_restore_lock_child(&root, "expect-locked");
    assert_store_bound_restore_lock_child(
        &terminal_locked,
        "snapshot-directory binding failure must retain the Store OS lock",
    );
    exchange.restore();
    assert_eq!(
        fs::metadata(&foreign)
            .map(|metadata| (metadata.dev(), metadata.ino()))
            .expect("foreign sibling restored"),
        foreign_identity
    );
    assert_eq!(regular_tree_bytes(&foreign), foreign_tree_before);
    if safety_snapshot.exists() {
        assert_eq!(
            fs::metadata(&safety_snapshot)
                .map(|metadata| (metadata.dev(), metadata.ino()))
                .expect("retained exact created snapshot identity"),
            created_identity
        );
    }
    drop(prepared);
    let recovered = coordinator
        .recover_startup()
        .await
        .expect("startup replay retires the unarmed replacement");
    assert_eq!(
        recovered.retirement_outcome(),
        RetirementOutcome::AbortedPreSwitch
    );
    let recovery_open = run_store_bound_restore_lock_child(&root, "expect-open");
    assert_store_bound_restore_lock_child(
        &recovery_open,
        "successful snapshot-binding replay must release the Store OS lock",
    );
    let reopened = Store::open_local(StoreOpenOptions::for_root(&root))
        .await
        .expect("reopen exact old current after snapshot binding failure");
    assert_eq!(
        reopened
            .list()
            .await
            .expect("read recovered old current")
            .into_iter()
            .map(|row| row.name)
            .collect::<Vec<_>>(),
        vec!["old-must-survive-snapshot-directory-exchange"]
    );
    reopened.pool().close().await;
    drop(reopened);
    drop(stale);
}

#[cfg(target_os = "linux")]
#[tokio::test(flavor = "current_thread")]
async fn store_bound_restore_rejects_archive_exchange_before_freeze_and_cancels_preview() {
    const PASSPHRASE: &str = "T130-store-bound-archive-binding-passphrase";

    let original_source = TestWorkspace::new().expect("original source workspace");
    let original_archive = export_single_data_source_archive(
        &original_source,
        "archive-binding-original",
        "original-archive-state",
        PASSPHRASE,
    )
    .await;
    let replacement_source = TestWorkspace::new().expect("replacement source workspace");
    let replacement_archive = export_single_data_source_archive(
        &replacement_source,
        "archive-binding-replacement",
        "replacement-archive-state",
        PASSPHRASE,
    )
    .await;

    let target = TestWorkspace::new().expect("target workspace");
    let root = target.root().join("store-bound-archive-binding-root");
    fs::create_dir_all(&root).expect("create target root");
    let current = Store::open_local(StoreOpenOptions::for_root(&root))
        .await
        .expect("open current Store");
    current
        .create(
            "old-before-rejected-exchange",
            "127.0.0.1",
            3306,
            "old-user",
            b"old-password",
        )
        .await
        .expect("seed current state");
    let stale = current.clone();
    let coordinator =
        RestoreCoordinator::from_store(current).expect("bind restore to current Store");
    let prepared: PreparedRestore = coordinator
        .preview_restore(&original_archive, PASSPHRASE)
        .await
        .expect("prepare archive-bound preview");
    let safety_snapshot = prepared.safety_backup_path().to_path_buf();
    assert!(!safety_snapshot.exists());

    let exchange = PathExchangeGuard::new(&original_archive, &replacement_archive);
    let error = match coordinator.begin_confirmation(&prepared) {
        Ok(_) => panic!("an exchanged archive leaf must not cross final confirmation"),
        Err(error) => error,
    };
    assert!(matches!(error, ImportError::UnsafeArchiveEntry(_)));
    assert!(
        !stale.pool().is_closed(),
        "archive binding failure must occur before Store freeze"
    );
    stale
        .create(
            "write-after-rejected-exchange",
            "127.0.0.1",
            3307,
            "still-open-user",
            b"still-open-password",
        )
        .await
        .expect("rejected archive exchange leaves current Store writable");
    exchange.restore();

    coordinator
        .cancel_preview(&prepared)
        .await
        .expect("identity rejection must leave the Ready preview cancelable");
    assert!(!safety_snapshot.exists());
    assert_eq!(
        fs::read_dir(root.join(".hivegui-db-staging-v1"))
            .expect("read canceled restore registry")
            .count(),
        0,
        "cancel must retire the complete preview instance"
    );
    let current_names = stale
        .list()
        .await
        .expect("read unchanged current after preview cancellation")
        .into_iter()
        .map(|row| row.name)
        .collect::<Vec<_>>();
    assert_eq!(
        current_names,
        vec![
            "old-before-rejected-exchange",
            "write-after-rejected-exchange"
        ]
    );

    drop(coordinator);
    stale.pool().close().await;
    drop(stale);
}

#[cfg(target_os = "linux")]
#[tokio::test(flavor = "current_thread")]
async fn preview_restore_pins_one_archive_descriptor_across_a_b_a_path_exchange() {
    const PASSPHRASE: &str = "T130-preview-pinned-archive-passphrase";
    const A_ARTIFACT_KEY: &str = "pinned-a/1.0.0/plugin.wasm";
    const A_WASM: &[u8] = b"\0asmT130-preview-pinned-archive-A";
    const B_ARTIFACT_KEY: &str = "pinned-b/1.0.0/plugin.wasm";
    const B_WASM: &[u8] = b"\0asmT130-preview-pinned-archive-B";

    let source_a = TestWorkspace::new().expect("archive A source workspace");
    let archive_a = export_data_source_and_plugin_archive(
        &source_a,
        "preview-pinned-archive-a",
        "pinned-archive-a",
        "pinned-a",
        A_ARTIFACT_KEY,
        A_WASM,
        PASSPHRASE,
    )
    .await;
    let source_b = TestWorkspace::new().expect("archive B source workspace");
    let archive_b = export_data_source_and_plugin_archive(
        &source_b,
        "preview-pinned-archive-b",
        "pinned-archive-b",
        "pinned-b",
        B_ARTIFACT_KEY,
        B_WASM,
        PASSPHRASE,
    )
    .await;
    let archive_a_sha256 = sha256_path(&archive_a);
    let archive_b_sha256 = sha256_path(&archive_b);
    assert_ne!(archive_a_sha256, archive_b_sha256);

    let target = TestWorkspace::new().expect("target workspace");
    let root = target.root().join("preview-pinned-archive-root");
    fs::create_dir_all(&root).expect("create target root");
    let current = Store::open_local(StoreOpenOptions::for_root(&root))
        .await
        .expect("open owner-aware target Store");
    current
        .create(
            "current-before-pinned-preview",
            "127.0.0.1",
            3306,
            "current-user",
            b"current-password",
        )
        .await
        .expect("seed unchanged current state");
    let stale = current.clone();
    let coordinator =
        RestoreCoordinator::from_store(current).expect("bind preview to target Store");

    // The future test interlock owns four two-party barriers:
    //   0 = the initial A descriptor is pinned and identity-bound, before any
    //       authenticated bytes are consumed;
    //   1 = the test releases the complete real preview after exchanging A -> B;
    //   2 = all authenticated bytes were consumed and the staging database/Plugin
    //       tree were completely built and validated, but the final canonical-path
    //       identity recheck and return have not run;
    //   3 = the test releases that final recheck after exchanging B -> A.
    // Production and tests must share the same private preview implementation. This
    // thin hook only waits at those boundaries; it must not select a descriptor,
    // decrypt bytes, materialize staging, or replace any production validation.
    let archive_read_barriers: [std::sync::Arc<tokio::sync::Barrier>; 4] =
        std::array::from_fn(|_| std::sync::Arc::new(tokio::sync::Barrier::new(2)));
    let preview_coordinator = coordinator.clone();
    let preview_archive = archive_a.clone();
    let preview_barriers = archive_read_barriers.clone();
    let mut preview_task = tokio::spawn(async move {
        preview_coordinator
            .preview_restore_with_archive_read_interlock_for_test(
                &preview_archive,
                PASSPHRASE,
                preview_barriers,
            )
            .await
    });

    tokio::time::timeout(Duration::from_secs(10), archive_read_barriers[0].wait())
        .await
        .expect("preview pins and binds archive A before authenticated consumption");
    let exchange = PathExchangeGuard::new(&archive_a, &archive_b);
    assert_eq!(sha256_path(&archive_a), archive_b_sha256);
    archive_read_barriers[1].wait().await;

    let mut early_preview_result = None;
    let staging_complete_interlock_reached = tokio::select! {
        _ = archive_read_barriers[2].wait() => true,
        result = &mut preview_task => {
            early_preview_result = Some(result.expect("preview task must not panic"));
            false
        }
        _ = tokio::time::sleep(Duration::from_secs(10)) => {
            panic!("preview neither completed authenticated staging nor failed closed")
        }
    };
    exchange.restore();
    assert_eq!(
        sha256_path(&archive_a),
        archive_a_sha256,
        "the ambient archive path must be A again before preview completes"
    );
    let preview_result = if staging_complete_interlock_reached {
        archive_read_barriers[3].wait().await;
        tokio::time::timeout(Duration::from_secs(30), &mut preview_task)
            .await
            .expect("bounded preview completion after releasing both barriers")
            .expect("preview task must not panic")
    } else {
        early_preview_result.expect("early fail-closed preview result")
    };
    match preview_result {
        Ok(prepared) => {
            let live = root
                .join(".hivegui-db-staging-v1")
                .join(format!("restore-{}", prepared.db_instance_operation_id()));
            assert_eq!(
                data_source_names_from_database(&live.join("datasources.db")).await,
                vec!["pinned-archive-a"],
                "preview must never stage archive B during an A -> B -> A path exchange"
            );
            assert_eq!(
                fs::read(live.join("plugins").join(A_ARTIFACT_KEY))
                    .expect("read staged archive A Plugin canary"),
                A_WASM
            );
            assert!(
                !live.join("plugins").join(B_ARTIFACT_KEY).exists(),
                "preview must not mix archive B Plugin bytes into the pinned A staging tree"
            );
            coordinator
                .cancel_preview(&prepared)
                .await
                .expect("retire successful unarmed A preview");
        }
        Err(error) => {
            assert!(
                matches!(error, ImportError::UnsafeArchiveEntry(_)),
                "descriptor/path ambiguity must fail through the stable unsafe-entry boundary: {error}"
            );
        }
    }

    let registry = root.join(".hivegui-db-staging-v1");
    assert!(
        !registry.exists()
            || fs::read_dir(&registry)
                .expect("read preview registry")
                .next()
                .is_none(),
        "successful cancellation or fail-closed preview must leave no live/tombstone residue"
    );
    assert!(
        !root.join("backups").exists(),
        "preview ambiguity must not claim a safety backup"
    );
    assert!(!stale.pool().is_closed());
    assert_eq!(
        stale
            .list()
            .await
            .expect("read unchanged current after pinned preview")
            .into_iter()
            .map(|row| row.name)
            .collect::<Vec<_>>(),
        vec!["current-before-pinned-preview"]
    );

    drop(coordinator);
    stale.pool().close().await;
    drop(stale);
}

#[tokio::test(flavor = "current_thread")]
async fn store_bound_restore_owner_applying_crash_replays_old_and_releases_owner() {
    const PASSPHRASE: &str = "T130-store-bound-applying-crash-passphrase";
    const NEW_ARTIFACT_KEY: &str = "new-restore/1.0.0/plugin.wasm";
    const NEW_WASM: &[u8] = b"\0asmT130-owned-new-current-wasm";
    const OLD_ARTIFACT_KEY: &str = "old-restore/1.0.0/plugin.wasm";
    const OLD_WASM: &[u8] = b"\0asmT130-owned-old-current-wasm";

    let source = TestWorkspace::new().expect("source workspace");
    let archive = export_data_source_and_plugin_archive(
        &source,
        "store-bound-applying-crash",
        "new-that-must-not-publish",
        "new-restore",
        NEW_ARTIFACT_KEY,
        NEW_WASM,
        PASSPHRASE,
    )
    .await;

    let target = TestWorkspace::new().expect("target workspace");
    let root = target.root().join("store-bound-applying-crash-root");
    fs::create_dir_all(&root).expect("create target root");
    let current = Store::open_local(StoreOpenOptions::for_root(&root))
        .await
        .expect("open owner-aware old current");
    current
        .create(
            "old-before-owner-applying",
            "127.0.0.1",
            3306,
            "old-user",
            b"old-password",
        )
        .await
        .expect("seed old state");
    let old_artifact = root.join("plugins").join(OLD_ARTIFACT_KEY);
    fs::create_dir_all(old_artifact.parent().expect("old Plugin artifact parent"))
        .expect("create old Plugin artifact parent");
    fs::write(&old_artifact, OLD_WASM).expect("write old managed Plugin artifact");
    sqlx::query(
        "INSERT INTO plugins \
         (identifier,name,version,s3_key,sha256,size_bytes,runtime,created_at,updated_at) \
         VALUES ('old-restore','Old Restore Plugin','1.0.0',?,?,?,'wasm32',\
                 '2026-08-27T00:00:00Z','2026-08-27T00:00:00Z')",
    )
    .bind(OLD_ARTIFACT_KEY)
    .bind(hex::encode(Sha256::digest(OLD_WASM)))
    .bind(i64::try_from(OLD_WASM.len()).expect("old WASM fixture size"))
    .execute(current.pool())
    .await
    .expect("seed old Plugin ownership");
    let stale = current.clone();
    let coordinator =
        RestoreCoordinator::from_store(current).expect("bind restore to current Store");
    let prepared: PreparedRestore = coordinator
        .preview_restore(&archive, PASSPHRASE)
        .await
        .expect("prepare applying-crash preview");
    let operation_id = prepared.db_instance_operation_id().to_owned();
    let safety_snapshot = prepared.safety_backup_path().to_path_buf();
    let confirmation = coordinator
        .begin_confirmation(&prepared)
        .expect("freeze current before applying crash");
    assert!(stale.pool().is_closed());
    let terminal_gate = stale
        .create(
            "must-not-write-during-applying",
            "127.0.0.1",
            3307,
            "blocked-user",
            b"blocked-password",
        )
        .await
        .expect_err("confirmation must close the write gate before owner publication");
    assert!(terminal_gate.to_string().contains("write_gate_closed"));

    let plugin_tree_switch = RESTORE_SWITCH_CRASH_POINTS
        .iter()
        .copied()
        .find(|point| point.as_str() == "plugin_tree_switch")
        .expect("Plugin tree switch crash point");
    let interrupted = confirmation
        .finish_with_crash(plugin_tree_switch)
        .await
        .expect_err("interrupt after database and Plugin current have switched");
    assert!(interrupted.reached_requested_boundary());
    assert!(stale.pool().is_closed());
    let current_database = root.join("datasources.db");
    let current_new_wasm = root.join("plugins").join(NEW_ARTIFACT_KEY);
    let live = root
        .join(".hivegui-db-staging-v1")
        .join(format!("restore-{operation_id}"));
    let live_old_database = live.join("old-datasources.db");
    let live_old_wasm = live.join("old-plugins").join(OLD_ARTIFACT_KEY);
    assert_eq!(
        data_source_names_from_database(&current_database).await,
        vec!["new-that-must-not-publish"],
        "the injected boundary must execute the new database switch"
    );
    assert_eq!(
        fs::read(&current_new_wasm).expect("read switched new Plugin artifact"),
        NEW_WASM,
        "the injected boundary must execute the new Plugin tree switch"
    );
    assert_eq!(
        data_source_names_from_database(&live_old_database).await,
        vec!["old-before-owner-applying"],
        "the live owner must retain the complete old database"
    );
    assert_eq!(
        fs::read(&live_old_wasm).expect("read retained old Plugin artifact"),
        OLD_WASM,
        "the live owner must retain the complete old Plugin tree"
    );
    assert!(safety_snapshot.join("manifest.json").is_file());
    assert_eq!(
        data_source_names_from_database(&safety_snapshot.join("datasources.db")).await,
        vec!["old-before-owner-applying"]
    );
    let owner: serde_json::Value = serde_json::from_slice(
        &fs::read(live.join(".hivegui-db-recovery-v1.json"))
            .expect("applying owner must be durable at the crash boundary"),
    )
    .expect("parse applying owner");
    assert_eq!(owner["phase"], "applying");
    let applying_locked = run_store_bound_restore_lock_child(&root, "expect-locked");
    assert_store_bound_restore_lock_child(
        &applying_locked,
        "an applying crash must retain the process lock before startup replay",
    );

    fs::write(
        &current_database,
        b"T130-deliberately-corrupted-owned-new-current-database",
    )
    .expect("destroy switched new current database");
    fs::write(
        &current_new_wasm,
        b"T130-deliberately-corrupted-owned-new-current-wasm",
    )
    .expect("destroy switched new current Plugin artifact");
    assert_ne!(
        sha256_path(&current_database),
        owner["new_database"]["sha256"]
            .as_str()
            .expect("owner binds new database hash")
    );
    assert_ne!(
        fs::read(&current_new_wasm).expect("read corrupted new WASM"),
        NEW_WASM
    );
    let post_corruption_gate = stale
        .create(
            "must-not-write-after-new-current-corruption",
            "127.0.0.1",
            3308,
            "blocked-user",
            b"blocked-password",
        )
        .await
        .expect_err("the applying crash must retain the write gate after current corruption");
    assert!(
        post_corruption_gate
            .to_string()
            .contains("write_gate_closed")
    );
    assert!(stale.pool().is_closed());

    let recovered = coordinator
        .recover_startup()
        .await
        .expect("startup must restore and verify complete old state");
    assert_eq!(recovered.retirement_outcome(), RetirementOutcome::Old);
    assert!(recovered.retirement_is_done());
    assert!(recovered.store_may_open());
    assert!(recovered.write_gate_is_open());
    assert!(recovered.has_exactly_one_live_database());
    assert!(recovered.has_no_mixed_database_or_plugin_tree());
    assert!(recovered.control_files_are_outside_live_tree());
    assert_eq!(
        fs::read_dir(root.join(".hivegui-db-staging-v1"))
            .expect("read recovered registry")
            .count(),
        0,
        "old retirement must be completely durable before Store open"
    );
    assert_eq!(
        data_source_names_from_database(&current_database).await,
        vec!["old-before-owner-applying"]
    );
    assert_eq!(
        fs::read(root.join("plugins").join(OLD_ARTIFACT_KEY))
            .expect("read startup-restored old Plugin artifact"),
        OLD_WASM
    );
    assert!(
        !root.join("plugins").join(NEW_ARTIFACT_KEY).exists(),
        "startup must not expose a mixed old database/new Plugin tree"
    );
    let recovery_open = run_store_bound_restore_lock_child(&root, "expect-open");
    assert_store_bound_restore_lock_child(
        &recovery_open,
        "startup replay must release the applying crash process lock",
    );
    drop(prepared);
    drop(coordinator);

    let reopened = Store::open_local(StoreOpenOptions::for_root(&root))
        .await
        .expect("startup recovery must release the crashed old owner");
    let names = reopened
        .list()
        .await
        .expect("read recovered old current")
        .into_iter()
        .map(|row| row.name)
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["old-before-owner-applying"]);
    assert_eq!(
        fs::read(root.join("plugins").join(OLD_ARTIFACT_KEY))
            .expect("read old Plugin through the reopened parent state"),
        OLD_WASM
    );
    drop(stale);
    reopened.pool().close().await;
    drop(reopened);
}

#[cfg(target_os = "linux")]
#[tokio::test(flavor = "current_thread")]
async fn store_open_leaf_exchange_before_backup_reopen_rejects_without_competitor_data_io() {
    use std::os::unix::fs::MetadataExt as _;

    let source = TestWorkspace::new().expect("source workspace");
    let source_store = Store::open_local(StoreOpenOptions::new(
        source.database_path(),
        source.plugin_root(),
    ))
    .await
    .expect("open source Store");
    let initial_checkpoint: (i64, i64, i64) = sqlx::query_as("PRAGMA wal_checkpoint(TRUNCATE)")
        .fetch_one(source_store.pool())
        .await
        .expect("converge source before creating a valid sibling competitor");
    assert_eq!(initial_checkpoint, (0, 0, 0));

    let competitor = source
        .database_path()
        .with_file_name("hivegui-main-leaf-competitor.db");
    fs::copy(source.database_path(), &competitor).expect("create valid sibling competitor");
    let competitor_sha256_before = sha256_path(&competitor);
    let competitor_metadata = fs::metadata(&competitor).expect("read competitor inode identity");
    let competitor_identity = (competitor_metadata.dev(), competitor_metadata.ino());
    let archive = unique_target_path(&source, "post-open-main-leaf-exchange");
    let coordinator = BackupCoordinator::from_store(source_store.clone())
        .expect("single production backup coordinator");
    let preview = coordinator
        .preview_export(&archive)
        .await
        .expect("preview before the last legal write");

    let mut held_connection = source_store
        .pool()
        .acquire()
        .await
        .expect("hold one production Pool connection across close drain");
    sqlx::query("PRAGMA wal_autocheckpoint=0")
        .execute(&mut *held_connection)
        .await
        .expect("disable automatic checkpoint on the held connection");
    sqlx::query(
        "INSERT INTO data_sources \
         (name,host,port,username,encrypted_password,created_at,updated_at) \
         VALUES ('last-legal-main-leaf-write','127.0.0.1',3307,'wal-user',X'01',CURRENT_TIMESTAMP,CURRENT_TIMESTAMP)",
    )
    .execute(&mut *held_connection)
    .await
    .expect("commit the last legal write into WAL");
    let wal = std::path::PathBuf::from(format!("{}-wal", source.database_path().display()));
    assert!(
        fs::metadata(&wal).is_ok_and(|metadata| metadata.len() > 32),
        "the last legal write must remain WAL-visible while the connection is held"
    );

    let competitor_watch = DataIoWatch::new(&competitor);
    assert!(
        competitor_watch.drain_masks().is_empty(),
        "installing the inode watch must not itself perform competitor data I/O"
    );
    let mut confirmation =
        Box::pin(coordinator.confirm_export(preview, "T119-main-leaf-exchange-passphrase"));
    tokio::select! {
        result = confirmation.as_mut() => {
            panic!("confirmation must wait for the held Pool connection to drain: {result:?}");
        }
        closed = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while !source_store.pool().is_closed() {
                tokio::task::yield_now().await;
            }
        }) => {
            closed.expect("confirmation must enter the closed-Pool drain boundary");
        }
    }

    let exchange_guard = PathExchangeGuard::new(source.database_path(), &competitor);
    let canonical_metadata =
        fs::metadata(source.database_path()).expect("read exchanged canonical inode identity");
    assert_eq!(
        (canonical_metadata.dev(), canonical_metadata.ino()),
        competitor_identity,
        "the canonical source path must now resolve to the pre-watched competitor inode"
    );
    drop(held_connection);
    let confirmation_outcome =
        tokio::time::timeout(std::time::Duration::from_secs(10), confirmation.as_mut())
            .await
            .expect("confirmation must terminate after the held connection drains");
    let competitor_data_io_masks = competitor_watch.drain_masks();
    drop(competitor_watch);
    let competitor_sha256_after = sha256_path(source.database_path());
    let archive_absent = !archive.exists();
    let confirmation_rejected_as_unsafe_source =
        matches!(&confirmation_outcome, Err(ExportError::UnsafeSource(_)));
    let confirmation_diagnostic = confirmation_outcome
        .as_ref()
        .err()
        .map(ToString::to_string)
        .unwrap_or_else(|| "confirmation unexpectedly succeeded".into());

    exchange_guard.restore();
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(source.database_path())
        .create_if_missing(false)
        .foreign_keys(true);
    let read_pool = sqlx::sqlite::SqlitePoolOptions::new()
        .min_connections(1)
        .max_connections(1)
        .connect_with(options)
        .await
        .expect("reopen the original main leaf after exchanging it back");
    let last_legal_write_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM data_sources WHERE name = 'last-legal-main-leaf-write'",
    )
    .fetch_one(&read_pool)
    .await
    .expect("read the WAL-visible last legal write from the original main leaf");
    read_pool.close().await;
    let terminal_probe = sqlx::query_scalar::<_, i64>("SELECT 1")
        .fetch_one(source_store.pool())
        .await;
    let terminal_probe_is_closed = matches!(&terminal_probe, Err(sqlx::Error::PoolClosed));

    assert_eq!(
        (
            confirmation_rejected_as_unsafe_source,
            archive_absent,
            competitor_data_io_masks.is_empty(),
            competitor_sha256_after == competitor_sha256_before,
            last_legal_write_count,
            source_store.pool().is_closed(),
            terminal_probe_is_closed,
        ),
        (true, true, true, true, 1, true, true),
        "Store-open leaf replacement before backup path reopen must be rejected as unsafe \
         before competitor data I/O; \
         confirmation={confirmation_diagnostic}; competitor_masks={competitor_data_io_masks:?}; \
         terminal_probe={terminal_probe:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn every_backup_confirmation_crash_point_executes_the_real_frozen_snapshot_boundary() {
    for point in BACKUP_CONFIRMATION_CRASH_POINTS {
        let source = TestWorkspace::new().expect("isolated confirmation workspace");
        let source_store = Store::open_local(StoreOpenOptions::new(
            source.database_path(),
            source.plugin_root(),
        ))
        .await
        .expect("open confirmation Store");
        source_store
            .create("before-preview", "127.0.0.1", 3306, "user", b"first")
            .await
            .expect("seed before preview");
        let archive = unique_target_path(&source, point.as_str());
        let coordinator = BackupCoordinator::from_store(source_store.clone())
            .expect("production confirmation coordinator");
        let preview = coordinator
            .preview_export(&archive)
            .await
            .expect("preview before last legal write");
        source_store
            .create("last-legal-write", "127.0.0.1", 3307, "user", b"last")
            .await
            .expect("persist the last legal write");

        let interrupted = coordinator
            .confirm_with_crash(preview, "T130-confirmation-crash-passphrase", point)
            .await
            .expect_err("the selected real confirmation boundary must interrupt");
        assert_eq!(interrupted.crash_point(), point.as_str());
        assert!(
            interrupted.reached_requested_boundary(),
            "confirmation must reach the requested real boundary: {interrupted}"
        );
        let blocked = source_store
            .create("after-confirmation", "127.0.0.1", 3308, "user", b"blocked")
            .await
            .expect_err("the write gate remains closed after interruption");
        assert!(blocked.to_string().contains("write_gate_closed"));

        let options = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(source.database_path())
            .create_if_missing(false)
            .foreign_keys(true);
        let read_pool = sqlx::sqlite::SqlitePoolOptions::new()
            .min_connections(1)
            .max_connections(1)
            .connect_with(options)
            .await
            .expect("open interrupted current read-only fixture");
        let names = sqlx::query_scalar::<_, String>("SELECT name FROM data_sources ORDER BY name")
            .fetch_all(&read_pool)
            .await
            .expect("read current after confirmation interruption");
        read_pool.close().await;
        assert_eq!(names, vec!["before-preview", "last-legal-write"]);

        if point.as_str() == "safe_snapshot_publish" {
            let manifest = BackupImporter::new(source.root().join("crash-inspect"))
                .inspect_manifest(&archive, "T130-confirmation-crash-passphrase")
                .await
                .expect("durably published final archive remains authenticated");
            assert_eq!(manifest.schema_version, 4);
        } else {
            assert!(
                !archive.exists(),
                "pre-publish interruption must not expose a final archive"
            );
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn sidecar_cleanup_journal_replays_all_five_durable_states_before_store_open() {
    for (state, location) in [
        ("prepared", "canonical"),
        ("prepared", "quarantine"),
        ("quarantined", "quarantine"),
        ("quarantined", "absent"),
        ("done", "absent"),
    ] {
        let workspace = TestWorkspace::new().expect("isolated cleanup replay workspace");
        let database = write_restore_current_database(workspace.root()).await;
        let database_sha256 = sha256_path(&database);
        let canonical_name = "datasources.db-wal";
        let canonical = workspace.root().join(canonical_name);
        let bytes = format!("safe-residual-{state}-{location}").into_bytes();
        fs::write(&canonical, &bytes).expect("seed proven-safe residual sidecar");
        let identity = sidecar_file_identity(&canonical);
        let cleanup_operation_id = Uuid::new_v4();
        let (journal, quarantine) = write_sidecar_cleanup_journal(
            workspace.root(),
            &database,
            SidecarCleanupJournalFixture {
                artifact: "wal",
                canonical_name,
                sidecar_identity: &identity,
                sidecar_size: bytes.len() as u64,
                sidecar_sha256: &hex::encode(Sha256::digest(&bytes)),
                cleanup_operation_id,
                state,
            },
        );
        match location {
            "canonical" => {}
            "quarantine" => fs::rename(&canonical, &quarantine)
                .expect("simulate durable canonical-to-quarantine rename"),
            "absent" => fs::remove_file(&canonical)
                .expect("simulate quarantine unlink before state durability"),
            _ => unreachable!("fixed cleanup fixture location"),
        }

        let replay = RestoreCoordinator::new(workspace.root())
            .expect("startup coordinator")
            .recover_startup()
            .await
            .expect("valid cleanup journal must converge before Store open");
        assert!(
            replay.store_may_open(),
            "state={state}, location={location}"
        );
        assert!(!journal.exists(), "journal must retire: {state}/{location}");
        assert!(
            !canonical.exists(),
            "canonical safe residual must be gone: {state}/{location}"
        );
        assert!(
            !quarantine.exists(),
            "quarantine safe residual must be gone: {state}/{location}"
        );
        assert_eq!(
            sha256_path(&database),
            database_sha256,
            "cleanup replay must not switch or rewrite the database"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn sidecar_cleanup_ambiguity_preserves_every_byte_and_blocks_store_open() {
    let workspace = TestWorkspace::new().expect("cleanup ambiguity workspace");
    let database = write_restore_current_database(workspace.root()).await;
    let canonical_name = "datasources.db-wal";
    let canonical = workspace.root().join(canonical_name);
    let canonical_bytes = b"owned-safe-residual";
    fs::write(&canonical, canonical_bytes).expect("seed canonical residual");
    let identity = sidecar_file_identity(&canonical);
    let (journal, quarantine) = write_sidecar_cleanup_journal(
        workspace.root(),
        &database,
        SidecarCleanupJournalFixture {
            artifact: "wal",
            canonical_name,
            sidecar_identity: &identity,
            sidecar_size: canonical_bytes.len() as u64,
            sidecar_sha256: &hex::encode(Sha256::digest(canonical_bytes)),
            cleanup_operation_id: Uuid::new_v4(),
            state: "prepared",
        },
    );
    let quarantine_bytes = b"concurrent-quarantine-identity";
    fs::write(&quarantine, quarantine_bytes).expect("plant ambiguous quarantine");

    let error = RestoreCoordinator::new(workspace.root())
        .expect("startup coordinator")
        .recover_startup()
        .await
        .expect_err("dual canonical/quarantine ownership must fail closed");
    assert_eq!(
        error.to_string(),
        "storage_recovery_blocked { reason: sidecar_unknown_owner, artifact: wal }"
    );
    assert_eq!(
        fs::read(&canonical).expect("canonical preserved"),
        canonical_bytes
    );
    assert_eq!(
        fs::read(&quarantine).expect("quarantine preserved"),
        quarantine_bytes
    );
    assert!(journal.exists(), "ambiguous ownership evidence is retained");
}

#[tokio::test(flavor = "current_thread")]
async fn sidecar_cleanup_selects_reason_priority_before_artifact_priority() {
    let workspace = TestWorkspace::new().expect("cleanup priority workspace");
    let database = write_restore_current_database(workspace.root()).await;

    let wal = workspace.root().join("datasources.db-wal");
    fs::write(&wal, b"retired-wal").expect("seed WAL evidence");
    let wal_identity = sidecar_file_identity(&wal);
    let (wal_journal, wal_quarantine) = write_sidecar_cleanup_journal(
        workspace.root(),
        &database,
        SidecarCleanupJournalFixture {
            artifact: "wal",
            canonical_name: "datasources.db-wal",
            sidecar_identity: &wal_identity,
            sidecar_size: 11,
            sidecar_sha256: &hex::encode(Sha256::digest(b"retired-wal")),
            cleanup_operation_id: Uuid::new_v4(),
            state: "done",
        },
    );
    fs::remove_file(&wal).expect("simulate completed WAL unlink");
    fs::write(&wal_quarantine, b"reappeared-quarantine")
        .expect("plant lower-priority WAL unknown-owner state");

    let rollback = workspace.root().join("datasources.db-journal");
    fs::write(&rollback, b"owned-rollback").expect("seed rollback evidence");
    let rollback_identity = sidecar_file_identity(&rollback);
    let (rollback_journal, _) = write_sidecar_cleanup_journal(
        workspace.root(),
        &database,
        SidecarCleanupJournalFixture {
            artifact: "rollback_journal",
            canonical_name: "datasources.db-journal",
            sidecar_identity: &rollback_identity,
            sidecar_size: 14,
            sidecar_sha256: &hex::encode(Sha256::digest(b"owned-rollback")),
            cleanup_operation_id: Uuid::new_v4(),
            state: "prepared",
        },
    );
    fs::remove_file(&rollback).expect("replace rollback identity");
    fs::write(&rollback, b"new-rollback-identity")
        .expect("plant higher-priority reappeared rollback");

    let error = RestoreCoordinator::new(workspace.root())
        .expect("startup coordinator")
        .recover_startup()
        .await
        .expect_err("all candidates must be classified before choosing one error");
    assert_eq!(
        error.to_string(),
        "storage_recovery_blocked { reason: sidecar_reappeared, artifact: rollback_journal }"
    );
    assert!(wal_journal.exists());
    assert!(wal_quarantine.exists());
    assert!(rollback_journal.exists());
    assert_eq!(
        fs::read(&rollback).expect("rollback preserved"),
        b"new-rollback-identity"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn orphan_or_misnamed_sidecar_cleanup_control_state_blocks_without_deletion() {
    for (name, bytes) in [
        (
            format!(".hivegui-sidecar-quarantine-v1-{}-wal", Uuid::new_v4()),
            b"orphan-quarantine".as_slice(),
        ),
        (
            format!(".hivegui-sidecar-cleanup-v1-{}-wal.json", "0".repeat(64)),
            b"{}".as_slice(),
        ),
    ] {
        let workspace = TestWorkspace::new().expect("isolated unknown control workspace");
        write_restore_current_database(workspace.root()).await;
        let unknown = workspace.root().join(name);
        fs::write(&unknown, bytes).expect("seed unknown cleanup control state");

        let error = RestoreCoordinator::new(workspace.root())
            .expect("startup coordinator")
            .recover_startup()
            .await
            .expect_err("unowned cleanup control state must keep Store closed");
        assert_eq!(
            error.to_string(),
            "storage_recovery_blocked { reason: sidecar_unknown_owner, artifact: wal }"
        );
        assert_eq!(
            fs::read(&unknown).expect("unknown evidence retained"),
            bytes
        );
    }
}

#[test]
fn backup_restore_crash_point_inventory_is_complete_and_non_overlapping() {
    let backup = BACKUP_CONFIRMATION_CRASH_POINTS
        .iter()
        .map(|point| point.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let switch = RESTORE_SWITCH_CRASH_POINTS
        .iter()
        .map(|point| point.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let retirement = RETIREMENT_CRASH_POINTS
        .iter()
        .map(|point| point.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let sidecar = SIDECAR_CLEANUP_CRASH_POINTS
        .iter()
        .map(|point| point.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(backup.len(), BACKUP_CONFIRMATION_CRASH_POINTS.len());
    assert_eq!(switch.len(), RESTORE_SWITCH_CRASH_POINTS.len());
    assert_eq!(retirement.len(), RETIREMENT_CRASH_POINTS.len());
    assert_eq!(sidecar.len(), SIDECAR_CLEANUP_CRASH_POINTS.len());
    assert!(backup.is_disjoint(&switch));
    assert!(backup.is_disjoint(&retirement));
    assert!(switch.is_disjoint(&retirement));
    assert!(sidecar.is_disjoint(&backup));
    assert!(sidecar.is_disjoint(&switch));
    assert!(sidecar.is_disjoint(&retirement));
    for required in [
        "write_gate_close",
        "checkpoint_current",
        "checkpoint_staging",
        "close_connections",
        "sidecar_convergence",
        "safe_snapshot_publish",
        "manifest_arm_staging_write",
        "manifest_arm_staging_fsync",
        "manifest_arm_rename",
        "manifest_arm_parent_fsync",
        "manifest_arm",
        "owner_prepared_staging_write",
        "owner_prepared_staging_fsync",
        "owner_prepared_rename",
        "owner_prepared_parent_fsync",
        "owner_prepared_publish",
        "database_switch",
        "plugin_tree_switch",
        "owner_applying_staging_write",
        "owner_applying_staging_fsync",
        "owner_applying_rename",
        "owner_applying_parent_fsync",
        "owner_applying_publish",
        "new_health_verify",
        "new_search_verify",
        "new_artifact_verify",
        "new_identity_verify",
        "owner_committed_staging_write",
        "owner_committed_staging_fsync",
        "owner_committed_rename",
        "owner_committed_parent_fsync",
        "owner_committed_publish",
        "retirement_prepared_staging_write",
        "retirement_prepared_staging_fsync",
        "retirement_prepared_rename",
        "retirement_prepared_parent_fsync",
        "retirement_prepared_publish",
        "live_to_tombstone_rename",
        "live_to_tombstone_parent_fsync",
        "live_to_tombstone_identity_verify",
        "retirement_renamed_staging_write",
        "retirement_renamed_staging_fsync",
        "retirement_renamed_rename",
        "retirement_renamed_parent_fsync",
        "retirement_renamed_publish",
        "tombstone_leaf_unlink",
        "tombstone_directory_fsync",
        "tombstone_rmdir",
        "retirement_done_staging_write",
        "retirement_done_staging_fsync",
        "retirement_done_rename",
        "retirement_done_parent_fsync",
        "retirement_done_publish",
        "retirement_journal_delete",
        "retirement_journal_unlink",
        "retirement_journal_unlink_parent_fsync",
        "sidecar_prepared_staging_fsync",
        "sidecar_prepared_publish",
        "sidecar_prepared_parent_fsync",
        "sidecar_quarantine_rename",
        "sidecar_quarantine_parent_fsync",
        "sidecar_quarantine_identity_verify",
        "sidecar_quarantined_staging_fsync",
        "sidecar_quarantined_publish",
        "sidecar_quarantined_parent_fsync",
        "sidecar_quarantine_unlink",
        "sidecar_quarantine_unlink_parent_fsync",
        "sidecar_done_staging_fsync",
        "sidecar_done_publish",
        "sidecar_done_parent_fsync",
        "sidecar_journal_unlink",
        "sidecar_journal_unlink_parent_fsync",
    ] {
        assert!(
            backup.contains(required)
                || switch.contains(required)
                || retirement.contains(required)
                || sidecar.contains(required),
            "missing crash boundary {required}"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn prepared_owner_binds_every_controlled_old_new_and_safety_object() {
    let source = TestWorkspace::new().expect("source workspace");
    let source_database = write_seed_database(&source).await;
    let archive = unique_target_path(&source, "owner-evidence");
    BackupExporter::new(&source_database)
        .export_age(&archive, "T130-owner-evidence-passphrase")
        .await
        .expect("export replacement state");

    let target = TestWorkspace::new().expect("target workspace");
    let root = target.root().join("owner-evidence-root");
    fs::create_dir_all(&root).expect("create target root");
    let current_database = root.join("datasources.db");
    let current_plugins = root.join("plugins");
    let old_store = Store::open_local(StoreOpenOptions::new(&current_database, &current_plugins))
        .await
        .expect("open old current");
    old_store
        .create(
            "old-owner-evidence",
            "127.0.0.1",
            3306,
            "old-user",
            b"old-password",
        )
        .await
        .expect("seed old state");
    old_store.pool().close().await;
    drop(old_store);
    let old_database_sha256 = sha256_path(&current_database);
    let old_database_identity = sidecar_file_identity(&current_database);
    let old_plugin_identity = sidecar_file_identity(&current_plugins);

    let coordinator = RestoreCoordinator::new(&root).expect("restore coordinator");
    let plan = coordinator
        .prepare_restore(&archive, "T130-owner-evidence-passphrase")
        .await
        .expect("prepare authenticated restore");
    let interrupted = coordinator
        .apply_with_crash(
            &plan,
            RESTORE_SWITCH_CRASH_POINTS
                .iter()
                .copied()
                .find(|point| point.as_str() == "owner_prepared_publish")
                .expect("owner prepared point"),
        )
        .await
        .expect_err("interrupt after durable prepared owner");
    assert!(interrupted.reached_requested_boundary());

    let live_relative = format!(
        ".hivegui-db-staging-v1/restore-{}",
        plan.db_instance_operation_id()
    );
    let live = root.join(&live_relative);
    let owner: serde_json::Value = serde_json::from_slice(
        &fs::read(live.join(".hivegui-db-recovery-v1.json")).expect("durable prepared owner"),
    )
    .expect("parse owner");
    assert_eq!(owner["phase"], "prepared");
    assert_eq!(
        owner["manifest_identity"],
        sidecar_file_identity(&live.join(".hivegui-db-instance-v1.json"))
    );
    assert_eq!(owner["old_database"]["relative_path"], "datasources.db");
    assert_eq!(owner["old_database"]["identity"], old_database_identity);
    assert_eq!(owner["old_database"]["sha256"], old_database_sha256);
    assert_eq!(
        owner["new_database"]["relative_path"],
        format!("{live_relative}/datasources.db")
    );
    assert_eq!(
        owner["new_database"]["sha256"],
        sha256_path(&live.join("datasources.db"))
    );
    assert_eq!(owner["old_plugin_root"]["relative_path"], "plugins");
    assert_eq!(owner["old_plugin_root"]["identity"], old_plugin_identity);
    assert_eq!(
        owner["new_plugin_root"]["relative_path"],
        format!("{live_relative}/plugins")
    );
    assert_eq!(
        owner["safety_snapshot"]["relative_path"],
        format!("backups/restore-safety-{}", plan.db_instance_operation_id())
    );
    for descriptor in [
        &owner["old_database"],
        &owner["new_database"],
        &owner["old_plugin_root"],
        &owner["new_plugin_root"],
        &owner["safety_snapshot"],
    ] {
        assert_eq!(descriptor["sha256"].as_str().map(str::len), Some(64));
        assert!(
            descriptor["identity"]
                .as_str()
                .is_some_and(|value| !value.is_empty())
        );
        assert!(
            descriptor["relative_path"]
                .as_str()
                .is_some_and(|value| !value.starts_with('/'))
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn prepared_retirement_binds_live_owner_terminal_current_and_plugin_tree() {
    let source = TestWorkspace::new().expect("source workspace");
    let source_database = write_seed_database(&source).await;
    let archive = unique_target_path(&source, "retirement-evidence");
    BackupExporter::new(&source_database)
        .export_age(&archive, "T130-retirement-evidence-passphrase")
        .await
        .expect("export replacement state");

    let target = TestWorkspace::new().expect("target workspace");
    let root = target.root().join("retirement-evidence-root");
    fs::create_dir_all(&root).expect("create target root");
    let current_database = root.join("datasources.db");
    let current_plugins = root.join("plugins");
    let old_store = Store::open_local(StoreOpenOptions::new(&current_database, &current_plugins))
        .await
        .expect("open old current");
    old_store.pool().close().await;
    drop(old_store);

    let coordinator = RestoreCoordinator::new(&root).expect("restore coordinator");
    let plan = coordinator
        .prepare_restore(&archive, "T130-retirement-evidence-passphrase")
        .await
        .expect("prepare authenticated restore");
    let point = RETIREMENT_CRASH_POINTS
        .iter()
        .copied()
        .find(|point| point.as_str() == "retirement_prepared_publish")
        .expect("retirement prepared point");
    let interrupted = coordinator
        .apply_with_crash(&plan, point)
        .await
        .expect_err("interrupt after durable prepared retirement");
    assert!(interrupted.reached_requested_boundary());

    let live_basename = format!("restore-{}", plan.db_instance_operation_id());
    let registry = root.join(".hivegui-db-staging-v1");
    let live = registry.join(&live_basename);
    let journal: serde_json::Value = serde_json::from_slice(
        &fs::read(registry.join(format!(
            ".hivegui-db-retirement-v1-restore-{}.json",
            plan.db_instance_operation_id()
        )))
        .expect("durable prepared retirement"),
    )
    .expect("parse retirement journal");
    assert_eq!(journal["state"], "prepared");
    assert_eq!(journal["terminal_outcome"], "new");
    assert_eq!(
        journal["live_directory_identity"],
        sidecar_file_identity(&live)
    );
    assert_eq!(journal["manifest_ownership_state"], "armed");
    assert_eq!(journal["owner_phase"], "committed");
    assert!(
        journal["manifest_identity"]
            .as_str()
            .is_some_and(|value| !value.is_empty())
    );
    assert!(
        journal["owner_identity"]
            .as_str()
            .is_some_and(|value| !value.is_empty())
    );
    assert_eq!(
        journal["terminal_database"]["relative_path"],
        "datasources.db"
    );
    assert_eq!(
        journal["terminal_database"]["sha256"],
        sha256_path(&current_database)
    );
    assert_eq!(journal["terminal_plugin_root"]["relative_path"], "plugins");
    assert_eq!(
        journal["terminal_plugin_root"]["identity"],
        sidecar_file_identity(&current_plugins)
    );
    assert_eq!(
        journal["terminal_plugin_root"]["sha256"]
            .as_str()
            .map(str::len),
        Some(64)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn startup_reconciles_exact_next_manifest_staging_before_aborted_retirement() {
    let source = TestWorkspace::new().expect("source workspace");
    let source_database = write_seed_database(&source).await;
    let archive = unique_target_path(&source, "manifest-next-staging");
    BackupExporter::new(&source_database)
        .export_age(&archive, "T130-manifest-next-passphrase")
        .await
        .expect("export replacement state");
    let target = TestWorkspace::new().expect("target workspace");
    let root = target.root().join("manifest-next-root");
    fs::create_dir_all(&root).expect("create target root");
    let coordinator = RestoreCoordinator::new(&root).expect("restore coordinator");
    let plan = coordinator
        .prepare_restore(&archive, "T130-manifest-next-passphrase")
        .await
        .expect("prepare unarmed restore");
    let live = root
        .join(".hivegui-db-staging-v1")
        .join(format!("restore-{}", plan.db_instance_operation_id()));
    let final_path = live.join(".hivegui-db-instance-v1.json");
    let mut armed: serde_json::Value =
        serde_json::from_slice(&fs::read(&final_path).expect("read final unarmed manifest"))
            .expect("parse final unarmed manifest");
    armed["ownership_state"] = serde_json::Value::String("armed".into());
    fs::write(
        live.join(".hivegui-db-instance-v1.json.staging"),
        serde_json::to_vec(&armed).expect("encode next armed staging"),
    )
    .expect("plant exact next manifest staging");

    let replay = RestoreCoordinator::new(&root)
        .expect("restart coordinator")
        .recover_startup()
        .await
        .expect("exact unarmed-to-armed staging is discarded before retrying final");
    assert_eq!(
        replay.retirement_outcome(),
        RetirementOutcome::AbortedPreSwitch
    );
    assert_eq!(
        fs::read_dir(root.join(".hivegui-db-staging-v1"))
            .expect("registry")
            .count(),
        0
    );
}

#[tokio::test(flavor = "current_thread")]
async fn startup_reconciles_exact_next_owner_staging_and_restores_prepared_old() {
    let source = TestWorkspace::new().expect("source workspace");
    let source_database = write_seed_database(&source).await;
    let archive = unique_target_path(&source, "owner-next-staging");
    BackupExporter::new(&source_database)
        .export_age(&archive, "T130-owner-next-passphrase")
        .await
        .expect("export replacement state");
    let target = TestWorkspace::new().expect("target workspace");
    let root = target.root().join("owner-next-root");
    fs::create_dir_all(&root).expect("create target root");
    let current_database = root.join("datasources.db");
    let current_plugins = root.join("plugins");
    let old_store = Store::open_local(StoreOpenOptions::new(&current_database, &current_plugins))
        .await
        .expect("open old current");
    old_store
        .create(
            "old-owner-next",
            "127.0.0.1",
            3306,
            "old-user",
            b"old-password",
        )
        .await
        .expect("seed old current");
    old_store.pool().close().await;
    drop(old_store);
    let coordinator = RestoreCoordinator::new(&root).expect("restore coordinator");
    let plan = coordinator
        .prepare_restore(&archive, "T130-owner-next-passphrase")
        .await
        .expect("prepare restore");
    let point = RESTORE_SWITCH_CRASH_POINTS
        .iter()
        .copied()
        .find(|point| point.as_str() == "owner_prepared_publish")
        .expect("prepared owner point");
    coordinator
        .apply_with_crash(&plan, point)
        .await
        .expect_err("interrupt after prepared owner");
    let live = root
        .join(".hivegui-db-staging-v1")
        .join(format!("restore-{}", plan.db_instance_operation_id()));
    let owner_path = live.join(".hivegui-db-recovery-v1.json");
    let mut applying: serde_json::Value =
        serde_json::from_slice(&fs::read(&owner_path).expect("read prepared owner"))
            .expect("parse prepared owner");
    applying["phase"] = serde_json::Value::String("applying".into());
    fs::write(
        live.join(".hivegui-db-recovery-v1.json.staging"),
        serde_json::to_vec(&applying).expect("encode applying owner staging"),
    )
    .expect("plant exact next owner staging");

    let replay = RestoreCoordinator::new(&root)
        .expect("restart coordinator")
        .recover_startup()
        .await
        .expect("discard next owner staging and replay durable prepared final");
    assert_eq!(replay.retirement_outcome(), RetirementOutcome::Old);
    let reopened = Store::open_local(StoreOpenOptions::new(&current_database, &current_plugins))
        .await
        .expect("reopen restored old");
    assert_eq!(
        reopened
            .list()
            .await
            .expect("list old")
            .into_iter()
            .map(|row| row.name)
            .collect::<Vec<_>>(),
        vec!["old-owner-next"]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn startup_reconciles_exact_next_retirement_staging_before_replaying_final() {
    let source = TestWorkspace::new().expect("source workspace");
    let source_database = write_seed_database(&source).await;
    let archive = unique_target_path(&source, "retirement-next-staging");
    BackupExporter::new(&source_database)
        .export_age(&archive, "T130-retirement-next-passphrase")
        .await
        .expect("export replacement state");
    let target = TestWorkspace::new().expect("target workspace");
    let root = target.root().join("retirement-next-root");
    fs::create_dir_all(&root).expect("create target root");
    let coordinator = RestoreCoordinator::new(&root).expect("restore coordinator");
    let plan = coordinator
        .prepare_restore(&archive, "T130-retirement-next-passphrase")
        .await
        .expect("prepare restore");
    let point = RETIREMENT_CRASH_POINTS
        .iter()
        .copied()
        .find(|point| point.as_str() == "retirement_prepared_publish")
        .expect("prepared retirement point");
    coordinator
        .apply_with_crash(&plan, point)
        .await
        .expect_err("interrupt after prepared retirement");
    let registry = root.join(".hivegui-db-staging-v1");
    let journal_path = registry.join(format!(
        ".hivegui-db-retirement-v1-restore-{}.json",
        plan.db_instance_operation_id()
    ));
    let mut renamed: serde_json::Value =
        serde_json::from_slice(&fs::read(&journal_path).expect("read prepared retirement"))
            .expect("parse prepared retirement");
    renamed["state"] = serde_json::Value::String("renamed".into());
    fs::write(
        std::path::PathBuf::from(format!("{}.staging", journal_path.display())),
        serde_json::to_vec(&renamed).expect("encode renamed retirement staging"),
    )
    .expect("plant exact next retirement staging");

    let replay = RestoreCoordinator::new(&root)
        .expect("restart coordinator")
        .recover_startup()
        .await
        .expect("discard next retirement staging and replay prepared final");
    assert_eq!(replay.retirement_outcome(), RetirementOutcome::New);
    assert_eq!(
        fs::read_dir(&registry).expect("registry").count(),
        0,
        "retirement must converge completely"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn startup_discards_proven_unpublished_retirement_staging_and_retires_from_live_owner() {
    let source = TestWorkspace::new().expect("source workspace");
    let source_database = write_seed_database(&source).await;
    let archive = unique_target_path(&source, "retirement-staging-only");
    BackupExporter::new(&source_database)
        .export_age(&archive, "T130-retirement-staging-only-passphrase")
        .await
        .expect("export replacement state");
    let target = TestWorkspace::new().expect("target workspace");
    let root = target.root().join("retirement-staging-only-root");
    fs::create_dir_all(&root).expect("create target root");
    let coordinator = RestoreCoordinator::new(&root).expect("restore coordinator");
    let plan = coordinator
        .prepare_restore(&archive, "T130-retirement-staging-only-passphrase")
        .await
        .expect("prepare restore");
    let point = RETIREMENT_CRASH_POINTS
        .iter()
        .copied()
        .find(|point| point.as_str() == "retirement_prepared_publish")
        .expect("prepared retirement point");
    coordinator
        .apply_with_crash(&plan, point)
        .await
        .expect_err("interrupt after prepared retirement");
    let registry = root.join(".hivegui-db-staging-v1");
    let journal_path = registry.join(format!(
        ".hivegui-db-retirement-v1-restore-{}.json",
        plan.db_instance_operation_id()
    ));
    let staging = std::path::PathBuf::from(format!("{}.staging", journal_path.display()));
    fs::rename(&journal_path, &staging).expect("simulate crash before initial journal publish");

    let replay = RestoreCoordinator::new(&root)
        .expect("restart coordinator")
        .recover_startup()
        .await
        .expect("proven unpublished staging is discarded and rebuilt from live owner");
    assert_eq!(replay.retirement_outcome(), RetirementOutcome::New);
    assert_eq!(fs::read_dir(&registry).expect("registry").count(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn every_restore_switch_and_retirement_crash_replays_or_fails_closed_at_armed_owner_gap() {
    let source = TestWorkspace::new().expect("source workspace");
    let database = write_seed_database(&source).await;
    let archive = unique_target_path(&source, "restore-crash-matrix");
    BackupExporter::new(&database)
        .export_age(&archive, "T119-crash-matrix-passphrase")
        .await
        .expect("seed authenticated archive");

    for point in RESTORE_SWITCH_CRASH_POINTS
        .iter()
        .chain(RETIREMENT_CRASH_POINTS.iter())
    {
        let target = TestWorkspace::new().expect("isolated target per crash point");
        let current_database = target.root().join("datasources.db");
        let current_plugins = target.root().join("plugins");
        let old_store =
            Store::open_local(StoreOpenOptions::new(&current_database, &current_plugins))
                .await
                .expect("open old current per crash point");
        old_store
            .create(
                "old-before-crash",
                "127.0.0.1",
                3307,
                "old-user",
                b"old-password",
            )
            .await
            .expect("seed old current per crash point");
        drop(old_store);
        let coordinator =
            RestoreCoordinator::new(target.root()).expect("production restore coordinator");
        let plan = coordinator
            .prepare_restore(&archive, "T119-crash-matrix-passphrase")
            .await
            .expect("prepare unarmed restore instance");
        assert_ne!(
            plan.db_instance_operation_id(),
            plan.cleanup_operation_id(),
            "instance UUID and cleanup UUID are distinct"
        );
        assert_eq!(plan.manifest().format_version, 3);
        assert!(!plan.owner_exists());
        let injected = coordinator
            .apply_with_crash(&plan, *point)
            .await
            .expect_err("fault point must interrupt the first process");
        assert_eq!(injected.crash_point(), point.as_str());
        assert!(
            injected.reached_requested_boundary(),
            "fault injection must advance the real state machine to {}: {injected}",
            point.as_str()
        );

        let replay = RestoreCoordinator::new(target.root())
            .expect("restart coordinator")
            .recover_startup()
            .await;
        if matches!(
            point.as_str(),
            "manifest_arm_rename"
                | "manifest_arm_parent_fsync"
                | "manifest_arm"
                | "owner_prepared_staging_write"
                | "owner_prepared_staging_fsync"
        ) {
            assert_eq!(
                replay
                    .expect_err("armed manifest without a durable owner is blocked")
                    .to_string(),
                "storage_recovery_blocked { reason: sidecar_unknown_owner, artifact: wal }"
            );
            continue;
        }
        let replay = replay.expect("deterministic startup replay");
        assert!(replay.store_may_open());
        assert!(replay.has_exactly_one_live_database());
        assert!(replay.has_no_mixed_database_or_plugin_tree());
        assert!(replay.control_files_are_outside_live_tree());
        assert!(matches!(
            replay.retirement_outcome(),
            RetirementOutcome::AbortedPreSwitch | RetirementOutcome::Old | RetirementOutcome::New
        ));
        assert!(replay.retirement_is_done());
        assert!(replay.write_gate_is_open());
        let expected_aborted = matches!(
            point.as_str(),
            "manifest_arm_staging_write"
                | "manifest_arm_staging_fsync"
                | "retirement_journal_unlink"
                | "retirement_journal_unlink_parent_fsync"
        );
        let expected_new = matches!(
            point.as_str(),
            "owner_committed_rename"
                | "owner_committed_parent_fsync"
                | "owner_committed_publish"
                | "retirement_prepared_staging_write"
                | "retirement_prepared_staging_fsync"
                | "retirement_prepared_rename"
                | "retirement_prepared_parent_fsync"
                | "retirement_prepared_publish"
                | "live_to_tombstone_rename"
                | "live_to_tombstone_parent_fsync"
                | "live_to_tombstone_identity_verify"
                | "retirement_renamed_staging_write"
                | "retirement_renamed_staging_fsync"
                | "retirement_renamed_rename"
                | "retirement_renamed_parent_fsync"
                | "retirement_renamed_publish"
                | "tombstone_leaf_unlink"
                | "tombstone_directory_fsync"
                | "tombstone_rmdir"
                | "retirement_done_staging_write"
                | "retirement_done_staging_fsync"
                | "retirement_done_rename"
                | "retirement_done_parent_fsync"
                | "retirement_done_publish"
                | "retirement_journal_delete"
        );
        assert_eq!(
            replay.retirement_outcome(),
            if expected_aborted {
                RetirementOutcome::AbortedPreSwitch
            } else if expected_new {
                RetirementOutcome::New
            } else {
                RetirementOutcome::Old
            },
            "the replay outcome after {} is determined by the committed owner boundary",
            point.as_str()
        );
        let reopened =
            Store::open_local(StoreOpenOptions::new(&current_database, &current_plugins))
                .await
                .expect("reopen the replay-selected current");
        let names = reopened
            .list()
            .await
            .expect("list replay-selected current")
            .into_iter()
            .map(|row| row.name)
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            if expected_new
                || matches!(
                    point.as_str(),
                    "retirement_journal_unlink" | "retirement_journal_unlink_parent_fsync"
                )
            {
                vec!["backup-seed".to_string()]
            } else {
                vec!["old-before-crash".to_string()]
            },
            "selected state after {}",
            point.as_str()
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn startup_checks_current_cleanup_before_replaying_a_committed_live_owner() {
    let source = TestWorkspace::new().expect("source workspace");
    let source_database = write_seed_database(&source).await;
    let archive = unique_target_path(&source, "startup-cleanup-order");
    BackupExporter::new(&source_database)
        .export_age(&archive, "T130-startup-cleanup-order-passphrase")
        .await
        .expect("export replacement state");

    let target = TestWorkspace::new().expect("target workspace");
    let current_database = write_restore_current_database(target.root()).await;
    let coordinator = RestoreCoordinator::new(target.root()).expect("restore coordinator");
    let plan = coordinator
        .prepare_restore(&archive, "T130-startup-cleanup-order-passphrase")
        .await
        .expect("prepare replacement");
    let committed = RESTORE_SWITCH_CRASH_POINTS
        .iter()
        .copied()
        .find(|point| point.as_str() == "owner_committed_publish")
        .expect("committed owner point");
    coordinator
        .apply_with_crash(&plan, committed)
        .await
        .expect_err("stop with a durable committed live owner");
    let live = target
        .root()
        .join(".hivegui-db-staging-v1")
        .join(format!("restore-{}", plan.db_instance_operation_id()));
    assert!(
        live.is_dir(),
        "committed owner is live before startup replay"
    );

    let residual = target.root().join("datasources.db-journal");
    let residual_bytes = vec![0_u8; 128];
    fs::write(&residual, &residual_bytes).expect("plant recoverable current sidecar");
    let current_before = sha256_path(&current_database);

    let error = RestoreCoordinator::new(target.root())
        .expect("startup coordinator")
        .recover_startup()
        .await
        .expect_err("current cleanup must block before owner replay");
    assert_eq!(
        error.to_string(),
        "storage_recovery_blocked { reason: sidecar_recoverable, artifact: rollback_journal }"
    );
    assert!(
        live.is_dir(),
        "blocked current cleanup must not retire or otherwise mutate the live owner"
    );
    assert_eq!(
        fs::read(&residual).expect("recoverable sidecar retained"),
        residual_bytes
    );
    assert_eq!(sha256_path(&current_database), current_before);
}

#[cfg(target_os = "linux")]
#[tokio::test(flavor = "current_thread")]
async fn offline_apply_revalidates_safety_binding_before_opening_the_return_gate() {
    use std::os::unix::fs::MetadataExt as _;

    const PASSPHRASE: &str = "T130-offline-return-proof-passphrase";
    const OLD_NAME: &str = "old-before-offline-return-proof";
    const NEW_NAME: &str = "new-before-offline-return-proof";

    async fn assert_real_store_write_gate_closed(root: &Path, name: &str) {
        let database = root.join("datasources.db");
        let database_sha256 = sha256_path(&database);
        match Store::open_local(StoreOpenOptions::for_root(root)).await {
            Err(error) => {
                assert_eq!(error.kind(), StoreOpenErrorKind::Io);
                assert_eq!(
                    error.failure_class(),
                    Some(DatabaseFailureClass::Persistent)
                );
                assert_eq!(
                    error.retry_count(),
                    0,
                    "a closed process gate must reject before any Store-open retry"
                );
            }
            Ok(store) => {
                let write_succeeded = store
                    .create(
                        name,
                        "127.0.0.1",
                        3390,
                        "return-proof-user",
                        b"return-proof-password",
                    )
                    .await
                    .is_ok();
                store.pool().close().await;
                drop(store);
                panic!(
                    "the closed process gate unexpectedly admitted a Store; production write succeeded={write_succeeded}"
                );
            }
        }
        assert_eq!(
            sha256_path(&database),
            database_sha256,
            "the blocked production writer must not modify the selected new current"
        );
        for sidecar in [
            root.join("datasources.db-wal"),
            root.join("datasources.db-shm"),
            root.join("datasources.db-journal"),
        ] {
            assert!(
                !sidecar.exists(),
                "the closed Store probe must leave no SQLite sidecar: {}",
                sidecar.display()
            );
        }
    }

    assert_offline_apply_return_proof_source_contract();

    let source = TestWorkspace::new().expect("replacement source workspace");
    let archive =
        export_single_data_source_archive(&source, "offline-return-proof", NEW_NAME, PASSPHRASE)
            .await;

    let target = TestWorkspace::new().expect("offline restore target workspace");
    let root = target.root().join("offline-return-proof-root");
    fs::create_dir_all(&root).expect("create offline restore root");
    let current_database = root.join("datasources.db");
    let old_store = Store::open_local(StoreOpenOptions::for_root(&root))
        .await
        .expect("open old current Store");
    old_store
        .create(
            OLD_NAME,
            "127.0.0.1",
            3307,
            "old-return-proof-user",
            b"old-return-proof-password",
        )
        .await
        .expect("seed old current state");
    old_store.pool().close().await;
    drop(old_store);

    let coordinator = RestoreCoordinator::new(&root).expect("offline restore coordinator");
    let plan = coordinator
        .prepare_restore(&archive, PASSPHRASE)
        .await
        .expect("prepare authenticated offline restore");
    let operation_id = plan.db_instance_operation_id().to_owned();
    let registry = root.join(".hivegui-db-staging-v1");
    let live = registry.join(format!("restore-{operation_id}"));
    let safety_snapshot = root
        .join("backups")
        .join(format!("restore-safety-{operation_id}"));

    let backups = root.join("backups");
    fs::create_dir_all(&backups).expect("create safety snapshot parent");
    let foreign = backups.join("offline-return-foreign-tree");
    let foreign_nested = foreign.join("nested");
    fs::create_dir_all(&foreign_nested).expect("create foreign regular tree");
    fs::write(
        foreign.join("root-canary.bin"),
        b"offline-return-foreign-root-must-not-change",
    )
    .expect("write foreign root canary");
    fs::write(
        foreign_nested.join("nested-canary.bin"),
        b"offline-return-foreign-nested-must-not-change",
    )
    .expect("write foreign nested canary");
    let foreign_tree_before = regular_tree_bytes(&foreign);
    let foreign_metadata = fs::metadata(&foreign).expect("record foreign tree identity");
    let foreign_identity = (foreign_metadata.dev(), foreign_metadata.ino());
    let foreign_watches = watch_regular_tree(&foreign);
    assert!(
        foreign_watches
            .iter()
            .all(|watch| watch.drain_masks().is_empty()),
        "foreign watch setup must establish an empty pre-exchange baseline"
    );

    let barriers: [std::sync::Arc<tokio::sync::Barrier>; 2] =
        std::array::from_fn(|_| std::sync::Arc::new(tokio::sync::Barrier::new(2)));
    let apply_coordinator = coordinator.clone();
    let apply_plan = plan.clone();
    let apply_barriers = barriers.clone();
    let mut apply_task = tokio::spawn(async move {
        apply_coordinator
            .apply_restore_with_safety_return_interlock_for_test(&apply_plan, apply_barriers)
            .await
    });

    // Barrier 0 is after the one real switch/health/retirement outcome has
    // completed with keep_frozen=true, and immediately before the same held
    // safety proof is revalidated for the public return. The return is not
    // finalized and no writer may enter yet.
    tokio::time::timeout(Duration::from_secs(30), barriers[0].wait())
        .await
        .expect("reach frozen offline return-proof boundary");
    assert_eq!(
        data_source_names_from_database(&current_database).await,
        vec![NEW_NAME]
    );
    assert_eq!(
        data_source_names_from_database(&safety_snapshot.join("datasources.db")).await,
        vec![OLD_NAME]
    );
    assert!(safety_snapshot.join("manifest.json").is_file());
    assert!(
        !live.exists(),
        "the real successful outcome must have retired its live instance before return proof"
    );
    assert_eq!(
        fs::read_dir(&registry)
            .expect("read fully retired offline registry")
            .count(),
        0,
        "the interlock must follow the real successful retirement outcome"
    );
    assert_real_store_write_gate_closed(&root, "must-not-write-before-return-proof").await;
    assert!(
        foreign_watches
            .iter()
            .all(|watch| watch.drain_masks().is_empty()),
        "discard only pre-exchange fixture activity before the monitored interval"
    );

    let exchange = RestorableDirectoryExchangeGuard::new(&safety_snapshot, &foreign);
    assert_eq!(
        fs::metadata(&safety_snapshot)
            .map(|metadata| (metadata.dev(), metadata.ino()))
            .expect("inspect exchanged canonical safety entry"),
        foreign_identity,
        "the exact foreign directory must occupy the canonical safety path"
    );
    barriers[1].wait().await;

    let apply_error = tokio::time::timeout(Duration::from_secs(10), &mut apply_task)
        .await
        .expect("return-time safety mismatch must resolve without reopening the gate")
        .expect("offline apply task must not panic")
        .expect_err("a successful switch with an invalid return proof must never report Ok");
    assert!(
        matches!(
            &apply_error,
            ImportError::UnsafeArchiveEntry(reason)
                if reason == "restore safety snapshot identity changed"
        ),
        "return-time rejection must identify the exact invalid safety binding: {apply_error}"
    );
    assert_eq!(
        data_source_names_from_database(&current_database).await,
        vec![NEW_NAME],
        "the completed real outcome remains selected while return is fail-closed"
    );
    assert_real_store_write_gate_closed(&root, "must-not-write-after-invalid-return-proof").await;

    let foreign_masks = foreign_watches
        .iter()
        .flat_map(DataIoWatch::drain_masks)
        .collect::<Vec<_>>();
    let foreign_data_io_absent = foreign_masks.iter().all(|mask| {
        mask & (libc::IN_OPEN
            | libc::IN_ACCESS
            | libc::IN_MODIFY
            | libc::IN_CLOSE_WRITE
            | libc::IN_CREATE
            | libc::IN_DELETE
            | libc::IN_DELETE_SELF)
            == 0
    });
    drop(foreign_watches);
    let foreign_tree_while_exchanged = regular_tree_bytes(&safety_snapshot);
    assert!(
        foreign_data_io_absent && foreign_tree_while_exchanged == foreign_tree_before,
        "return proof may compare binding metadata but must perform zero foreign-tree data I/O or mutation; masks={foreign_masks:?}"
    );

    exchange.restore();
    assert_eq!(regular_tree_bytes(&foreign), foreign_tree_before);
    assert_eq!(
        data_source_names_from_database(&safety_snapshot.join("datasources.db")).await,
        vec![OLD_NAME],
        "swapback must restore the verified old safety snapshot before startup recovery"
    );

    let recovered = RestoreCoordinator::new(&root)
        .expect("fresh startup coordinator")
        .recover_startup()
        .await
        .expect("startup replay conservatively releases the fail-closed return gate");
    assert!(
        recovered.retirement_is_done()
            && recovered.store_may_open()
            && recovered.write_gate_is_open(),
        "only startup replay may reopen a gate after invalid return proof"
    );
    let reopened = Store::open_local(StoreOpenOptions::for_root(&root))
        .await
        .expect("open selected new current after startup replay");
    assert_eq!(
        reopened
            .list()
            .await
            .expect("read selected new current after startup replay")
            .into_iter()
            .map(|row| row.name)
            .collect::<Vec<_>>(),
        vec![NEW_NAME]
    );
    reopened
        .create(
            "write-after-return-proof-recovery",
            "127.0.0.1",
            3391,
            "recovered-user",
            b"recovered-password",
        )
        .await
        .expect("startup replay is the first boundary allowed to reopen writes");
    reopened.pool().close().await;
    drop(reopened);
}

#[tokio::test(flavor = "current_thread")]
async fn confirmed_offline_restore_switches_complete_state_then_commits_and_retires() {
    let source = TestWorkspace::new().expect("source workspace");
    let source_store = Store::open_local(StoreOpenOptions::new(
        source.database_path(),
        source.plugin_root(),
    ))
    .await
    .expect("open source Store");
    source_store
        .create(
            "restored-new",
            "127.0.0.1",
            3306,
            "new-user",
            b"new-password",
        )
        .await
        .expect("seed source state");
    let archive = unique_target_path(&source, "confirmed-offline-restore");
    BackupExporter::new(source.database_path())
        .export_age(&archive, "T130-confirmed-restore-passphrase")
        .await
        .expect("export source state");
    drop(source_store);

    let target = TestWorkspace::new().expect("target workspace");
    let target_root = target.root().join("current-data-root");
    fs::create_dir_all(&target_root).expect("create target data root");
    let current_database = target_root.join("datasources.db");
    let current_plugins = target_root.join("plugins");
    let old_store = Store::open_local(StoreOpenOptions::new(&current_database, &current_plugins))
        .await
        .expect("open old current Store");
    old_store
        .create(
            "pre-restore-old",
            "127.0.0.1",
            3307,
            "old-user",
            b"old-password",
        )
        .await
        .expect("seed old state");
    drop(old_store);

    let coordinator = RestoreCoordinator::new(&target_root).expect("restore coordinator");
    let plan = coordinator
        .prepare_restore(&archive, "T130-confirmed-restore-passphrase")
        .await
        .expect("prepare authenticated restore");
    let live = target_root
        .join(".hivegui-db-staging-v1")
        .join(format!("restore-{}", plan.db_instance_operation_id()));
    let instance_manifest: serde_json::Value = serde_json::from_slice(
        &fs::read(live.join(".hivegui-db-instance-v1.json"))
            .expect("unarmed instance manifest must be durable before apply"),
    )
    .expect("parse unarmed instance manifest");
    assert_eq!(instance_manifest["schema_version"], 1);
    assert_eq!(instance_manifest["role"], "restore");
    assert_eq!(
        instance_manifest["db_instance_operation_id"],
        plan.db_instance_operation_id()
    );
    assert_eq!(
        instance_manifest["db_id"],
        format!("restore/{}", plan.db_instance_operation_id())
    );
    assert_eq!(instance_manifest["database_name"], "datasources.db");
    assert_eq!(instance_manifest["ownership_state"], "unarmed");
    assert_eq!(
        instance_manifest["cleanup_operation_id"],
        plan.cleanup_operation_id()
    );
    assert_ne!(
        plan.cleanup_operation_id(),
        plan.db_instance_operation_id(),
        "instance and cleanup lifetimes require distinct UUIDs"
    );
    assert!(!live.join(".hivegui-db-instance-v1.json.staging").exists());
    assert!(!live.join(".hivegui-db-recovery-v1.json").exists());
    assert!(!live.join(".hivegui-db-recovery-v1.json.staging").exists());
    let applied = coordinator
        .apply_restore(&plan)
        .await
        .expect("apply complete replacement");
    assert_eq!(applied.retirement_outcome(), RetirementOutcome::New);
    assert!(applied.retirement_is_done());
    assert!(applied.store_may_open());

    let restored = Store::open_local(StoreOpenOptions::new(&current_database, &current_plugins))
        .await
        .expect("open committed new current");
    let names = restored
        .list()
        .await
        .expect("list committed state")
        .into_iter()
        .map(|row| row.name)
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["restored-new"]);
    assert!(
        target_root.join("backups").is_dir(),
        "the verified pre-switch safety snapshot remains available"
    );
    assert_eq!(
        fs::read_dir(target_root.join(".hivegui-db-staging-v1"))
            .expect("registry remains discoverable")
            .count(),
        0,
        "committed instance and retirement journal must be fully retired"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn restore_safety_snapshot_hashes_complete_database_and_owned_plugin_tree() {
    let source = TestWorkspace::new().expect("source workspace");
    let source_database = write_seed_database(&source).await;
    let archive = unique_target_path(&source, "safety-snapshot-completeness");
    BackupExporter::new(&source_database)
        .export_age(&archive, "T130-safety-snapshot-passphrase")
        .await
        .expect("export replacement state");

    let target = TestWorkspace::new().expect("target workspace");
    let target_root = target.root().join("safety-current-root");
    fs::create_dir(&target_root).expect("create exact current root");
    let current_database = target_root.join("datasources.db");
    let current_plugins = target_root.join("plugins");
    let old_store = Store::open_local(StoreOpenOptions::new(&current_database, &current_plugins))
        .await
        .expect("open old current Store");
    old_store
        .create(
            "safety-old-current",
            "127.0.0.1",
            3307,
            "old-user",
            b"old-secret",
        )
        .await
        .expect("seed old current entity");
    let artifact_key = "safety-old/1.0.0/plugin.wasm";
    let artifact_path = current_plugins.join(artifact_key);
    fs::create_dir_all(artifact_path.parent().expect("artifact parent"))
        .expect("create old Plugin directories");
    let wasm = b"\0asmT130-raw-safety-owned-wasm";
    fs::write(&artifact_path, wasm).expect("write old managed WASM");
    let wasm_sha256 = hex::encode(Sha256::digest(wasm));
    sqlx::query(
        "INSERT INTO plugins (identifier,name,version,s3_key,sha256,size_bytes,runtime,created_at,updated_at) \
         VALUES ('safety-old','Safety Old','1.0.0',?,?,?,'wasm32','2026-08-26T00:00:00Z','2026-08-26T00:00:00Z')",
    )
    .bind(artifact_key)
    .bind(&wasm_sha256)
    .bind(wasm.len() as i64)
    .execute(old_store.pool())
    .await
    .expect("seed old Plugin ownership");
    sqlx::query(
        "INSERT INTO plugin_artifact_operations \
         (operation_id,kind,staging_name,state,created_at,updated_at) \
         VALUES ('00000000-0000-4000-8000-000000001300','create','t130-safety-ledger','done','2026-08-26T00:00:00Z','2026-08-26T00:00:00Z')",
    )
    .execute(old_store.pool())
    .await
    .expect("seed old operation ledger");
    sqlx::query(
        "INSERT INTO plugin_artifact_gc \
         (artifact_key,source_operation_id,last_attempt_at,reason,state) \
         VALUES ('safety-obsolete/plugin.wasm','00000000-0000-4000-8000-000000001300',0,'safety-test','pending')",
    )
    .execute(old_store.pool())
    .await
    .expect("seed old GC ledger");
    old_store.pool().close().await;
    drop(old_store);
    let old_database_sha256 = sha256_path(&current_database);

    let coordinator = RestoreCoordinator::new(&target_root).expect("restore coordinator");
    let plan = coordinator
        .prepare_restore(&archive, "T130-safety-snapshot-passphrase")
        .await
        .expect("prepare replacement");
    coordinator
        .apply_restore(&plan)
        .await
        .expect("complete replacement and verified safety snapshot");

    let snapshots = target_root.join("backups");
    let mut snapshot_entries = fs::read_dir(&snapshots)
        .expect("list safety snapshots")
        .collect::<Result<Vec<_>, _>>()
        .expect("read safety snapshot entry");
    assert_eq!(snapshot_entries.len(), 1, "one operation owns one snapshot");
    let snapshot = snapshot_entries.pop().expect("single snapshot").path();
    let manifest: serde_json::Value = serde_json::from_slice(
        &fs::read(snapshot.join("manifest.json")).expect("read safety manifest"),
    )
    .expect("parse safety manifest");
    let database_descriptor = manifest["database"]
        .as_object()
        .expect("safety manifest must bind the raw database file");
    assert_eq!(database_descriptor["path"], "datasources.db");
    assert_eq!(database_descriptor["sha256"], old_database_sha256);
    assert_eq!(
        database_descriptor["size_bytes"],
        fs::metadata(snapshot.join("datasources.db"))
            .expect("snapshot database metadata")
            .len()
    );
    let plugin_descriptors = manifest["plugin_files"]
        .as_array()
        .expect("Plugin entries must be hash-bound descriptors");
    assert_eq!(plugin_descriptors.len(), 1);
    assert_eq!(
        plugin_descriptors[0]["path"],
        format!("plugins/{artifact_key}")
    );
    assert_eq!(plugin_descriptors[0]["sha256"], wasm_sha256);
    assert_eq!(plugin_descriptors[0]["size_bytes"], wasm.len() as u64);
    assert!(
        plugin_descriptors[0]["source_identity"]
            .as_str()
            .is_some_and(|identity| !identity.is_empty())
    );
    assert!(
        plugin_descriptors[0]["snapshot_identity"]
            .as_str()
            .is_some_and(|identity| !identity.is_empty())
    );
    assert_eq!(
        fs::read(snapshot.join("plugins").join(artifact_key)).expect("read safety snapshot WASM"),
        wasm
    );
    let snapshot_options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(snapshot.join("datasources.db"))
        .create_if_missing(false)
        .read_only(true);
    let snapshot_pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(snapshot_options)
        .await
        .expect("open raw safety database");
    let ledgers: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM plugin_artifact_operations), \
                (SELECT COUNT(*) FROM plugin_artifact_gc)",
    )
    .fetch_one(&snapshot_pool)
    .await
    .expect("read preserved internal ledgers");
    snapshot_pool.close().await;
    assert_eq!(
        ledgers,
        (1, 1),
        "raw safety snapshot preserves internal tables"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn applying_recovery_rebuilds_complete_old_state_from_verified_safety_snapshot() {
    let source = TestWorkspace::new().expect("source workspace");
    let source_database = write_seed_database(&source).await;
    let archive = unique_target_path(&source, "safety-fallback");
    BackupExporter::new(&source_database)
        .export_age(&archive, "T130-safety-fallback-passphrase")
        .await
        .expect("export replacement state");

    let target = TestWorkspace::new().expect("target workspace");
    let root = target.root().join("safety-fallback-root");
    fs::create_dir_all(&root).expect("create target root");
    let current_database = root.join("datasources.db");
    let current_plugins = root.join("plugins");
    let old_store = Store::open_local(StoreOpenOptions::new(&current_database, &current_plugins))
        .await
        .expect("open old current");
    old_store
        .create(
            "old-from-safety",
            "127.0.0.1",
            3306,
            "old-user",
            b"old-password",
        )
        .await
        .expect("seed old database");
    let wasm = include_bytes!("fixtures/plugins/shared-smoke/plugin.wasm");
    let wasm_sha256 = hex::encode(Sha256::digest(wasm));
    let old_wasm = current_plugins.join("old-plugin/1/plugin.wasm");
    fs::create_dir_all(old_wasm.parent().expect("old Plugin parent"))
        .expect("create old Plugin tree");
    fs::write(&old_wasm, wasm).expect("write old Plugin artifact");
    sqlx::query(
        "INSERT INTO plugins \
         (identifier,name,version,s3_key,sha256,size_bytes,runtime,created_at,updated_at) \
         VALUES ('old-plugin','Old Plugin','1.0.0','old-plugin/1/plugin.wasm',?,?,\
         'extism','2026-08-26T00:00:00Z','2026-08-26T00:00:00Z')",
    )
    .bind(&wasm_sha256)
    .bind(wasm.len() as i64)
    .execute(old_store.pool())
    .await
    .expect("seed old Plugin ownership");
    old_store.pool().close().await;
    drop(old_store);

    let coordinator = RestoreCoordinator::new(&root).expect("restore coordinator");
    let plan = coordinator
        .prepare_restore(&archive, "T130-safety-fallback-passphrase")
        .await
        .expect("prepare authenticated restore");
    let point = RESTORE_SWITCH_CRASH_POINTS
        .iter()
        .copied()
        .find(|point| point.as_str() == "plugin_tree_switch")
        .expect("Plugin switch crash point");
    let interrupted = coordinator
        .apply_with_crash(&plan, point)
        .await
        .expect_err("interrupt after both database and Plugin switch");
    assert!(interrupted.reached_requested_boundary());

    let live = root
        .join(".hivegui-db-staging-v1")
        .join(format!("restore-{}", plan.db_instance_operation_id()));
    fs::remove_file(live.join("old-datasources.db")).expect("simulate unavailable old database");
    fs::remove_dir_all(live.join("old-plugins")).expect("simulate unavailable old Plugin tree");

    let replay = RestoreCoordinator::new(&root)
        .expect("restart coordinator")
        .recover_startup()
        .await
        .expect("verified safety snapshot must rebuild old state");
    assert_eq!(replay.retirement_outcome(), RetirementOutcome::Old);
    let reopened = Store::open_local(StoreOpenOptions::new(&current_database, &current_plugins))
        .await
        .expect("reopen safety-restored old current");
    let names = reopened
        .list()
        .await
        .expect("list safety-restored database")
        .into_iter()
        .map(|row| row.name)
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["old-from-safety"]);
    assert_eq!(
        fs::read(current_plugins.join("old-plugin/1/plugin.wasm"))
            .expect("safety-restored Plugin artifact"),
        wasm
    );
}

#[tokio::test(flavor = "current_thread")]
async fn closed_checkpoint_converges_a_proven_safe_residual_through_cleanup_journal() {
    let source = TestWorkspace::new().expect("source workspace");
    let source_database = write_seed_database(&source).await;
    let archive = unique_target_path(&source, "safe-sidecar-cleanup");
    BackupExporter::new(&source_database)
        .export_age(&archive, "T130-safe-sidecar-passphrase")
        .await
        .expect("export replacement state");

    let target = TestWorkspace::new().expect("target workspace");
    let target_root = target.root().join("safe-sidecar-current");
    fs::create_dir(&target_root).expect("create target root");
    let current_database = target_root.join("datasources.db");
    let current_plugins = target_root.join("plugins");
    let old_store = Store::open_local(StoreOpenOptions::new(&current_database, &current_plugins))
        .await
        .expect("open old current");
    old_store
        .create(
            "old-before-safe-sidecar",
            "127.0.0.1",
            3307,
            "old-user",
            b"old-password",
        )
        .await
        .expect("seed old current");
    drop(old_store);

    let coordinator = RestoreCoordinator::new(&target_root).expect("restore coordinator");
    let plan = coordinator
        .prepare_restore(&archive, "T130-safe-sidecar-passphrase")
        .await
        .expect("prepare unarmed restore");
    let live = target_root
        .join(".hivegui-db-staging-v1")
        .join(format!("restore-{}", plan.db_instance_operation_id()));
    let residual = live.join("datasources.db-journal");
    fs::write(&residual, [0_u8; 128]).expect("seed non-hot rollback-journal residual");

    let applied = coordinator
        .apply_restore(&plan)
        .await
        .expect("successful checkpoint proves and durably cleans the residual");
    assert_eq!(applied.retirement_outcome(), RetirementOutcome::New);
    assert!(!residual.exists());
    assert!(
        fs::read_dir(&target_root)
            .expect("scan current root controls")
            .all(|entry| {
                let name = entry
                    .expect("current root entry")
                    .file_name()
                    .to_string_lossy()
                    .to_string();
                !name.starts_with(".hivegui-sidecar-cleanup-v1-")
                    && !name.starts_with(".hivegui-sidecar-quarantine-v1-")
            }),
        "journal and quarantine must be durably retired before Store opens"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn every_sidecar_cleanup_durability_boundary_replays_without_switching_current() {
    let source = TestWorkspace::new().expect("source workspace");
    let source_database = write_seed_database(&source).await;
    let archive = unique_target_path(&source, "sidecar-crash-matrix");
    BackupExporter::new(&source_database)
        .export_age(&archive, "T130-sidecar-crash-passphrase")
        .await
        .expect("export replacement state");

    for point in SIDECAR_CLEANUP_CRASH_POINTS {
        eprintln!("sidecar crash boundary: {}", point.as_str());
        let target = TestWorkspace::new().expect("isolated sidecar crash workspace");
        let root = target.root().join(point.as_str());
        fs::create_dir_all(&root).expect("create current root");
        let current_database = root.join("datasources.db");
        let current_plugins = root.join("plugins");
        let old_store =
            Store::open_local(StoreOpenOptions::new(&current_database, &current_plugins))
                .await
                .expect("open old current");
        old_store
            .create(
                "old-before-sidecar-crash",
                "127.0.0.1",
                3306,
                "old-user",
                b"old-password",
            )
            .await
            .expect("seed old current");
        old_store.pool().close().await;
        drop(old_store);
        let old_sha256 = sha256_path(&current_database);

        let coordinator = RestoreCoordinator::new(&root).expect("restore coordinator");
        let plan = coordinator
            .prepare_restore(&archive, "T130-sidecar-crash-passphrase")
            .await
            .expect("prepare authenticated restore");
        let residual = root.join("datasources.db-journal");
        let residual_bytes = vec![0_u8; 128];
        fs::write(&residual, &residual_bytes).expect("plant proven closed residual");

        let interrupted = coordinator
            .apply_with_crash(&plan, point)
            .await
            .expect_err("the requested cleanup durability point must interrupt");
        assert_eq!(interrupted.crash_point(), point.as_str());
        assert!(
            interrupted.reached_requested_boundary(),
            "cleanup must execute the real durability boundary {}: {interrupted}",
            point.as_str()
        );
        assert_eq!(
            sha256_path(&current_database),
            old_sha256,
            "cleanup interruption must not switch current"
        );

        let replay = RestoreCoordinator::new(&root)
            .expect("restart coordinator")
            .recover_startup()
            .await;
        if matches!(
            point.as_str(),
            "sidecar_prepared_staging_fsync"
                | "sidecar_quarantined_staging_fsync"
                | "sidecar_done_staging_fsync"
        ) {
            let error = replay.expect_err("ambiguous staging state must remain fail-closed");
            let expected_reason = if point.as_str() == "sidecar_prepared_staging_fsync" {
                "sidecar_recoverable"
            } else {
                "sidecar_unknown_owner"
            };
            assert_eq!(
                error.to_string(),
                format!(
                    "storage_recovery_blocked {{ reason: {expected_reason}, artifact: rollback_journal }}"
                )
            );
            if point.as_str() == "sidecar_prepared_staging_fsync" {
                assert_eq!(
                    fs::read(&residual).expect("canonical residual retained"),
                    residual_bytes
                );
            }
            continue;
        }
        let replay = replay.unwrap_or_else(|error| {
            panic!(
                "durable cleanup state {} must replay to old current: {error}",
                point.as_str()
            )
        });
        assert_eq!(
            replay.retirement_outcome(),
            RetirementOutcome::AbortedPreSwitch
        );
        assert!(replay.store_may_open());
        assert!(!residual.exists());
        let reopened =
            Store::open_local(StoreOpenOptions::new(&current_database, &current_plugins))
                .await
                .expect("reopen old current after cleanup replay");
        let names = reopened
            .list()
            .await
            .expect("read replay-selected old state")
            .into_iter()
            .map(|row| row.name)
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["old-before-sidecar-crash"]);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn restore_refuses_open_store_clones_before_freezing_or_checkpointing_current() {
    let source = TestWorkspace::new().expect("source workspace");
    let source_database = write_seed_database(&source).await;
    let archive = unique_target_path(&source, "connections-open");
    BackupExporter::new(&source_database)
        .export_age(&archive, "T130-connections-open-passphrase")
        .await
        .expect("export replacement state");

    let target = TestWorkspace::new().expect("target workspace");
    let root = target.root().join("connections-open-root");
    fs::create_dir_all(&root).expect("create restore root");
    let database = root.join("datasources.db");
    let plugin_root = root.join("plugins");
    let store = Store::open_local(StoreOpenOptions::new(&database, &plugin_root))
        .await
        .expect("open current Store");
    store
        .create(
            "old-current",
            "127.0.0.1",
            3306,
            "old-user",
            b"old-password",
        )
        .await
        .expect("seed current state");
    let retained_clone = store.clone();
    drop(store);

    let coordinator = RestoreCoordinator::new(&root).expect("restore coordinator");
    let plan = coordinator
        .prepare_restore(&archive, "T130-connections-open-passphrase")
        .await
        .expect("prepare authenticated restore");
    let error = coordinator
        .apply_restore(&plan)
        .await
        .expect_err("an open Store clone must block the closed-database boundary");
    assert_eq!(
        error.to_string(),
        "storage_recovery_blocked { reason: connections_open, artifact: connection }"
    );
    retained_clone
        .create(
            "still-open-after-refusal",
            "127.0.0.1",
            3307,
            "old-user",
            b"still-writable",
        )
        .await
        .expect("refusal occurs before the write gate is frozen");
    let names = retained_clone
        .list()
        .await
        .expect("read unchanged current")
        .into_iter()
        .map(|row| row.name)
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["old-current", "still-open-after-refusal"]);
}

#[tokio::test(flavor = "current_thread")]
async fn restore_reports_checkpoint_busy_without_reclassifying_or_switching_current() {
    let source = TestWorkspace::new().expect("source workspace");
    let source_database = write_seed_database(&source).await;
    let archive = unique_target_path(&source, "checkpoint-busy");
    BackupExporter::new(&source_database)
        .export_age(&archive, "T130-checkpoint-busy-passphrase")
        .await
        .expect("export replacement state");

    let target = TestWorkspace::new().expect("target workspace");
    let root = target.root().join("checkpoint-busy-root");
    fs::create_dir_all(&root).expect("create restore root");
    let database = root.join("datasources.db");
    let plugin_root = root.join("plugins");
    let store = Store::open_local(StoreOpenOptions::new(&database, &plugin_root))
        .await
        .expect("open old current Store");
    store
        .create(
            "old-current",
            "127.0.0.1",
            3306,
            "old-user",
            b"old-password",
        )
        .await
        .expect("seed old current");
    store.pool().close().await;
    drop(store);

    let coordinator = RestoreCoordinator::new(&root).expect("restore coordinator");
    let plan = coordinator
        .prepare_restore(&archive, "T130-checkpoint-busy-passphrase")
        .await
        .expect("prepare authenticated restore");
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&database)
        .create_if_missing(false)
        .foreign_keys(true);
    let reader_pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options.clone())
        .await
        .expect("open raw reader");
    let writer_pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .expect("open raw writer");
    sqlx::query("PRAGMA journal_mode=WAL")
        .execute(&writer_pool)
        .await
        .expect("force WAL mode");
    let mut reader = reader_pool.acquire().await.expect("acquire reader");
    sqlx::query("BEGIN")
        .execute(&mut *reader)
        .await
        .expect("start reader snapshot");
    let _: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM data_sources")
        .fetch_one(&mut *reader)
        .await
        .expect("materialize reader snapshot");
    sqlx::query(
        "INSERT INTO data_sources \
         (name,host,port,username,encrypted_password,created_at,updated_at) \
         VALUES ('committed-in-wal','127.0.0.1',3307,'wal-user',X'01',CURRENT_TIMESTAMP,CURRENT_TIMESTAMP)",
    )
    .execute(&writer_pool)
    .await
    .expect("commit a frame newer than the reader snapshot");
    writer_pool.close().await;
    let wal = std::path::PathBuf::from(format!("{}-wal", database.display()));
    let wal_before = fs::read(&wal).expect("hot WAL remains present");

    let error = coordinator
        .apply_restore(&plan)
        .await
        .expect_err("busy checkpoint must block before switching current");
    assert_eq!(
        error.to_string(),
        "storage_recovery_blocked { reason: checkpoint_busy, artifact: checkpoint }"
    );
    assert_eq!(
        fs::read(&wal).expect("busy WAL remains canonical"),
        wal_before,
        "a busy checkpoint must not route the WAL through cleanup"
    );
    sqlx::query("ROLLBACK")
        .execute(&mut *reader)
        .await
        .expect("release reader snapshot");
    drop(reader);
    reader_pool.close().await;
}

#[tokio::test(flavor = "current_thread")]
async fn restore_reports_checkpoint_failed_without_leaking_sqlite_text_or_switching_current() {
    let source = TestWorkspace::new().expect("source workspace");
    let source_database = write_seed_database(&source).await;
    let archive = unique_target_path(&source, "checkpoint-failed");
    BackupExporter::new(&source_database)
        .export_age(&archive, "T130-checkpoint-failed-passphrase")
        .await
        .expect("export replacement state");

    let target = TestWorkspace::new().expect("target workspace");
    let root = target.root().join("checkpoint-failed-root");
    fs::create_dir_all(&root).expect("create restore root");
    let database = root.join("datasources.db");
    let plugin_root = root.join("plugins");
    let store = Store::open_local(StoreOpenOptions::new(&database, &plugin_root))
        .await
        .expect("open old current Store");
    store.pool().close().await;
    drop(store);

    let coordinator = RestoreCoordinator::new(&root).expect("restore coordinator");
    let plan = coordinator
        .prepare_restore(&archive, "T130-checkpoint-failed-passphrase")
        .await
        .expect("prepare authenticated restore");
    let corrupt = b"not-a-sqlite-database-and-never-a-secret";
    fs::write(&database, corrupt).expect("replace current with corrupt fixture");

    let error = coordinator
        .apply_restore(&plan)
        .await
        .expect_err("non-busy checkpoint failure must block");
    assert_eq!(
        error.to_string(),
        "storage_recovery_blocked { reason: checkpoint_failed, artifact: checkpoint }"
    );
    assert_eq!(
        fs::read(&database).expect("corrupt current retained"),
        corrupt,
        "checkpoint failure must not switch or rewrite current"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn startup_replay_restores_prepared_old_but_finishes_committed_new() {
    let source = TestWorkspace::new().expect("source workspace");
    let source_store = Store::open_local(StoreOpenOptions::new(
        source.database_path(),
        source.plugin_root(),
    ))
    .await
    .expect("open source Store");
    source_store
        .create(
            "new-from-archive",
            "127.0.0.1",
            3306,
            "new",
            b"new-password",
        )
        .await
        .expect("seed new archive row");
    let archive = unique_target_path(&source, "owner-replay");
    BackupExporter::new(source.database_path())
        .export_age(&archive, "T130-owner-replay-passphrase")
        .await
        .expect("export new state");
    drop(source_store);

    for phase in ["prepared", "committed"] {
        let target = TestWorkspace::new().expect("isolated target");
        let root = target.root().join(format!("{phase}-current"));
        fs::create_dir_all(&root).expect("create current root");
        let current_database = root.join("datasources.db");
        let current_plugins = root.join("plugins");
        let old_store =
            Store::open_local(StoreOpenOptions::new(&current_database, &current_plugins))
                .await
                .expect("open old Store");
        old_store
            .create(
                "old-before-restore",
                "127.0.0.1",
                3307,
                "old",
                b"old-password",
            )
            .await
            .expect("seed old row");
        drop(old_store);
        let coordinator = RestoreCoordinator::new(&root).expect("coordinator");
        let plan = coordinator
            .prepare_restore(&archive, "T130-owner-replay-passphrase")
            .await
            .expect("prepare restore");
        let point_name = if phase == "prepared" {
            "owner_prepared_publish"
        } else {
            "owner_committed_publish"
        };
        let point = RESTORE_SWITCH_CRASH_POINTS
            .iter()
            .copied()
            .find(|point| point.as_str() == point_name)
            .expect("owner phase crash point");
        let interrupted = coordinator
            .apply_with_crash(&plan, point)
            .await
            .expect_err("interrupt at durable owner phase");
        assert!(interrupted.reached_requested_boundary());

        let replay = RestoreCoordinator::new(&root)
            .expect("restart coordinator")
            .recover_startup()
            .await
            .expect("replay owner state");
        assert_eq!(
            replay.retirement_outcome(),
            if phase == "prepared" {
                RetirementOutcome::Old
            } else {
                RetirementOutcome::New
            }
        );
        let reopened =
            Store::open_local(StoreOpenOptions::new(&current_database, &current_plugins))
                .await
                .expect("reopen proven outcome");
        let names = reopened
            .list()
            .await
            .expect("list proven outcome")
            .into_iter()
            .map(|row| row.name)
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            if phase == "prepared" {
                vec!["old-before-restore"]
            } else {
                vec!["new-from-archive"]
            }
        );
        assert_eq!(
            fs::read_dir(root.join(".hivegui-db-staging-v1"))
                .expect("registry")
                .count(),
            0
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn backup_media_canary_is_absent_after_success_error_crash_and_cross_device_restore() {
    let source = TestWorkspace::new().expect("source workspace");
    let target = TestWorkspace::new().expect("target workspace");
    let source_store = Store::open_local(StoreOpenOptions::new(
        source.database_path(),
        source.plugin_root(),
    ))
    .await
    .expect("open source Store");
    let canary = place_canary_for_test(
        SensitiveField::DataSourcePassword,
        &unique_canary_payload("T119-backup-all-media"),
    )
    .expect("backup canary");
    source_store
        .create(
            "backup-canary",
            "127.0.0.1",
            3306,
            "canary-user",
            canary.plaintext_token().as_bytes(),
        )
        .await
        .expect("persist encrypted source canary");
    let archive = source.root().join("backups/final/t119-canary.age");
    let coordinator = BackupCoordinator::from_store(source_store).expect("backup coordinator");
    let preview = coordinator
        .preview_export(&archive)
        .await
        .expect("preview canary export");
    coordinator
        .confirm_export(preview, "T119-canary-passphrase")
        .await
        .expect("confirmed canary export");

    let restore = RestoreCoordinator::new(target.root()).expect("restore coordinator");
    let wrong = restore
        .prepare_restore(&archive, "wrong-passphrase")
        .await
        .expect_err("wrong passphrase must fail before staging plaintext");
    assert!(!wrong.to_string().contains(&canary.plaintext_token()));
    let plan = restore
        .prepare_restore(&archive, "T119-canary-passphrase")
        .await
        .expect("prepare cross-device restore");
    restore
        .apply_with_crash(&plan, RESTORE_SWITCH_CRASH_POINTS[1])
        .await
        .expect_err("inject crash after the durable prepared owner");
    let replay = RestoreCoordinator::new(target.root())
        .expect("restart restore coordinator")
        .recover_startup()
        .await
        .expect("replay crash without plaintext residue");
    assert!(replay.store_may_open());

    for root in [source.root(), target.root()] {
        let scan = scan_all_mediums_for_test(root, &canary).expect("scan all backup media");
        assert!(
            scan.hits.is_empty(),
            "backup canary leaked through success/error/crash/cross-device path: {:?}",
            scan.hits
        );
    }
}

#[cfg(target_os = "linux")]
#[tokio::test(flavor = "current_thread")]
async fn checkpoint_uses_pinned_staging_and_current_leaves_during_a_b_a_exchange() {
    use std::os::unix::fs::MetadataExt as _;

    const PASSPHRASE: &str = "T119-T130-pinned-checkpoint-leaf-passphrase";

    assert_current_snapshot_copy_seam_source_contract();

    let source = TestWorkspace::new().expect("checkpoint source workspace");
    let archive = export_single_data_source_archive(
        &source,
        "pinned-checkpoint-leaf",
        "checkpoint-source-row",
        PASSPHRASE,
    )
    .await;

    let target = TestWorkspace::new().expect("checkpoint target workspace");
    let root = target.root().join("pinned-checkpoint-leaf-root");
    fs::create_dir_all(&root).expect("create checkpoint target root");
    let current_database = root.join("datasources.db");
    let current = Store::open_local(StoreOpenOptions::for_root(&root))
        .await
        .expect("open owner-aware checkpoint target");
    current
        .create(
            "checkpoint-current-row",
            "127.0.0.1",
            3306,
            "checkpoint-user",
            b"checkpoint-password",
        )
        .await
        .expect("seed checkpoint current state");
    let stale = current.clone();
    let coordinator =
        RestoreCoordinator::from_store(current).expect("bind checkpoint restore coordinator");
    let prepared = coordinator
        .preview_restore(&archive, PASSPHRASE)
        .await
        .expect("prepare authenticated checkpoint replacement");
    let live = root
        .join(".hivegui-db-staging-v1")
        .join(format!("restore-{}", prepared.db_instance_operation_id()));
    let staging_database = live.join("datasources.db");
    let staging_competitor = live.join("staging-checkpoint-competitor.db");
    let current_competitor = root.join("current-checkpoint-competitor.db");
    fs::write(&staging_competitor, b"foreign staging checkpoint leaf")
        .expect("write staging checkpoint competitor");
    fs::write(&current_competitor, b"foreign current checkpoint leaf")
        .expect("write current checkpoint competitor");

    let staging_metadata =
        fs::metadata(&staging_database).expect("record exact staging database identity");
    let staging_identity = (staging_metadata.dev(), staging_metadata.ino());
    let current_metadata =
        fs::metadata(&current_database).expect("record exact current database identity");
    let current_identity = (current_metadata.dev(), current_metadata.ino());
    let staging_competitor_before = fs::read(&staging_competitor)
        .expect("record staging checkpoint competitor bytes");
    let current_competitor_before =
        fs::read(&current_competitor).expect("record current checkpoint competitor bytes");

    let confirmation = coordinator
        .begin_confirmation(&prepared)
        .expect("freeze exact Store for pinned checkpoint proof");
    assert!(stale.pool().is_closed());

    // Staging uses barriers 0..=3 and current uses 4..=7. For each leaf:
    // pinned before SQLx I/O -> release with B at the canonical name ->
    // checkpoint/health/close complete on A -> restore A before continuing.
    let barriers: [std::sync::Arc<tokio::sync::Barrier>; 8] =
        std::array::from_fn(|_| std::sync::Arc::new(tokio::sync::Barrier::new(2)));
    let finish_barriers = barriers.clone();
    let mut finish_task = tokio::spawn(async move {
        confirmation
            .finish_with_pinned_checkpoint_interlocks_for_test(finish_barriers)
            .await
    });

    tokio::time::timeout(Duration::from_secs(10), barriers[0].wait())
        .await
        .expect("staging leaf is pinned before SQLx checkpoint");
    assert!(
        open_linux_fd_count_for_identity(staging_identity) >= 1,
        "production must retain the exact staging inode before checkpoint"
    );
    let staging_watch = DataIoWatch::new(&staging_competitor);
    assert!(staging_watch.drain_masks().is_empty());
    let staging_exchange = PathExchangeGuard::new(&staging_database, &staging_competitor);
    barriers[1].wait().await;
    tokio::time::timeout(Duration::from_secs(10), barriers[2].wait())
        .await
        .expect("staging checkpoint completes through its pinned leaf");
    let staging_competitor_masks = staging_watch.drain_masks();
    assert!(
        staging_competitor_masks.is_empty(),
        "SQLx staging checkpoint touched the competitor inode: {staging_competitor_masks:?}"
    );
    staging_exchange.restore();
    assert_eq!(
        fs::read(&staging_competitor).expect("read untouched staging competitor"),
        staging_competitor_before
    );
    barriers[3].wait().await;

    tokio::time::timeout(Duration::from_secs(10), barriers[4].wait())
        .await
        .expect("current leaf is pinned before SQLx checkpoint");
    assert!(
        open_linux_fd_count_for_identity(current_identity) >= 1,
        "production must retain the exact current inode before checkpoint"
    );
    let current_watch = DataIoWatch::new(&current_competitor);
    assert!(current_watch.drain_masks().is_empty());
    let current_exchange = PathExchangeGuard::new(&current_database, &current_competitor);
    barriers[5].wait().await;
    tokio::time::timeout(Duration::from_secs(10), barriers[6].wait())
        .await
        .expect("current checkpoint completes through its pinned leaf");
    let current_competitor_masks = current_watch.drain_masks();
    assert!(
        current_competitor_masks.is_empty(),
        "SQLx current checkpoint touched the competitor inode: {current_competitor_masks:?}"
    );
    current_exchange.restore();
    assert_eq!(
        fs::read(&current_competitor).expect("read untouched current competitor"),
        current_competitor_before
    );
    barriers[7].wait().await;

    let recovery = tokio::time::timeout(Duration::from_secs(30), &mut finish_task)
        .await
        .expect("bounded pinned-leaf restore completion")
        .expect("pinned-leaf restore task must not panic")
        .expect("pinned staging/current checkpoint restore must succeed");
    assert_eq!(recovery.retirement_outcome(), RetirementOutcome::New);
    assert_eq!(
        data_source_names_from_database(&current_database).await,
        vec!["checkpoint-source-row"]
    );

    drop(prepared);
    coordinator
        .recover_startup()
        .await
        .expect("release terminal restore owner after pinned-leaf proof");
    drop(stale);
}
