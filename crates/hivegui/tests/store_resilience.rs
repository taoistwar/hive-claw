//! Red contract for Store startup, locking, retry, and corruption recovery.

mod support;

use std::{
    collections::VecDeque,
    env, fs,
    future::Future,
    path::PathBuf,
    pin::Pin,
    process::{Command, Output},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use hivegui::datasource::{
    migrations::{CURRENT_SCHEMA_VERSION, capture_artifact_snapshot},
    store::{
        CorruptionDecision, DatabaseFailureClass, RetryPolicy, RetrySleeper, Store,
        StoreOpenErrorKind, StoreOpenFaultInjector, StoreOpenOptions, StoreStartupGate,
    },
};
use support::TestWorkspace;

const OWNER_PHASE: &str = "Foundation";
const APPROVAL_TASK: &str = "T012";
const IMPLEMENTATION_TASKS: [&str; 2] = ["T022", "T028"];
const EXPECTED_WAL_AUTOCHECKPOINT_PAGES: i64 = 128;
const STORE_LOCK_CHILD_MODE: &str = "HIVEGUI_T012_STORE_LOCK_CHILD_MODE";
const STORE_LOCK_CHILD_DATABASE: &str = "HIVEGUI_T012_STORE_LOCK_CHILD_DATABASE";
const STORE_LOCK_CHILD_PLUGINS: &str = "HIVEGUI_T012_STORE_LOCK_CHILD_PLUGINS";

fn options(workspace: &TestWorkspace) -> StoreOpenOptions {
    StoreOpenOptions::new(workspace.database_path(), workspace.plugin_root())
}

#[tokio::test]
async fn business_store_configures_durable_bounded_wal_on_every_connection() {
    assert_eq!(OWNER_PHASE, "Foundation");
    assert_eq!(APPROVAL_TASK, "T012");
    assert!(IMPLEMENTATION_TASKS.contains(&"T028"));

    let workspace = TestWorkspace::new().expect("isolated workspace");
    let store = Store::open_local(options(&workspace))
        .await
        .expect("open canonical business Store");
    assert_business_connection_pragmas(&store).await;
    drop(store);

    let reopened = Store::open_existing(workspace.database_path())
        .await
        .expect("open existing business Store");
    assert_business_connection_pragmas(&reopened).await;

    let application_store_root = workspace.root().join("application-store");
    let application_store = Store::new(&application_store_root)
        .await
        .expect("open application business Store");
    assert_business_connection_pragmas(&application_store).await;
}

async fn assert_business_connection_pragmas(store: &Store) {
    let pool = store.pool();
    let connection_count =
        usize::try_from(pool.options().get_max_connections()).expect("pool size fits usize");
    let mut connections = Vec::with_capacity(connection_count);
    for _ in 0..connection_count {
        connections.push(
            pool.acquire()
                .await
                .expect("acquire every pooled connection"),
        );
    }

    for (index, connection) in connections.iter_mut().enumerate() {
        let journal_mode: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(&mut **connection)
            .await
            .expect("read journal mode");
        let foreign_keys: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(&mut **connection)
            .await
            .expect("read foreign-key enforcement");
        let busy_timeout_ms: i64 = sqlx::query_scalar("PRAGMA busy_timeout")
            .fetch_one(&mut **connection)
            .await
            .expect("read busy timeout");
        let synchronous: i64 = sqlx::query_scalar("PRAGMA synchronous")
            .fetch_one(&mut **connection)
            .await
            .expect("read synchronous durability");
        let wal_autocheckpoint_pages: i64 = sqlx::query_scalar("PRAGMA wal_autocheckpoint")
            .fetch_one(&mut **connection)
            .await
            .expect("read WAL auto-checkpoint bound");

        assert_eq!(
            journal_mode.to_ascii_lowercase(),
            "wal",
            "business connection {index} must use WAL"
        );
        assert_eq!(
            foreign_keys, 1,
            "business connection {index} must enforce foreign keys"
        );
        assert!(
            busy_timeout_ms > 0,
            "business connection {index} must fail through a bounded busy timeout"
        );
        assert_eq!(
            synchronous, 2,
            "business connection {index} must retain synchronous=FULL durability"
        );
        assert_eq!(
            wal_autocheckpoint_pages, EXPECTED_WAL_AUTOCHECKPOINT_PAGES,
            "business connection {index} must bound checkpoint work instead of accumulating SQLite's 1000-page default"
        );
    }
}

#[tokio::test]
async fn one_process_owns_the_local_store_write_lock_until_store_drop() {
    assert_eq!(OWNER_PHASE, "Foundation");
    assert_eq!(APPROVAL_TASK, "T012");
    assert_eq!(IMPLEMENTATION_TASKS, ["T022", "T028"]);

    let workspace = TestWorkspace::new().expect("isolated workspace");
    let first = Store::open_local(options(&workspace))
        .await
        .expect("first Store owns the process lock");
    assert_eq!(
        first.schema_version().await.unwrap(),
        CURRENT_SCHEMA_VERSION
    );

    let error = match Store::open_local(options(&workspace)).await {
        Ok(_) => panic!("a second Store must not share the same write database"),
        Err(error) => error,
    };
    assert_eq!(error.kind(), StoreOpenErrorKind::AlreadyLocked);
    assert_eq!(error.database_path(), Some(workspace.database_path()));
    assert!(
        !error
            .to_string()
            .contains(workspace.root().to_string_lossy().as_ref())
    );

    // Rejecting the second owner must not invalidate the first owner.
    assert_eq!(
        first.schema_version().await.unwrap(),
        CURRENT_SCHEMA_VERSION
    );
    drop(first);

    let reopened = Store::open_local(options(&workspace))
        .await
        .expect("dropping the owner releases the lock");
    assert_eq!(
        reopened.schema_version().await.unwrap(),
        CURRENT_SCHEMA_VERSION
    );
}

#[tokio::test]
async fn child_store_lock_probe() {
    let Some(mode) = env::var_os(STORE_LOCK_CHILD_MODE) else {
        return;
    };
    let database = PathBuf::from(
        env::var_os(STORE_LOCK_CHILD_DATABASE).expect("child database path is configured"),
    );
    let plugins = PathBuf::from(
        env::var_os(STORE_LOCK_CHILD_PLUGINS).expect("child Plugin path is configured"),
    );
    let result = Store::open_local(StoreOpenOptions::new(&database, &plugins)).await;

    match mode.to_string_lossy().as_ref() {
        "expect-locked" => {
            let error = match result {
                Ok(_) => panic!("another process opened a Store whose file lock is already held"),
                Err(error) => error,
            };
            assert_eq!(error.kind(), StoreOpenErrorKind::AlreadyLocked);
        }
        "expect-open" => {
            let store = result.expect("child process acquires the released Store file lock");
            assert_eq!(
                store.schema_version().await.unwrap(),
                CURRENT_SCHEMA_VERSION
            );
        }
        other => panic!("unknown Store lock child mode: {other}"),
    }
}

#[tokio::test]
async fn write_lock_is_exclusive_across_processes_and_released_on_owner_exit() {
    let workspace = TestWorkspace::new().expect("isolated workspace");
    let owner = Store::open_local(options(&workspace))
        .await
        .expect("parent process owns the Store file lock");

    let locked = run_store_lock_child(&workspace, "expect-locked");
    assert_child_success(&locked, "a second process must observe AlreadyLocked");
    assert_eq!(
        owner.schema_version().await.unwrap(),
        CURRENT_SCHEMA_VERSION,
        "a rejected process must not invalidate the current owner"
    );

    drop(owner);
    let reopened = run_store_lock_child(&workspace, "expect-open");
    assert_child_success(
        &reopened,
        "the file lock must be available to another process after owner drop",
    );
}

fn run_store_lock_child(workspace: &TestWorkspace, mode: &str) -> Output {
    Command::new(env::current_exe().expect("resolve current integration-test executable"))
        .args(["--exact", "child_store_lock_probe", "--nocapture"])
        .env(STORE_LOCK_CHILD_MODE, mode)
        .env(STORE_LOCK_CHILD_DATABASE, workspace.database_path())
        .env(STORE_LOCK_CHILD_PLUGINS, workspace.plugin_root())
        .output()
        .expect("run isolated Store lock child process")
}

fn assert_child_success(output: &Output, context: &str) {
    assert!(
        output.status.success(),
        "{context}\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn retry_policy_is_exactly_one_two_four_seconds_for_lock_and_file_busy_only() {
    let policy = RetryPolicy::foundation_default();
    assert_eq!(policy.maximum_retries(), 3);
    assert_eq!(
        policy.delays(DatabaseFailureClass::DatabaseLocked),
        [
            Duration::from_secs(1),
            Duration::from_secs(2),
            Duration::from_secs(4)
        ]
    );
    assert_eq!(
        policy.delays(DatabaseFailureClass::FileBusy),
        [
            Duration::from_secs(1),
            Duration::from_secs(2),
            Duration::from_secs(4)
        ]
    );

    for non_retryable in [
        DatabaseFailureClass::Corrupt,
        DatabaseFailureClass::PermissionDenied,
        DatabaseFailureClass::UnsupportedSchema,
        DatabaseFailureClass::InvalidInput,
        DatabaseFailureClass::IntegrityCheckFailed,
    ] {
        assert!(
            policy.delays(non_retryable).is_empty(),
            "{non_retryable:?} must surface immediately"
        );
    }
}

#[test]
fn normal_store_open_uses_the_approved_retry_policy_without_hidden_extra_attempts() {
    let workspace = TestWorkspace::new().expect("isolated workspace");
    let options = options(&workspace);
    let policy = options.retry_policy();

    assert_eq!(policy, RetryPolicy::foundation_default());
    assert_eq!(
        policy.maximum_attempts(),
        4,
        "initial attempt plus three retries"
    );
    assert_eq!(policy.total_delay(), Duration::from_secs(7));
    assert!(!policy.retries_after_success());
}

#[tokio::test]
async fn real_store_open_uses_exact_retry_delays_and_stops_immediately_after_success() {
    for failure in [
        DatabaseFailureClass::DatabaseLocked,
        DatabaseFailureClass::FileBusy,
    ] {
        let workspace = TestWorkspace::new().expect("isolated workspace");
        let faults = Arc::new(SequencedOpenFaults::new([failure, failure, failure]));
        let sleeper = Arc::new(RecordingSleeper::default());
        let configured = options(&workspace)
            .with_open_fault_injector(faults.clone())
            .with_retry_sleeper(sleeper.clone());

        let store = Store::open_local(configured).await.unwrap_or_else(|error| {
            panic!("{failure:?} must succeed after three retries: {error}")
        });

        assert_eq!(
            store.schema_version().await.unwrap(),
            CURRENT_SCHEMA_VERSION
        );
        assert_eq!(faults.attempts(), 4, "initial attempt plus three retries");
        assert_eq!(
            sleeper.delays(),
            [
                Duration::from_secs(1),
                Duration::from_secs(2),
                Duration::from_secs(4)
            ]
        );
    }

    let workspace = TestWorkspace::new().expect("isolated workspace");
    let faults = Arc::new(SequencedOpenFaults::new([
        DatabaseFailureClass::DatabaseLocked,
        DatabaseFailureClass::DatabaseLocked,
    ]));
    let sleeper = Arc::new(RecordingSleeper::default());
    Store::open_local(
        options(&workspace)
            .with_open_fault_injector(faults.clone())
            .with_retry_sleeper(sleeper.clone()),
    )
    .await
    .expect("success on the third attempt stops the retry loop");
    assert_eq!(faults.attempts(), 3);
    assert_eq!(
        sleeper.delays(),
        [Duration::from_secs(1), Duration::from_secs(2)]
    );
}

#[tokio::test]
async fn real_store_open_never_sleeps_or_retries_non_retryable_failures() {
    for failure in [
        DatabaseFailureClass::Corrupt,
        DatabaseFailureClass::PermissionDenied,
        DatabaseFailureClass::InvalidInput,
        DatabaseFailureClass::UnsupportedSchema,
        DatabaseFailureClass::IntegrityCheckFailed,
    ] {
        let workspace = TestWorkspace::new().expect("isolated workspace");
        let faults = Arc::new(SequencedOpenFaults::new([failure]));
        let sleeper = Arc::new(RecordingSleeper::default());

        let error = match Store::open_local(
            options(&workspace)
                .with_open_fault_injector(faults.clone())
                .with_retry_sleeper(sleeper.clone()),
        )
        .await
        {
            Ok(_) => panic!("non-retryable failure must surface on the first attempt"),
            Err(error) => error,
        };

        assert_eq!(error.failure_class(), Some(failure));
        assert_eq!(faults.attempts(), 1, "{failure:?} was retried");
        assert!(
            sleeper.delays().is_empty(),
            "{failure:?} slept before failing"
        );
    }
}

#[tokio::test]
async fn missing_database_is_creation_not_corruption_and_opens_at_v4() {
    let workspace = TestWorkspace::new().expect("isolated workspace");
    assert!(!workspace.database_path().exists());

    let gate = Store::inspect_startup(options(&workspace))
        .await
        .expect("inspect missing database");
    assert_eq!(gate, StoreStartupGate::MissingDatabase);
    assert!(
        !workspace.database_path().exists(),
        "inspection is read-only"
    );

    let store = Store::open_local(options(&workspace))
        .await
        .expect("missing database is created");
    assert_eq!(
        store.schema_version().await.unwrap(),
        CURRENT_SCHEMA_VERSION
    );
}

#[tokio::test]
async fn corrupt_database_is_a_separate_startup_gate_and_inspection_is_non_mutating() {
    let workspace = corrupt_workspace();
    let before_database = fs::read(workspace.database_path()).expect("read corrupt bytes");
    let before_plugins = directory_bytes(workspace.plugin_root());

    let gate = Store::inspect_startup(options(&workspace))
        .await
        .expect("corruption is a classified startup state, not an opaque open error");
    let StoreStartupGate::Corrupt(report) = gate else {
        panic!("expected a dedicated corruption gate, got {gate:?}");
    };

    assert!(report.integrity_check_failed());
    assert_eq!(
        report.allowed_decisions(),
        [
            CorruptionDecision::RestoreBackup,
            CorruptionDecision::RebuildLocalDatabase,
            CorruptionDecision::Exit,
        ]
    );
    assert_eq!(
        fs::read(workspace.database_path()).unwrap(),
        before_database,
        "classification must not repair or replace bytes"
    );
    assert_eq!(directory_bytes(workspace.plugin_root()), before_plugins);
}

#[tokio::test]
async fn normal_open_refuses_corruption_without_implicit_rebuild_or_hiveweb_fallback() {
    let workspace = corrupt_workspace();
    let before_database = fs::read(workspace.database_path()).expect("read corrupt bytes");
    let before_plugins = directory_bytes(workspace.plugin_root());

    let error = match Store::open_local(options(&workspace)).await {
        Ok(_) => panic!("normal open must stop at the recovery gate"),
        Err(error) => error,
    };

    assert_eq!(error.kind(), StoreOpenErrorKind::CorruptDatabase);
    assert_eq!(error.retry_count(), 0, "corruption is not retryable");
    assert!(!error.to_string().contains("http"));
    assert!(!error.to_string().contains("hiveweb"));
    assert_eq!(
        fs::read(workspace.database_path()).unwrap(),
        before_database
    );
    assert_eq!(directory_bytes(workspace.plugin_root()), before_plugins);
}

#[tokio::test]
async fn rebuild_requires_explicit_confirmation_and_quarantines_corrupt_bytes() {
    let workspace = corrupt_workspace();
    let corrupt_bytes = fs::read(workspace.database_path()).expect("read corrupt bytes");
    let plugin_snapshot_before = directory_bytes(workspace.plugin_root());

    let unconfirmed = match Store::recover_corrupt_database(
        options(&workspace),
        CorruptionDecision::RebuildLocalDatabase,
        false,
    )
    .await
    {
        Ok(_) => panic!("rebuild must require explicit user confirmation"),
        Err(error) => error,
    };
    assert_eq!(unconfirmed.kind(), StoreOpenErrorKind::ConfirmationRequired);
    assert_eq!(fs::read(workspace.database_path()).unwrap(), corrupt_bytes);

    let report = Store::recover_corrupt_database(
        options(&workspace),
        CorruptionDecision::RebuildLocalDatabase,
        true,
    )
    .await
    .expect("confirmed rebuild creates a fresh local database");

    assert_eq!(report.schema_version, CURRENT_SCHEMA_VERSION);
    assert!(report.quarantined_database.is_file());
    assert_eq!(
        fs::read(&report.quarantined_database).unwrap(),
        corrupt_bytes,
        "original corrupt evidence must be preserved"
    );
    assert_ne!(
        fs::read(workspace.database_path()).unwrap(),
        corrupt_bytes,
        "active database must be a newly created v4 database"
    );
    assert_eq!(
        directory_bytes(workspace.plugin_root()),
        plugin_snapshot_before
    );

    let snapshot = capture_artifact_snapshot(workspace.database_path(), workspace.plugin_root())
        .await
        .expect("new database and preserved managed plugins validate together");
    assert!(snapshot.database_integrity_ok);
    assert!(snapshot.foreign_keys_ok);
}

fn corrupt_workspace() -> TestWorkspace {
    let workspace = TestWorkspace::new().expect("isolated workspace");
    fs::write(
        workspace.database_path(),
        b"not a sqlite database\0T012 deterministic corrupt fixture",
    )
    .expect("write corrupt database fixture");
    fs::write(
        workspace.plugin_root().join("managed-fixture.wasm"),
        b"\0asmT012-managed-plugin",
    )
    .expect("write managed Plugin fixture");
    workspace
}

fn directory_bytes(root: &std::path::Path) -> Vec<(String, Vec<u8>)> {
    let mut files = fs::read_dir(root)
        .expect("read directory")
        .map(|entry| entry.expect("directory entry"))
        .filter(|entry| entry.file_type().expect("file type").is_file())
        .map(|entry| {
            (
                entry.file_name().to_string_lossy().into_owned(),
                fs::read(entry.path()).expect("read file"),
            )
        })
        .collect::<Vec<_>>();
    files.sort_by(|left, right| left.0.cmp(&right.0));
    files
}

#[derive(Debug)]
struct SequencedOpenFaults {
    failures: Mutex<VecDeque<DatabaseFailureClass>>,
    attempts: AtomicUsize,
}

impl SequencedOpenFaults {
    fn new(failures: impl IntoIterator<Item = DatabaseFailureClass>) -> Self {
        Self {
            failures: Mutex::new(failures.into_iter().collect()),
            attempts: AtomicUsize::new(0),
        }
    }

    fn attempts(&self) -> usize {
        self.attempts.load(Ordering::SeqCst)
    }
}

impl StoreOpenFaultInjector for SequencedOpenFaults {
    fn before_open_attempt(&self, _attempt: usize) -> Option<DatabaseFailureClass> {
        self.attempts.fetch_add(1, Ordering::SeqCst);
        self.failures
            .lock()
            .expect("fault sequence mutex")
            .pop_front()
    }
}

#[derive(Debug, Default)]
struct RecordingSleeper {
    delays: Mutex<Vec<Duration>>,
}

impl RecordingSleeper {
    fn delays(&self) -> Vec<Duration> {
        self.delays.lock().expect("sleep recorder mutex").clone()
    }
}

impl RetrySleeper for RecordingSleeper {
    fn sleep(&self, delay: Duration) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        self.delays
            .lock()
            .expect("sleep recorder mutex")
            .push(delay);
        Box::pin(std::future::ready(()))
    }
}
