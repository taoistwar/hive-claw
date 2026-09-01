//! T014 Red contract for the device-local encryption key startup gate.
//!
//! These tests intentionally target the public `key_store` API planned by T025.
//! They must remain compile-Red until that API exists; do not replace them with
//! tests of fixture helpers or private validation functions.

#[path = "support/mod.rs"]
mod support;

use std::{
    env, fs, io,
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    sync::{Arc, Barrier},
    thread,
    time::{Duration, Instant},
};

use hivegui::datasource::key_store::{DeviceKeyBlockKind, DeviceKeyStartup, DeviceKeyStore};
use support::TestWorkspace;

const VALID_KEY_BYTES: [u8; 32] = [0x37; 32];
const CIPHERTEXT_FIXTURE: &[u8] = b"encrypted-sqlite-state-must-not-change";
const KEY_CHILD_PATH: &str = "HIVEGUI_T014_KEY_CHILD_PATH";
const KEY_CHILD_READY: &str = "HIVEGUI_T014_KEY_CHILD_READY";
const KEY_CHILD_START: &str = "HIVEGUI_T014_KEY_CHILD_START";
const KEY_CHILD_RESULT: &str = "HIVEGUI_T014_KEY_CHILD_RESULT";

fn ready_fingerprint(startup: DeviceKeyStartup) -> [u8; 32] {
    match startup {
        DeviceKeyStartup::Ready(key) => key.fingerprint(),
        DeviceKeyStartup::Blocked(block) => {
            panic!("new key store unexpectedly blocked: {:?}", block.kind())
        }
    }
}

fn blocked_kind(startup: DeviceKeyStartup) -> DeviceKeyBlockKind {
    match startup {
        DeviceKeyStartup::Ready(_) => panic!("unsafe key state opened the main Store"),
        DeviceKeyStartup::Blocked(block) => {
            assert!(block.is_blocking(), "key recovery must block Store startup");
            block.kind()
        }
    }
}

fn seed_ciphertext(workspace: &TestWorkspace) -> io::Result<Vec<u8>> {
    fs::write(workspace.database_path(), CIPHERTEXT_FIXTURE)?;
    fs::read(workspace.database_path())
}

fn write_key(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn assert_no_key_staging_files(path: &Path) {
    let parent = path.parent().expect("device key has a parent directory");
    let entries = fs::read_dir(parent)
        .expect("read device-key directory")
        .map(|entry| entry.expect("read device-key directory entry").file_name())
        .collect::<Vec<_>>();
    assert_eq!(entries.len(), 1, "no staging files may survive startup");
    assert_eq!(
        entries[0].as_os_str(),
        path.file_name().expect("device key has a name")
    );
}

#[test]
fn first_start_atomically_creates_a_random_owner_only_key_and_restart_reuses_it() {
    let first_workspace = TestWorkspace::new().expect("create first isolated XDG workspace");
    let first_store = DeviceKeyStore::new(first_workspace.device_key_path());
    let first_fingerprint = ready_fingerprint(
        first_store
            .open_for_startup(true)
            .expect("first startup without ciphertext creates a key"),
    );
    let first_bytes =
        fs::read(first_workspace.device_key_path()).expect("read first generated key");

    assert_eq!(first_bytes.len(), 32, "device key has a fixed byte length");
    assert_ne!(first_bytes, [0_u8; 32], "device key is not a zero fixture");
    DeviceKeyStore::validate_owner_only(first_workspace.device_key_path())
        .expect("generated key is readable and writable only by its owner");
    assert_no_key_staging_files(first_workspace.device_key_path());

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(
            fs::metadata(first_workspace.device_key_path())
                .expect("stat generated key")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    let restart_fingerprint = ready_fingerprint(
        DeviceKeyStore::new(first_workspace.device_key_path())
            .open_for_startup(true)
            .expect("restart reopens the existing key"),
    );
    assert_eq!(restart_fingerprint, first_fingerprint);
    assert_eq!(
        fs::read(first_workspace.device_key_path()).expect("read key after restart"),
        first_bytes,
        "restart must not replace key material"
    );

    let second_workspace = TestWorkspace::new().expect("create second isolated XDG workspace");
    let second_fingerprint = ready_fingerprint(
        DeviceKeyStore::new(second_workspace.device_key_path())
            .open_for_startup(true)
            .expect("second device creates its own key"),
    );
    assert_ne!(
        second_fingerprint, first_fingerprint,
        "independent first starts must use system randomness"
    );
}

#[test]
fn concurrent_first_start_converges_on_one_complete_key() {
    const CONTENDERS: usize = 8;

    let workspace = TestWorkspace::new().expect("create isolated XDG workspace");
    let key_path = workspace.device_key_path().to_path_buf();
    let barrier = Arc::new(Barrier::new(CONTENDERS));
    let threads = (0..CONTENDERS)
        .map(|_| {
            let key_path = key_path.clone();
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                ready_fingerprint(
                    DeviceKeyStore::new(&key_path)
                        .open_for_startup(true)
                        .expect("concurrent startup resolves an atomic key"),
                )
            })
        })
        .collect::<Vec<_>>();

    let fingerprints = threads
        .into_iter()
        .map(|thread| thread.join().expect("device-key contender did not panic"))
        .collect::<Vec<_>>();
    assert!(
        fingerprints
            .iter()
            .all(|fingerprint| *fingerprint == fingerprints[0]),
        "every contender must observe the same final key"
    );
    assert_eq!(fs::read(&key_path).expect("read final key").len(), 32);
    assert_no_key_staging_files(&key_path);
}

#[test]
fn child_device_key_creation_probe() {
    let Some(key_path) = env::var_os(KEY_CHILD_PATH).map(PathBuf::from) else {
        return;
    };
    let ready_path = PathBuf::from(env::var_os(KEY_CHILD_READY).expect("child ready path"));
    let start_path = PathBuf::from(env::var_os(KEY_CHILD_START).expect("child start path"));
    let result_path = PathBuf::from(env::var_os(KEY_CHILD_RESULT).expect("child result path"));

    fs::write(&ready_path, b"ready").expect("announce device-key child readiness");
    wait_for_path(&start_path, Duration::from_secs(10));

    let fingerprint = ready_fingerprint(
        DeviceKeyStore::new(&key_path)
            .open_for_startup(true)
            .expect("cross-process first startup resolves one atomic key"),
    );
    fs::write(result_path, fingerprint).expect("write observed key fingerprint");
}

#[test]
fn concurrent_process_first_start_converges_on_one_atomic_key() {
    const CONTENDERS: usize = 8;

    let workspace = TestWorkspace::new().expect("create isolated XDG workspace");
    let process_root = workspace.root().join("device-key-process-race");
    fs::create_dir_all(&process_root).expect("create process coordination directory");
    let start_path = process_root.join("start");

    let mut children = Vec::with_capacity(CONTENDERS);
    let mut ready_paths = Vec::with_capacity(CONTENDERS);
    let mut result_paths = Vec::with_capacity(CONTENDERS);
    for index in 0..CONTENDERS {
        let ready_path = process_root.join(format!("ready-{index}"));
        let result_path = process_root.join(format!("result-{index}"));
        children.push(spawn_key_child(
            workspace.device_key_path(),
            &ready_path,
            &start_path,
            &result_path,
        ));
        ready_paths.push(ready_path);
        result_paths.push(result_path);
    }

    for ready_path in &ready_paths {
        wait_for_path(ready_path, Duration::from_secs(10));
    }
    fs::write(&start_path, b"start").expect("release all key-creation processes");

    for child in children {
        let output = child
            .wait_with_output()
            .expect("wait for device-key child process");
        assert_process_success(&output, "device-key creation contender failed");
    }

    let fingerprints = result_paths
        .iter()
        .map(|path| fs::read(path).expect("read child key fingerprint"))
        .collect::<Vec<_>>();
    assert!(
        fingerprints
            .iter()
            .all(|fingerprint| fingerprint == &fingerprints[0]),
        "all processes must observe the same final key"
    );
    assert_eq!(fingerprints[0].len(), 32);
    assert_eq!(
        fs::read(workspace.device_key_path())
            .expect("read cross-process generated key")
            .len(),
        32
    );
    DeviceKeyStore::validate_owner_only(workspace.device_key_path())
        .expect("cross-process generated key remains owner-only");
    assert_no_key_staging_files(workspace.device_key_path());
}

#[test]
fn existing_ciphertext_and_missing_key_enters_blocking_recovery_without_mutation() {
    let workspace = TestWorkspace::new().expect("create isolated XDG workspace");
    let database_before = seed_ciphertext(&workspace).expect("seed encrypted database bytes");

    let startup = DeviceKeyStore::new(workspace.device_key_path())
        .with_database_path(workspace.database_path())
        .open_for_startup(true)
        .expect("missing key is a classified startup state");

    assert_eq!(blocked_kind(startup), DeviceKeyBlockKind::Missing);
    assert_eq!(DeviceKeyBlockKind::Missing.stable_code(), "key_missing");
    assert!(!workspace.device_key_path().exists());
    assert_eq!(
        fs::read(workspace.database_path()).expect("read unchanged database"),
        database_before
    );
}

#[test]
fn existing_ciphertext_and_corrupt_key_never_generates_a_replacement() {
    let workspace = TestWorkspace::new().expect("create isolated XDG workspace");
    let database_before = seed_ciphertext(&workspace).expect("seed encrypted database bytes");
    let corrupt_key = [0x91_u8; 31];
    write_key(workspace.device_key_path(), &corrupt_key).expect("write corrupt key fixture");

    let startup = DeviceKeyStore::new(workspace.device_key_path())
        .open_for_startup(true)
        .expect("corrupt key is a classified startup state");

    assert_eq!(blocked_kind(startup), DeviceKeyBlockKind::Corrupt);
    assert_eq!(DeviceKeyBlockKind::Corrupt.stable_code(), "key_corrupt");
    assert_eq!(
        fs::read(workspace.device_key_path()).expect("read unchanged corrupt key"),
        corrupt_key
    );
    assert_eq!(
        fs::read(workspace.database_path()).expect("read unchanged database"),
        database_before
    );
    assert_no_key_staging_files(workspace.device_key_path());
}

#[cfg(unix)]
#[test]
fn existing_ciphertext_and_unsafe_permissions_never_opens_or_replaces_the_key() {
    use std::os::unix::fs::PermissionsExt as _;

    let workspace = TestWorkspace::new().expect("create isolated XDG workspace");
    let database_before = seed_ciphertext(&workspace).expect("seed encrypted database bytes");
    write_key(workspace.device_key_path(), &VALID_KEY_BYTES).expect("write key fixture");
    fs::set_permissions(
        workspace.device_key_path(),
        fs::Permissions::from_mode(0o644),
    )
    .expect("make fixture permissions unsafe");

    let startup = DeviceKeyStore::new(workspace.device_key_path())
        .open_for_startup(true)
        .expect("unsafe permissions are a classified startup state");

    assert_eq!(blocked_kind(startup), DeviceKeyBlockKind::Permissions);
    assert_eq!(
        DeviceKeyBlockKind::Permissions.stable_code(),
        "key_permissions"
    );
    assert_eq!(
        fs::read(workspace.device_key_path()).expect("read unchanged key"),
        VALID_KEY_BYTES
    );
    assert_eq!(
        fs::metadata(workspace.device_key_path())
            .expect("stat unchanged key")
            .permissions()
            .mode()
            & 0o777,
        0o644
    );
    assert_eq!(
        fs::read(workspace.database_path()).expect("read unchanged database"),
        database_before
    );
}

#[cfg(unix)]
#[test]
fn existing_ciphertext_and_unreadable_key_path_is_a_stable_blocking_state() {
    use std::os::unix::fs::symlink;

    let workspace = TestWorkspace::new().expect("create isolated XDG workspace");
    let database_before = seed_ciphertext(&workspace).expect("seed encrypted database bytes");
    let key_path = workspace.device_key_path();
    if let Some(parent) = key_path.parent() {
        fs::create_dir_all(parent).expect("ensure device-key parent exists for symlink");
    }
    // Self-pointing symlink: the OS cannot resolve the loop, so
    // any `fs::read` attempt fails with ELOOP. This is the
    // "unreadable" case the recovery view must classify without
    // mutating or replacing the key path.
    symlink(key_path, key_path).expect("create deterministic unreadable symlink loop");

    let startup = DeviceKeyStore::new(key_path)
        .with_database_path(workspace.database_path())
        .open_for_startup(true)
        .expect("an unreadable key path is a classified startup state");

    assert_eq!(blocked_kind(startup), DeviceKeyBlockKind::Unreadable);
    assert_eq!(
        DeviceKeyBlockKind::Unreadable.stable_code(),
        "key_unreadable"
    );
    assert!(
        fs::symlink_metadata(key_path)
            .expect("stat unchanged unreadable key path")
            .file_type()
            .is_symlink(),
        "startup must not replace the unreadable key path"
    );
    assert_eq!(fs::read_link(key_path).unwrap(), key_path);
    assert_eq!(
        fs::read(workspace.database_path()).expect("read unchanged database"),
        database_before
    );
    assert_no_key_staging_files(key_path);
}

fn spawn_key_child(
    key_path: &Path,
    ready_path: &Path,
    start_path: &Path,
    result_path: &Path,
) -> Child {
    Command::new(env::current_exe().expect("resolve current integration-test executable"))
        .args(["--exact", "child_device_key_creation_probe", "--nocapture"])
        .env(KEY_CHILD_PATH, key_path)
        .env(KEY_CHILD_READY, ready_path)
        .env(KEY_CHILD_START, start_path)
        .env(KEY_CHILD_RESULT, result_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn device-key creation child")
}

fn wait_for_path(path: &Path, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for process coordination path {}",
            path.display()
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn assert_process_success(output: &Output, context: &str) {
    assert!(
        output.status.success(),
        "{context}\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
