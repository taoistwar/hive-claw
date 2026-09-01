//! T119/T130 root-scoped backup/restore maintenance exclusion contracts.
//!
//! These tests deliberately drive the public Backup/Restore coordinator
//! lifecycles. A maintenance-busy result must be selected before archive or
//! target-path I/O and must never expose roots, paths, or passphrases.

#![allow(missing_docs)]

mod support;

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::Arc,
    time::Duration,
};

use hivegui::datasource::backup::{
    BACKUP_CONFIRMATION_CRASH_POINTS, BackupCoordinator, BackupCrashPoint, BackupExporter,
    ExportError, ImportError, RESTORE_SWITCH_CRASH_POINTS, RestoreCoordinator,
};
use hivegui::datasource::{Store, StoreOpenErrorKind, StoreOpenOptions};
use support::TestWorkspace;

const PASSPHRASE: &str = "T119-T130-root-maintenance-passphrase";
const RESTORE_STORE_LOCK_CHILD_MODE: &str = "HIVEGUI_MAINTENANCE_RESTORE_LOCK_CHILD_MODE";
const RESTORE_STORE_LOCK_CHILD_ROOT: &str = "HIVEGUI_MAINTENANCE_RESTORE_LOCK_CHILD_ROOT";

async fn open_seeded_store(root: &Path, data_source_name: &str) -> Store {
    let store = Store::open_local(StoreOpenOptions::for_root(root))
        .await
        .expect("open owner-aware maintenance Store");
    store
        .create(
            data_source_name,
            "127.0.0.1",
            3306,
            "maintenance-user",
            b"maintenance-password",
        )
        .await
        .expect("seed maintenance Store");
    store
}

async fn export_restore_archive(workspace: &TestWorkspace, label: &str) -> PathBuf {
    let source_root = workspace.root().join(format!("{label}-source"));
    let source = open_seeded_store(&source_root, &format!("{label}-archive-row")).await;
    source.pool().close().await;
    drop(source);

    let archive = workspace.root().join(format!("{label}.age.tar"));
    BackupExporter::new(source_root.join("datasources.db"))
        .export_age(&archive, PASSPHRASE)
        .await
        .expect("export maintenance restore archive");
    archive
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
    Command::new(
        env::current_exe().expect("resolve backup_maintenance integration-test executable"),
    )
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

fn crash_point(points: &[BackupCrashPoint], name: &str) -> BackupCrashPoint {
    points
        .iter()
        .copied()
        .find(|point| point.as_str() == name)
        .unwrap_or_else(|| panic!("missing crash point {name}"))
}

fn assert_import_maintenance_busy(
    error: ImportError,
    expected_active: &'static str,
    forbidden: &[&str],
) {
    let rendered = error.to_string();
    let active = match error {
        ImportError::MaintenanceBusy { active } => active,
        other => panic!("expected maintenance busy, got {other}"),
    };
    assert_eq!(active, expected_active);
    assert_eq!(
        rendered,
        format!("maintenance_busy {{ active: {expected_active} }}")
    );
    for secret in forbidden {
        assert!(
            !rendered.contains(secret),
            "maintenance-busy error leaked forbidden input: {rendered}"
        );
    }
}

fn assert_import_store_owner_missing(error: ImportError, forbidden: &[&str]) {
    let rendered = error.to_string();
    match error {
        ImportError::StorageRecoveryBlocked { reason, artifact } => {
            assert_eq!(reason, "connections_open");
            assert_eq!(artifact, "connection");
        }
        other => panic!("expected fixed Store-owner-missing rejection, got {other}"),
    }
    assert_eq!(
        rendered,
        "storage_recovery_blocked { reason: connections_open, artifact: connection }"
    );
    for secret in forbidden {
        assert!(
            !rendered.contains(secret),
            "Store-owner-missing rejection leaked forbidden input: {rendered}"
        );
    }
}

fn assert_export_maintenance_busy(
    error: ExportError,
    expected_active: &'static str,
    forbidden: &[&str],
) {
    let rendered = error.to_string();
    let active = match error {
        ExportError::MaintenanceBusy { active } => active,
        other => panic!("expected maintenance busy, got {other}"),
    };
    assert_eq!(active, expected_active);
    assert_eq!(
        rendered,
        format!("maintenance_busy {{ active: {expected_active} }}")
    );
    for secret in forbidden {
        assert!(
            !rendered.contains(secret),
            "maintenance-busy error leaked forbidden input: {rendered}"
        );
    }
}

fn restore_registry_entries(root: &Path) -> Vec<PathBuf> {
    let registry = root.join(".hivegui-db-staging-v1");
    match fs::read_dir(registry) {
        Ok(entries) => {
            let mut paths = entries
                .map(|entry| entry.expect("read maintenance registry entry").path())
                .collect::<Vec<_>>();
            paths.sort();
            paths
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => panic!("read maintenance registry: {error}"),
    }
}

async fn assert_write_gate_closed(store: &Store, name: &str) {
    let error = store
        .create(name, "127.0.0.1", 3307, "blocked-user", b"blocked-password")
        .await
        .expect_err("maintenance terminal must keep the write gate closed");
    assert!(error.to_string().contains("write_gate_closed"));
}

fn assert_restore_cancel_interlock_seam_source_contract() {
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

    fn assert_thin_cancel_wrapper(item: &str, label: &str, forwards_barriers: bool) {
        let body_start = item
            .find('{')
            .map(|offset| offset + 1)
            .expect("cancel wrapper body");
        let call_start = item
            .find("self.cancel_preview_internal(")
            .expect("shared cancellation tail call");
        assert!(
            item[body_start..call_start].trim().is_empty(),
            "{label} must do no work before its shared cancellation tail-call"
        );
        assert_eq!(
            item.matches("self.cancel_preview_internal(").count(),
            1,
            "{label} must call the shared private cancellation exactly once: {item}"
        );
        assert_eq!(
            item.matches(".await").count(),
            1,
            "{label} may await only the shared cancellation: {item}"
        );
        for forbidden_control_flow in [" if ", " if let ", " match ", " loop ", " while ", " for "]
        {
            assert!(
                !item.contains(forbidden_control_flow),
                "{label} must not select a test-only retirement path: {forbidden_control_flow} in {item}"
            );
        }
        let await_end = item
            .rfind(".await")
            .map(|offset| offset + ".await".len())
            .expect("single shared cancellation await");
        let tail_call = &item[call_start..await_end];
        assert_eq!(
            tail_call.contains("barriers"),
            forwards_barriers,
            "{label} must forward barriers only through the shared cancellation call: {tail_call}"
        );
        assert_eq!(
            item[await_end..].trim(),
            "}",
            "{label} must return the shared cancellation result without fallback cleanup"
        );
    }

    let source = include_str!("../src/datasource/backup.rs");
    let normalized = source.split_whitespace().collect::<Vec<_>>().join(" ");
    let ordinary_signature = "pub async fn cancel_preview(&self, prepared: &PreparedRestore)";
    let wrapper_signature = "pub async fn cancel_preview_with_retirement_interlock_for_test(";
    let internal_signature = "async fn cancel_preview_internal(";
    let ordinary_start = normalized
        .find(ordinary_signature)
        .expect("production RestoreCoordinator::cancel_preview");
    let wrapper_start = normalized
        .find(wrapper_signature)
        .expect("thin cancellation retirement interlock wrapper");
    let internal_start = normalized
        .find(internal_signature)
        .expect("shared private cancellation implementation");
    assert!(
        ordinary_start < wrapper_start && wrapper_start < internal_start,
        "ordinary cancel, test wrapper, and shared private internal must remain adjacent and ordered"
    );

    let ordinary = item_body(&normalized, ordinary_signature);
    let wrapper = item_body(&normalized, wrapper_signature);
    assert_thin_cancel_wrapper(ordinary, "ordinary cancel_preview", false);
    assert_thin_cancel_wrapper(wrapper, "retirement-interlock test wrapper", true);
    assert!(
        !ordinary.contains("barriers") && wrapper.contains("barriers"),
        "only the thin test wrapper may accept and forward synchronization barriers"
    );

    let internal = item_body(&normalized, internal_signature);
    assert_eq!(
        internal.matches("retire_preview_plan(").count(),
        1,
        "the shared cancellation internal must own the single real retirement call: {internal}"
    );
    assert!(
        internal.contains("retirement_interlock"),
        "the thin wait seam must be threaded into the shared cancellation internal"
    );
}

fn assert_restore_recovery_claim_seam_source_contract() {
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

    fn assert_tail_call(item: &str, label: &str, forwards_barriers: bool) {
        let body_start = item
            .find('{')
            .map(|offset| offset + 1)
            .expect("recovery wrapper body");
        let call_start = item
            .find("self.recover_startup_internal(")
            .expect("shared recovery tail call");
        assert!(
            item[body_start..call_start].trim().is_empty(),
            "{label} must do no work before the shared recovery tail-call"
        );
        assert_eq!(item.matches("self.recover_startup_internal(").count(), 1);
        assert_eq!(item.matches(".await").count(), 1);
        let await_end = item
            .rfind(".await")
            .map(|offset| offset + ".await".len())
            .expect("single shared recovery await");
        assert_eq!(
            item[call_start..await_end].contains("barriers"),
            forwards_barriers
        );
        assert_eq!(item[await_end..].trim(), "}");
        for forbidden in [" if ", " if let ", " match ", " loop ", " while ", " for "] {
            assert!(
                !item.contains(forbidden),
                "{label} must not select a second recovery path: {forbidden}"
            );
        }
    }

    let source = include_str!("../src/datasource/backup.rs");
    let normalized = source.split_whitespace().collect::<Vec<_>>().join(" ");
    let store_source = include_str!("../src/datasource/store.rs");
    let normalized_store = store_source
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let ordinary_signature = "pub async fn recover_startup(&self)";
    let wrapper_signature = "pub async fn recover_startup_with_replay_interlock_for_test(";
    let admission_wrapper_signature =
        "pub async fn recover_startup_with_admission_interlock_for_test(";
    let internal_signature = "async fn recover_startup_internal(";
    let ordinary_start = normalized
        .find(ordinary_signature)
        .expect("ordinary startup recovery");
    let wrapper_start = normalized
        .find(wrapper_signature)
        .expect("recovery interlock wrapper");
    let admission_wrapper_start = normalized
        .find(admission_wrapper_signature)
        .expect("recovery admission interlock wrapper");
    let internal_start = normalized
        .find(internal_signature)
        .expect("shared recovery internal");
    assert!(
        ordinary_start < wrapper_start
            && wrapper_start < admission_wrapper_start
            && admission_wrapper_start < internal_start
    );
    assert_tail_call(
        item_body(&normalized, ordinary_signature),
        "ordinary recovery",
        false,
    );
    assert_tail_call(
        item_body(&normalized, wrapper_signature),
        "recovery interlock wrapper",
        true,
    );
    assert_tail_call(
        item_body(&normalized, admission_wrapper_signature),
        "recovery admission interlock wrapper",
        true,
    );
    assert!(
        item_body(&normalized, ordinary_signature)
            .contains("self.recover_startup_internal(None, None).await")
    );
    assert!(
        item_body(&normalized, wrapper_signature)
            .contains("self.recover_startup_internal(Some(barriers), None).await")
    );
    assert!(
        item_body(&normalized, admission_wrapper_signature)
            .contains("self.recover_startup_internal(None, Some(barriers)).await")
    );

    let coordinator = item_body(&normalized, "pub struct RestoreCoordinator");
    assert!(
        coordinator.contains("restore_recovery_state: Arc<AtomicU8>"),
        "all exact coordinator clones must share one authoritative idle/running/done recovery state: {coordinator}"
    );
    let internal = item_body(&normalized, internal_signature);
    assert_eq!(
        internal.matches("replay_registry_retirements(").count(),
        1,
        "only the shared recovery internal may execute destructive replay: {internal}"
    );
    assert_eq!(
        internal.matches("admission_interlock").count(),
        3,
        "the admission interlock may appear only in its parameter, helper name, and one unconditional thin-wait argument"
    );
    assert_eq!(
        internal.matches("recovery_admission.complete()").count(),
        2,
        "both exact-terminal and cold success must select DONE before the admission guard drops"
    );
    let state_load = internal
        .find("let recovery_state_before_admission = self.restore_recovery_state.load(")
        .expect("recovery must observe state immediately before its test-only admission pause");
    let admission_wait = internal[state_load..]
        .find("wait_restore_recovery_admission_interlock(admission_interlock.as_ref()).await")
        .map(|offset| state_load + offset)
        .expect("thin admission interlock must separate the stale load from authoritative CAS");
    let admission_cas = internal[admission_wait..]
        .find("self.restore_recovery_state.compare_exchange(")
        .map(|offset| admission_wait + offset)
        .expect("shared idle/running/done state must authoritatively admit replay");
    assert_eq!(
        internal
            .matches("restore_recovery_state.compare_exchange(")
            .count(),
        1,
        "the shared internal must have exactly one authoritative admission CAS"
    );
    let admission_match = internal[admission_cas..]
        .find("match admission")
        .map(|offset| admission_cas + offset)
        .expect("every CAS outcome must be handled before Store-root inspection");
    let running_rejection = internal[admission_match..]
        .find("RESTORE_RECOVERY_RUNNING")
        .map(|offset| admission_match + offset)
        .expect("a running exact clone must fail fast before replay");
    let done_rejection = internal[running_rejection..]
        .find("RESTORE_RECOVERY_DONE")
        .map(|offset| running_rejection + offset)
        .expect("a late-resuming completed clone must fail closed before replay");
    let completed_rejection = internal[done_rejection..]
        .find(
            "ImportError::StorageRecoveryBlocked { reason: \"connections_open\", artifact: \"connection\"",
        )
        .map(|offset| done_rejection + offset)
        .expect("completed exact coordinator must use the fixed owner-missing envelope");
    let admission_guard = internal[completed_rejection..]
        .find("RestoreRecoveryAdmission::new(self.restore_recovery_state.clone())")
        .map(|offset| completed_rejection + offset)
        .expect("a successful CAS must install an error-reset admission guard");
    let maintenance_lease_lookup = internal[admission_guard..]
        .find("self.restore_maintenance_lease()")
        .map(|offset| admission_guard + offset)
        .expect("maintenance lease inspection must follow authoritative admission");
    let snapshot = internal[maintenance_lease_lookup..]
        .find("store_maintenance_snapshot(")
        .map(|offset| maintenance_lease_lookup + offset)
        .expect("claim owner must inspect the exact root maintenance owner");
    assert!(
        state_load < admission_wait
            && admission_wait < admission_cas
            && admission_cas < admission_match
            && admission_match < running_rejection
            && running_rejection < done_rejection
            && done_rejection < completed_rejection
            && completed_rejection < admission_guard
            && admission_guard < maintenance_lease_lookup
            && maintenance_lease_lookup < snapshot
    );
    let maintenance_tuple = internal[snapshot..]
        .find("match (maintenance_lease.as_ref(), maintenance_snapshot)")
        .map(|offset| snapshot + offset)
        .expect("local lease and root registry snapshot must be matched in that exact order");
    let missing_registry = internal[maintenance_tuple..]
        .find("(Some(_), None) => return Err(")
        .map(|offset| maintenance_tuple + offset)
        .expect(
            "a local maintenance lease without its root registry owner must fail closed explicitly",
        );
    let terminal = internal[snapshot..]
        .find("StoreMaintenancePhase::RestoreTerminal")
        .map(|offset| snapshot + offset)
        .expect("claim owner must require the exact terminal phase");
    let claim = internal[terminal..]
        .find("maintenance_lease.claim_terminal_recovery()")
        .map(|offset| terminal + offset)
        .expect("root registry must atomically admit only the exact terminal recovery owner");
    let wait = internal[claim..]
        .find("wait_restore_recovery_replay_interlock(replay_interlock.as_ref()).await")
        .map(|offset| claim + offset)
        .expect("thin replay interlock after exact terminal ownership check");
    let replay = internal[wait..]
        .find("replay_registry_retirements(")
        .map(|offset| wait + offset)
        .expect("ordinary destructive replay after the thin wait");
    assert!(
        completed_rejection < snapshot
            && snapshot < maintenance_tuple
            && maintenance_tuple < missing_registry
            && missing_registry < wait
            && snapshot < terminal
            && terminal < claim
            && claim < wait
    );
    assert!(wait < replay);

    let preview_internal = item_body(&normalized, "async fn preview_restore_internal(");
    let exact_store_owner = preview_internal
        .find("if !self.store.as_ref().is_some_and(Store::has_restore_owner)")
        .expect("stale recovered coordinators must reject a missing exact Store owner");
    let owner_missing_error = preview_internal[exact_store_owner..]
        .find("StorageRecoveryBlocked { reason: \"connections_open\", artifact: \"connection\"")
        .map(|offset| exact_store_owner + offset)
        .expect("missing exact Store owner must use the fixed fail-closed envelope");
    let maintenance_acquire = preview_internal[owner_missing_error..]
        .find("self.acquire_restore_maintenance()")
        .map(|offset| owner_missing_error + offset)
        .expect("ordinary restore maintenance acquisition");
    let archive_open = preview_internal[maintenance_acquire..]
        .find("VerifiedArchiveBinding::open(archive)")
        .map(|offset| maintenance_acquire + offset)
        .expect("ordinary bound archive open");
    assert!(
        exact_store_owner < owner_missing_error
            && owner_missing_error < maintenance_acquire
            && maintenance_acquire < archive_open,
        "owner loss must reject before maintenance, archive, or staging I/O"
    );

    let release = internal[replay..]
        .find(".release_with_finalize( StoreMaintenancePhase::RestoreTerminal,")
        .or_else(|| {
            internal[replay..]
                .find(".release_with_finalize(StoreMaintenancePhase::RestoreTerminal,")
        })
        .map(|offset| replay + offset)
        .expect("typed exact-terminal release/finalize after durable replay");
    let local_slot_lock = internal[..release]
        .rfind("let mut maintenance_slot = self.maintenance_lease.lock()")
        .expect("all fallible local-slot locking must precede terminal finalization");
    let exact_local_slot = internal[local_slot_lock..release]
        .find(
            "if !maintenance_slot.as_ref().is_some_and(|current| Arc::ptr_eq(current, maintenance_lease))",
        )
        .or_else(|| {
            internal[local_slot_lock..release].find(
                "if !maintenance_slot .as_ref() .is_some_and(|current| Arc::ptr_eq(current, maintenance_lease))",
            )
        })
        .map(|offset| local_slot_lock + offset)
        .expect("terminal finalization must gate on the exact local lease Arc");
    let local_slot_rejection = internal[exact_local_slot..release]
        .find("return Err(ImportError::Io(\"maintenance registry unavailable\".into()))")
        .map(|offset| exact_local_slot + offset)
        .expect("an inexact local lease must fail closed before terminal finalization");
    let gate_open = internal[release..]
        .find("frozen.remove(")
        .map(|offset| release + offset)
        .expect("typed finalization owns the write-gate transition");
    let os_owner_release = internal[gate_open..]
        .find("owner_slot.take()")
        .map(|offset| gate_open + offset)
        .expect("typed finalization owns the OS-owner transition");
    let terminal_clear = internal[os_owner_release..]
        .find("restore_owner_terminal.store(false")
        .map(|offset| os_owner_release + offset)
        .expect("typed finalization clears terminal after gate/OS owner transition");
    let completed_store = internal[terminal_clear..]
        .find("recovery_admission.complete()")
        .map(|offset| terminal_clear + offset)
        .expect("typed finalization records DONE without another fallible step");
    let local_slot_take = internal[completed_store..]
        .find("maintenance_slot.take()")
        .map(|offset| completed_store + offset)
        .expect("typed finalization consumes the exact local maintenance slot");
    assert!(
        local_slot_lock < exact_local_slot
            && exact_local_slot < local_slot_rejection
            && local_slot_rejection < release
            && release < gate_open
            && gate_open < os_owner_release
            && os_owner_release < terminal_clear
            && terminal_clear < completed_store
            && completed_store < local_slot_take
    );
    let release_tail = &internal[release..];
    assert!(
        release_tail.matches('?').count() <= 1,
        "all fallible guards must be acquired before exact terminal finalization: {release_tail}"
    );
    for forbidden in [".lock()", ".map_err(", "return Err(", ".await"] {
        assert!(
            !release_tail.contains(forbidden),
            "no fallible or asynchronous work may follow exact terminal release: {forbidden}"
        );
    }

    let claim_terminal = item_body(&normalized_store, "fn claim_terminal_recovery(");
    let claim_registry = claim_terminal
        .find("let mut owners = registry")
        .expect("terminal claim must hold the root maintenance registry");
    let claim_key = claim_terminal[claim_registry..]
        .find("owners.get_mut(&self.registry_key)")
        .map(|offset| claim_registry + offset)
        .expect("terminal claim must look up the exact canonical root key");
    let claim_owner_id = claim_terminal[claim_key..]
        .find("owner.owner_id == self.owner_id")
        .map(|offset| claim_key + offset)
        .expect("terminal claim must compare exact owner id");
    let claim_kind = claim_terminal[claim_owner_id..]
        .find("owner.kind == self.kind")
        .map(|offset| claim_owner_id + offset)
        .expect("terminal claim must compare exact maintenance kind");
    let claim_phase = claim_terminal[claim_kind..]
        .find("owner.phase == StoreMaintenancePhase::RestoreTerminal")
        .map(|offset| claim_kind + offset)
        .expect("terminal claim must require RestoreTerminal");
    let claim_unclaimed = claim_terminal[claim_phase..]
        .find("!owner.recovery_claimed")
        .map(|offset| claim_phase + offset)
        .expect("terminal recovery claim must be one-shot");
    let mark_claimed = claim_terminal[claim_unclaimed..]
        .find("owner.recovery_claimed = true")
        .map(|offset| claim_unclaimed + offset)
        .expect("admitted terminal recovery must atomically mark the root owner claimed");
    assert!(
        claim_registry < claim_key
            && claim_key < claim_owner_id
            && claim_owner_id < claim_kind
            && claim_kind < claim_phase
            && claim_phase < claim_unclaimed
            && claim_unclaimed < mark_claimed
    );

    let release_with_finalize = item_body(&normalized_store, "fn release_with_finalize");
    assert!(
        release_with_finalize.contains("F: FnOnce()")
            && !release_with_finalize.contains("FnOnce() ->"),
        "terminal finalization closure must be infallible"
    );
    let registry_lock = release_with_finalize
        .find("let mut owners = registry")
        .expect("typed finalization must hold the root maintenance registry");
    let release_key = release_with_finalize[registry_lock..]
        .find("owners.get(&self.registry_key)")
        .map(|offset| registry_lock + offset)
        .expect("typed finalization must look up the exact canonical root key");
    let exact_owner = release_with_finalize[release_key..]
        .find("owner.owner_id == self.owner_id")
        .map(|offset| release_key + offset)
        .expect("typed finalization must compare exact owner id");
    let exact_kind = release_with_finalize[exact_owner..]
        .find("owner.kind == self.kind")
        .map(|offset| exact_owner + offset)
        .expect("typed finalization must compare exact maintenance kind");
    let exact_phase = release_with_finalize[exact_kind..]
        .find("owner.phase == expected")
        .map(|offset| exact_kind + offset)
        .expect("typed finalization must compare the caller's exact terminal phase");
    let exact_claim = release_with_finalize[exact_phase..]
        .find("owner.recovery_claimed")
        .map(|offset| exact_phase + offset)
        .expect("typed finalization must require the admitted recovery claim");
    let finalize = release_with_finalize[exact_claim..]
        .find("finalize()")
        .map(|offset| exact_claim + offset)
        .expect("typed finalization executes only after exact tuple validation");
    let remove = release_with_finalize[finalize..]
        .find("owners.remove(&self.registry_key)")
        .map(|offset| finalize + offset)
        .expect("maintenance entry is removed last while the root registry stays locked");
    assert_eq!(release_with_finalize.matches("finalize()").count(), 1);
    assert_eq!(
        release_with_finalize
            .matches("owners.remove(&self.registry_key)")
            .count(),
        1
    );
    assert!(
        registry_lock < release_key
            && release_key < exact_owner
            && exact_owner < exact_kind
            && exact_kind < exact_phase
            && exact_phase < exact_claim
            && exact_claim < finalize
            && finalize < remove
    );
    let after_finalize = &release_with_finalize[finalize..];
    assert_eq!(
        after_finalize.matches('?').count(),
        0,
        "no fallible work may run after in-memory finalization"
    );
    for forbidden in [".lock()", ".map_err(", "return Err("] {
        assert!(
            !after_finalize.contains(forbidden),
            "no new failure path may open after finalization: {forbidden}"
        );
    }

    let admission_type = item_body(&normalized, "struct RestoreRecoveryAdmission");
    assert!(
        admission_type.contains("state: Arc<AtomicU8>"),
        "the admission guard must own the exact shared recovery state"
    );
    let admission_complete = item_body(&normalized, "fn complete(&mut self)");
    assert!(
        admission_complete.contains("self.state.store(RESTORE_RECOVERY_DONE, Ordering::SeqCst)"),
        "successful finalization must durably select DONE before the guard drops"
    );
    let admission_drop = item_body(&normalized, "impl Drop for RestoreRecoveryAdmission");
    assert_eq!(admission_drop.matches("compare_exchange(").count(), 1);
    let reset_running = admission_drop
        .find("RESTORE_RECOVERY_RUNNING")
        .expect("failed recovery guard Drop must compare the RUNNING state");
    let reset_idle = admission_drop[reset_running..]
        .find("RESTORE_RECOVERY_IDLE")
        .map(|offset| reset_running + offset)
        .expect("failed recovery guard Drop must reset only RUNNING to IDLE");
    assert!(reset_running < reset_idle);

    let wait_helper = item_body(
        &normalized,
        "async fn wait_restore_recovery_replay_interlock(",
    );
    assert!(
        wait_helper.contains("recovery_barriers[0].wait().await")
            && wait_helper.contains("recovery_barriers[1].wait().await")
            && wait_helper.matches(".wait().await").count() == 2
    );
    for forbidden in [
        "Path",
        "File",
        "ImportError",
        "Result<",
        "fs::",
        "open(",
        "read(",
    ] {
        assert!(
            !wait_helper.contains(forbidden),
            "recovery interlock must be synchronization-only: {forbidden}"
        );
    }

    let admission_wait_helper = item_body(
        &normalized,
        "async fn wait_restore_recovery_admission_interlock(",
    );
    assert!(
        admission_wait_helper.contains("recovery_barriers[0].wait().await")
            && admission_wait_helper.contains("recovery_barriers[1].wait().await")
            && admission_wait_helper.matches(".wait().await").count() == 2
    );
    for forbidden in [
        "Path",
        "File",
        "ImportError",
        "Result<",
        "fs::",
        "open(",
        "read(",
    ] {
        assert!(
            !admission_wait_helper.contains(forbidden),
            "recovery admission interlock must be synchronization-only: {forbidden}"
        );
    }
}

fn assert_restore_preview_owner_revalidation_source_contract() {
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

    fn assert_tail_call(wrapper: &str, label: &str) {
        let body_start = wrapper
            .find('{')
            .map(|offset| offset + 1)
            .expect("preview wrapper body");
        let call_start = wrapper
            .find("self.preview_restore_internal(")
            .expect("shared preview tail-call");
        assert!(
            wrapper[body_start..call_start].trim().is_empty(),
            "{label} must do no work before the shared preview tail-call"
        );
        assert_eq!(wrapper.matches("self.preview_restore_internal(").count(), 1);
        assert_eq!(wrapper.matches(".await").count(), 1);
        let await_end = wrapper
            .rfind(".await")
            .map(|offset| offset + ".await".len())
            .expect("single shared preview await");
        assert_eq!(
            wrapper[await_end..].trim(),
            "}",
            "{label} must return the shared preview result without fallback work"
        );
    }

    let source = include_str!("../src/datasource/backup.rs");
    let normalized = source.split_whitespace().collect::<Vec<_>>().join(" ");
    let ordinary_signature =
        "pub async fn preview_restore( &self, archive: &Path, passphrase: &str,";
    let archive_wrapper_signature =
        "pub async fn preview_restore_with_archive_read_interlock_for_test(";
    let owner_wrapper_signature =
        "pub async fn preview_restore_with_owner_admission_interlock_for_test(";
    let internal_signature = "async fn preview_restore_internal(";
    let ordinary_start = normalized
        .find(ordinary_signature)
        .expect("ordinary restore preview");
    let archive_wrapper_start = normalized
        .find(archive_wrapper_signature)
        .expect("archive descriptor interlock wrapper");
    let owner_wrapper_start = normalized
        .find(owner_wrapper_signature)
        .expect("owner admission interlock wrapper");
    let internal_start = normalized
        .find(internal_signature)
        .expect("shared restore preview internal");
    assert!(
        ordinary_start < archive_wrapper_start
            && archive_wrapper_start < owner_wrapper_start
            && owner_wrapper_start < internal_start
    );

    let ordinary = item_body(&normalized, ordinary_signature);
    let archive_wrapper = item_body(&normalized, archive_wrapper_signature);
    let owner_wrapper = item_body(&normalized, owner_wrapper_signature);
    for (wrapper, exact_call) in [
        (
            ordinary,
            "self.preview_restore_internal(archive, passphrase, None, None)",
        ),
        (
            archive_wrapper,
            "self.preview_restore_internal(archive, passphrase, Some(barriers), None)",
        ),
        (
            owner_wrapper,
            "self.preview_restore_internal(archive, passphrase, None, Some(barriers))",
        ),
    ] {
        assert!(
            wrapper.contains(&format!("{exact_call}.await"))
                || wrapper.contains(&format!("{exact_call} .await")),
            "preview wrapper must preserve the exact approved argument sequence"
        );
    }
    for (wrapper, label) in [
        (ordinary, "ordinary preview"),
        (archive_wrapper, "archive interlock preview"),
        (owner_wrapper, "owner interlock preview"),
    ] {
        assert_tail_call(wrapper, label);
        for forbidden in [" if ", " if let ", " match ", " loop ", " while ", " for "] {
            assert!(
                !wrapper.contains(forbidden),
                "{label} must not select a separate preview path: {forbidden}"
            );
        }
    }

    let internal = item_body(&normalized, internal_signature);
    assert_eq!(
        internal.matches("owner_interlock").count(),
        3,
        "the owner interlock may appear only in its parameter, helper name, and thin-wait argument"
    );
    let first_owner_check = internal
        .find("if !self.store.as_ref().is_some_and(Store::has_restore_owner)")
        .expect("initial exact Store-owner check");
    let first_fixed_error = internal[first_owner_check..]
        .find(
            "ImportError::StorageRecoveryBlocked { reason: \"connections_open\", artifact: \"connection\"",
        )
        .map(|offset| first_owner_check + offset)
        .expect("initial owner loss must use the fixed pre-maintenance envelope");
    let owner_wait = internal[first_owner_check..]
        .find("wait_restore_preview_owner_interlock(owner_interlock.as_ref()).await")
        .map(|offset| first_owner_check + offset)
        .expect("thin pause after the initial owner observation");
    let maintenance_acquire = internal[owner_wait..]
        .find("let maintenance_lease = self.acquire_restore_maintenance()?")
        .map(|offset| owner_wait + offset)
        .expect("ordinary root maintenance acquire");
    let second_owner_check = internal[maintenance_acquire..]
        .find("if !self.store.as_ref().is_some_and(Store::has_restore_owner)")
        .map(|offset| maintenance_acquire + offset)
        .expect("owner must be revalidated while the new maintenance lease is held");
    let exact_release = internal[second_owner_check..]
        .find("maintenance_lease.release(StoreMaintenancePhase::RestoreReady)")
        .map(|offset| second_owner_check + offset)
        .expect("owner-loss branch must release the exact newly acquired Ready lease");
    let exact_clear = internal[exact_release..]
        .find("self.clear_restore_maintenance_if_exact(&maintenance_lease)")
        .map(|offset| exact_release + offset)
        .expect("owner-loss branch must clear only its exact local lease Arc");
    let fixed_error = internal[exact_clear..]
        .find(
            "ImportError::StorageRecoveryBlocked { reason: \"connections_open\", artifact: \"connection\"",
        )
        .map(|offset| exact_clear + offset)
        .expect("owner loss must use the fixed fail-closed envelope");
    let archive_open = internal[fixed_error..]
        .find("VerifiedArchiveBinding::open(archive)")
        .map(|offset| fixed_error + offset)
        .expect("ordinary archive descriptor open");
    assert!(
        first_owner_check < first_fixed_error
            && first_fixed_error < owner_wait
            && owner_wait < maintenance_acquire
            && maintenance_acquire < second_owner_check
            && second_owner_check < exact_release
            && exact_release < exact_clear
            && exact_clear < fixed_error
            && fixed_error < archive_open
    );

    let wait_helper = item_body(
        &normalized,
        "async fn wait_restore_preview_owner_interlock(",
    );
    assert!(
        wait_helper.contains("owner_barriers[0].wait().await")
            && wait_helper.contains("owner_barriers[1].wait().await")
            && wait_helper.matches(".wait().await").count() == 2
    );
    for forbidden in [
        "Path",
        "File",
        "ImportError",
        "Result<",
        "fs::",
        "open(",
        "read(",
    ] {
        assert!(
            !wait_helper.contains(forbidden),
            "preview owner interlock must be synchronization-only: {forbidden}"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn ready_maintenance_is_exclusive_per_root_and_busy_precedes_archive_io() {
    const MISSING_PASSPHRASE: &str = "must-not-leak-missing-archive-passphrase";

    let workspace = TestWorkspace::new().expect("maintenance workspace");
    let archive = export_restore_archive(&workspace, "ready-root-scope").await;
    let root_a = workspace.root().join("ready-root-a");
    let root_b = workspace.root().join("ready-root-b");
    let store_a = open_seeded_store(&root_a, "ready-current-a").await;
    let store_b = open_seeded_store(&root_b, "ready-current-b").await;
    let backup_a = BackupCoordinator::from_store(store_a.clone()).expect("bind backup root A");
    let restore_a = RestoreCoordinator::from_store(store_a.clone()).expect("bind restore root A");
    let restore_b = RestoreCoordinator::from_store(store_b.clone()).expect("bind restore root B");

    let backup_target_a = workspace.root().join("ready-backup-a.age.tar");
    let backup_ready = backup_a
        .preview_export(&backup_target_a)
        .await
        .expect("root A Backup reaches Ready");
    let missing_archive = workspace
        .root()
        .join("must-not-open-missing-archive-sensitive-name.age.tar");
    assert!(!missing_archive.exists());
    let same_root_restore = restore_a
        .preview_restore(&missing_archive, MISSING_PASSPHRASE)
        .await
        .expect_err("root A Backup Ready must block root A Restore before archive I/O");
    assert_import_maintenance_busy(
        same_root_restore,
        "backup_ready",
        &[
            missing_archive.to_string_lossy().as_ref(),
            MISSING_PASSPHRASE,
        ],
    );
    assert!(restore_registry_entries(&root_a).is_empty());

    let different_root_restore = restore_b
        .preview_restore(&archive, PASSPHRASE)
        .await
        .expect("root B Restore may run while root A Backup is Ready");
    let root_a_during_root_b = restore_a
        .preview_restore(&missing_archive, MISSING_PASSPHRASE)
        .await
        .expect_err("root B acquisition must not clear root A Backup Ready maintenance");
    assert_import_maintenance_busy(
        root_a_during_root_b,
        "backup_ready",
        &[
            missing_archive.to_string_lossy().as_ref(),
            MISSING_PASSPHRASE,
        ],
    );
    restore_b
        .cancel_preview(&different_root_restore)
        .await
        .expect("retire independent root B Restore preview");
    assert!(restore_registry_entries(&root_b).is_empty());
    let root_a_after_root_b = restore_a
        .preview_restore(&missing_archive, MISSING_PASSPHRASE)
        .await
        .expect_err("root B retirement must not clear root A Backup Ready maintenance");
    assert_import_maintenance_busy(
        root_a_after_root_b,
        "backup_ready",
        &[
            missing_archive.to_string_lossy().as_ref(),
            MISSING_PASSPHRASE,
        ],
    );

    backup_a
        .cancel_preview(&backup_ready)
        .await
        .expect("release root A Backup Ready lease");
    let restore_ready = restore_a
        .preview_restore(&archive, PASSPHRASE)
        .await
        .expect("root A Restore acquires maintenance after Backup release");
    let blocked_backup_target = workspace
        .root()
        .join("must-not-leak-blocked-backup-target.age.tar");
    let same_root_backup = backup_a
        .preview_export(&blocked_backup_target)
        .await
        .expect_err("root A Restore Ready must block root A Backup");
    assert_export_maintenance_busy(
        same_root_backup,
        "restore_ready",
        &[blocked_backup_target.to_string_lossy().as_ref(), PASSPHRASE],
    );
    assert!(!blocked_backup_target.exists());

    restore_a
        .cancel_preview(&restore_ready)
        .await
        .expect("retire root A Restore preview");
    let final_backup = backup_a
        .preview_export(&blocked_backup_target)
        .await
        .expect("root A Backup reacquires maintenance after Restore retirement");
    backup_a
        .cancel_preview(&final_backup)
        .await
        .expect("release final root A Backup preview");
    assert!(restore_registry_entries(&root_a).is_empty());
    assert!(!backup_target_a.exists());
    assert!(!blocked_backup_target.exists());

    store_a.pool().close().await;
    store_b.pool().close().await;
}

#[tokio::test(flavor = "current_thread")]
async fn restore_cancellation_holds_maintenance_until_retirement_and_is_aba_safe() {
    assert_restore_cancel_interlock_seam_source_contract();

    let workspace = TestWorkspace::new().expect("cancellation maintenance workspace");
    let archive = export_restore_archive(&workspace, "cancelling-aba").await;
    let root = workspace.root().join("cancelling-aba-root");
    let store = open_seeded_store(&root, "cancelling-aba-current").await;
    let restore = RestoreCoordinator::from_store(store.clone()).expect("bind restore coordinator");
    let backup = BackupCoordinator::from_store(store.clone()).expect("bind backup coordinator");

    let prepared_p1 = Arc::new(
        restore
            .preview_restore(&archive, PASSPHRASE)
            .await
            .expect("P1 Restore reaches Ready"),
    );
    let p1_operation = prepared_p1.db_instance_operation_id().to_owned();
    let barriers: [Arc<tokio::sync::Barrier>; 2] =
        std::array::from_fn(|_| Arc::new(tokio::sync::Barrier::new(2)));
    let cancel_restore = restore.clone();
    let cancel_prepared = prepared_p1.clone();
    let cancel_barriers = barriers.clone();
    let cancel_task = tokio::spawn(async move {
        cancel_restore
            .cancel_preview_with_retirement_interlock_for_test(
                cancel_prepared.as_ref(),
                cancel_barriers,
            )
            .await
    });

    tokio::time::timeout(Duration::from_secs(10), barriers[0].wait())
        .await
        .expect("P1 reaches Cancelling before retirement");
    let blocked_target = workspace.root().join("cancelling-busy.age.tar");
    let cancelling_busy = backup
        .preview_export(&blocked_target)
        .await
        .expect_err("same-root Backup must remain busy while P1 retirement is paused");
    assert_export_maintenance_busy(
        cancelling_busy,
        "restore_cancelling",
        &[blocked_target.to_string_lossy().as_ref(), PASSPHRASE],
    );
    assert_eq!(restore_registry_entries(&root).len(), 1);

    barriers[1].wait().await;
    tokio::time::timeout(Duration::from_secs(20), cancel_task)
        .await
        .expect("P1 cancellation completes after retirement release")
        .expect("P1 cancellation task does not panic")
        .expect("P1 retirement succeeds");
    assert!(restore_registry_entries(&root).is_empty());

    let prepared_p2 = restore
        .preview_restore(&archive, PASSPHRASE)
        .await
        .expect("P2 acquires maintenance after P1 retirement");
    assert_ne!(prepared_p2.db_instance_operation_id(), p1_operation);
    drop(prepared_p1);

    let p2_still_owns = backup
        .preview_export(&blocked_target)
        .await
        .expect_err("late P1 Arc Drop must not compare-remove P2 maintenance ownership");
    assert_export_maintenance_busy(
        p2_still_owns,
        "restore_ready",
        &[blocked_target.to_string_lossy().as_ref()],
    );
    assert_eq!(restore_registry_entries(&root).len(), 1);

    restore
        .cancel_preview(&prepared_p2)
        .await
        .expect("P2 cancellation retires its exact instance");
    assert!(restore_registry_entries(&root).is_empty());
    let backup_after_p2 = backup
        .preview_export(&blocked_target)
        .await
        .expect("Backup acquires maintenance only after P2 cancellation");
    backup
        .cancel_preview(&backup_after_p2)
        .await
        .expect("release post-P2 Backup preview");
    assert!(!blocked_target.exists());

    store.pool().close().await;
}

#[tokio::test(flavor = "current_thread")]
async fn only_the_exact_restore_owner_can_recover_and_release_confirmation_terminal() {
    let workspace = TestWorkspace::new().expect("restore owner maintenance workspace");
    let archive = export_restore_archive(&workspace, "exact-owner-recovery").await;
    let root = workspace.root().join("exact-owner-recovery-root");
    let store = open_seeded_store(&root, "exact-owner-current").await;
    let stale = store.clone();
    let restore_a = RestoreCoordinator::from_store(store.clone()).expect("bind exact owner A");
    let restore_b = RestoreCoordinator::from_store(store.clone()).expect("precreate contender B");
    let backup_b = BackupCoordinator::from_store(store.clone()).expect("precreate Backup B");

    let prepared_a = restore_a
        .preview_restore(&archive, PASSPHRASE)
        .await
        .expect("owner A Restore reaches Ready");
    let confirmation = restore_a
        .begin_confirmation(&prepared_a)
        .expect("owner A synchronously enters confirmation");
    assert_write_gate_closed(&stale, "must-not-write-during-confirmation").await;

    let contender_recovery = restore_b
        .recover_startup()
        .await
        .expect_err("precreated same-root B must not recover A confirmation");
    assert_import_maintenance_busy(
        contender_recovery,
        "restore_confirmation",
        &[root.to_string_lossy().as_ref(), PASSPHRASE],
    );
    let blocked_backup_target = workspace.root().join("confirmation-busy.age.tar");
    let contender_backup = backup_b
        .preview_export(&blocked_backup_target)
        .await
        .expect_err("same-root Backup must not enter during A confirmation");
    assert_export_maintenance_busy(
        contender_backup,
        "restore_confirmation",
        &[blocked_backup_target.to_string_lossy().as_ref()],
    );
    assert_write_gate_closed(&stale, "must-remain-closed-after-contender-calls").await;

    let plugin_tree_switch = crash_point(&RESTORE_SWITCH_CRASH_POINTS, "plugin_tree_switch");
    let interrupted = confirmation
        .finish_with_crash(plugin_tree_switch)
        .await
        .expect_err("interrupt exact A after database and Plugin switch");
    assert!(interrupted.reached_requested_boundary());

    let terminal_recovery = restore_b
        .recover_startup()
        .await
        .expect_err("precreated B must not recover A terminal");
    assert_import_maintenance_busy(
        terminal_recovery,
        "restore_terminal",
        &[root.to_string_lossy().as_ref()],
    );
    let terminal_backup = backup_b
        .preview_export(&blocked_backup_target)
        .await
        .expect_err("same-root Backup must remain blocked throughout A terminal");
    assert_export_maintenance_busy(
        terminal_backup,
        "restore_terminal",
        &[blocked_backup_target.to_string_lossy().as_ref()],
    );
    assert_write_gate_closed(&stale, "must-remain-closed-through-terminal").await;

    let recovered = restore_a
        .recover_startup()
        .await
        .expect("only exact owner A recovery releases maintenance");
    assert!(recovered.store_may_open());
    assert!(recovered.write_gate_is_open());

    let reopened = Store::open_local(StoreOpenOptions::for_root(&root))
        .await
        .expect("Store reopens immediately after exact A recovery while old handles stay alive");
    reopened
        .create(
            "write-after-exact-owner-recovery",
            "127.0.0.1",
            3308,
            "recovered-user",
            b"recovered-password",
        )
        .await
        .expect("exact A recovery reopens the write gate");
    let backup_after_recovery =
        BackupCoordinator::from_store(reopened.clone()).expect("bind post-recovery Backup");
    let released_preview = backup_after_recovery
        .preview_export(&blocked_backup_target)
        .await
        .expect("maintenance is released only after exact A recovery");

    drop(prepared_a);
    drop(restore_a);
    drop(restore_b);
    drop(backup_b);
    drop(stale);
    drop(store);
    let late_drop_restore =
        RestoreCoordinator::from_store(reopened.clone()).expect("bind late-drop contender");
    let missing_after_recovery = workspace
        .root()
        .join("must-not-open-after-old-owner-drop.age.tar");
    let still_owned_by_new_backup = late_drop_restore
        .preview_restore(&missing_after_recovery, PASSPHRASE)
        .await
        .expect_err("late old-owner Drop must not clear the new Backup maintenance lease");
    assert_import_maintenance_busy(
        still_owned_by_new_backup,
        "backup_ready",
        &[
            missing_after_recovery.to_string_lossy().as_ref(),
            PASSPHRASE,
        ],
    );
    assert!(!missing_after_recovery.exists());

    backup_after_recovery
        .cancel_preview(&released_preview)
        .await
        .expect("release post-recovery Backup preview");
    assert!(!blocked_backup_target.exists());
    reopened.pool().close().await;
}

#[tokio::test(flavor = "current_thread")]
async fn concurrent_exact_restore_recovery_has_one_replay_owner_and_no_open_error_window() {
    assert_restore_recovery_claim_seam_source_contract();

    let workspace = TestWorkspace::new().expect("concurrent recovery workspace");
    let archive = export_restore_archive(&workspace, "concurrent-exact-recovery").await;
    let root = workspace.root().join("concurrent-exact-recovery-root");
    let store = open_seeded_store(&root, "concurrent-recovery-current").await;
    let stale = store.clone();
    let coordinator =
        RestoreCoordinator::from_store(store).expect("bind exact concurrent recovery owner");
    let prepared = coordinator
        .preview_restore(&archive, PASSPHRASE)
        .await
        .expect("prepare concurrent recovery replacement");
    let operation_id = prepared.db_instance_operation_id().to_owned();
    let confirmation = coordinator
        .begin_confirmation(&prepared)
        .expect("enter exact concurrent recovery confirmation");
    let plugin_tree_switch = crash_point(&RESTORE_SWITCH_CRASH_POINTS, "plugin_tree_switch");
    let interrupted = confirmation
        .finish_with_crash(plugin_tree_switch)
        .await
        .expect_err("interrupt after the owner-protected Plugin switch");
    assert!(interrupted.reached_requested_boundary());

    let live = root
        .join(".hivegui-db-staging-v1")
        .join(format!("restore-{operation_id}"));
    let owner_path = live.join(".hivegui-db-recovery-v1.json");
    let owner_before = fs::read(&owner_path).expect("record terminal owner bytes");
    let current_before = fs::read(root.join("datasources.db")).expect("record terminal current");
    let registry_before = restore_registry_entries(&root);
    assert_write_gate_closed(&stale, "concurrent-recovery-before-claim").await;
    assert_store_bound_restore_lock_child(
        &run_store_bound_restore_lock_child(&root, "expect-locked"),
        "terminal owner must retain the OS lock before the recovery claim",
    );

    let barriers: [Arc<tokio::sync::Barrier>; 2] =
        std::array::from_fn(|_| Arc::new(tokio::sync::Barrier::new(2)));
    let first_coordinator = coordinator.clone();
    let first_barriers = barriers.clone();
    let first_recovery = tokio::spawn(async move {
        first_coordinator
            .recover_startup_with_replay_interlock_for_test(first_barriers)
            .await
    });
    tokio::time::timeout(Duration::from_secs(10), barriers[0].wait())
        .await
        .expect("first exact clone owns the root recovery claim before replay");

    let second_outcome =
        tokio::time::timeout(Duration::from_secs(2), coordinator.recover_startup())
            .await
            .expect("second exact clone must fail fast instead of waiting for replay")
            .expect_err("only one exact clone may enter destructive replay");
    assert_import_maintenance_busy(
        second_outcome,
        "restore_terminal",
        &[root.to_string_lossy().as_ref(), PASSPHRASE],
    );
    assert_eq!(restore_registry_entries(&root), registry_before);
    assert_eq!(
        fs::read(&owner_path).expect("reread untouched terminal owner"),
        owner_before
    );
    assert_eq!(
        fs::read(root.join("datasources.db")).expect("reread untouched terminal current"),
        current_before
    );
    assert_write_gate_closed(&stale, "concurrent-recovery-loser").await;
    assert_store_bound_restore_lock_child(
        &run_store_bound_restore_lock_child(&root, "expect-locked"),
        "losing exact clone must not release the terminal OS lock",
    );

    barriers[1].wait().await;
    let recovered = tokio::time::timeout(Duration::from_secs(20), first_recovery)
        .await
        .expect("claimed recovery finishes after replay release")
        .expect("claimed recovery task does not panic")
        .expect("claimed exact recovery succeeds");
    assert!(recovered.store_may_open());
    assert!(recovered.write_gate_is_open());
    assert!(restore_registry_entries(&root).is_empty());
    assert_store_bound_restore_lock_child(
        &run_store_bound_restore_lock_child(&root, "expect-open"),
        "only successful exact replay may release the terminal OS lock",
    );

    let current_after_recovery =
        fs::read(root.join("datasources.db")).expect("record recovered current bytes");
    let registry_after_recovery = restore_registry_entries(&root);
    assert!(registry_after_recovery.is_empty());
    let registry_directory = root.join(".hivegui-db-staging-v1");
    let registry_metadata_after_recovery = fs::symlink_metadata(&registry_directory)
        .ok()
        .map(|metadata| (metadata.len(), metadata.modified().ok()));
    let missing_archive = workspace
        .root()
        .join("must-not-open-after-exact-recovery-owner-handoff.age.tar");
    assert!(!missing_archive.exists());

    let stale_preview = coordinator
        .preview_restore(&missing_archive, PASSPHRASE)
        .await
        .expect_err("recovered coordinator no longer owns a Store and must reject before I/O");
    assert_import_store_owner_missing(
        stale_preview,
        &[missing_archive.to_string_lossy().as_ref(), PASSPHRASE],
    );
    let repeated_recovery = coordinator
        .recover_startup()
        .await
        .expect_err("one exact coordinator may complete startup recovery only once");
    assert_import_store_owner_missing(
        repeated_recovery,
        &[root.to_string_lossy().as_ref(), PASSPHRASE],
    );

    assert!(!missing_archive.exists());
    assert_eq!(restore_registry_entries(&root), registry_after_recovery);
    assert_eq!(
        fs::symlink_metadata(&registry_directory)
            .ok()
            .map(|metadata| (metadata.len(), metadata.modified().ok())),
        registry_metadata_after_recovery,
        "stale coordinator calls must not create, retire, or rewrite staging entries"
    );
    assert_eq!(
        fs::read(root.join("datasources.db")).expect("reread recovered current bytes"),
        current_after_recovery
    );
    assert_store_bound_restore_lock_child(
        &run_store_bound_restore_lock_child(&root, "expect-open"),
        "stale coordinator rejection must not re-close the gate or reclaim the OS lock",
    );

    let reopened = Store::open_local(StoreOpenOptions::for_root(&root))
        .await
        .expect("Store opens after the claimed recovery returns success");
    reopened.pool().close().await;
    drop(prepared);
    drop(coordinator);
    drop(stale);
}

#[tokio::test(flavor = "current_thread")]
async fn late_resuming_exact_restore_clone_cannot_replay_after_winner_finalizes() {
    assert_restore_recovery_claim_seam_source_contract();

    let workspace = TestWorkspace::new().expect("late-resume recovery workspace");
    let archive = export_restore_archive(&workspace, "late-resume-exact-recovery").await;
    let root = workspace.root().join("late-resume-exact-recovery-root");
    let store = open_seeded_store(&root, "late-resume-current").await;
    let stale = store.clone();
    let coordinator =
        RestoreCoordinator::from_store(store).expect("bind late-resume recovery owner");
    let prepared = coordinator
        .preview_restore(&archive, PASSPHRASE)
        .await
        .expect("prepare late-resume replacement");
    let confirmation = coordinator
        .begin_confirmation(&prepared)
        .expect("enter late-resume confirmation");
    let plugin_tree_switch = crash_point(&RESTORE_SWITCH_CRASH_POINTS, "plugin_tree_switch");
    let interrupted = confirmation
        .finish_with_crash(plugin_tree_switch)
        .await
        .expect_err("interrupt into exact-owner terminal recovery");
    assert!(interrupted.reached_requested_boundary());
    assert_write_gate_closed(&stale, "late-resume-before-admission").await;
    assert_store_bound_restore_lock_child(
        &run_store_bound_restore_lock_child(&root, "expect-locked"),
        "terminal restore must retain the OS lock before either clone is admitted",
    );

    let barriers: [Arc<tokio::sync::Barrier>; 2] =
        std::array::from_fn(|_| Arc::new(tokio::sync::Barrier::new(2)));
    let late_coordinator = coordinator.clone();
    let late_barriers = barriers.clone();
    let late_recovery = tokio::spawn(async move {
        late_coordinator
            .recover_startup_with_admission_interlock_for_test(late_barriers)
            .await
    });
    tokio::time::timeout(Duration::from_secs(10), barriers[0].wait())
        .await
        .expect("late clone observes IDLE before the authoritative admission CAS");

    let winner = tokio::time::timeout(Duration::from_secs(20), coordinator.recover_startup())
        .await
        .expect("ordinary exact clone finishes while the stale clone is paused")
        .expect("ordinary exact clone wins recovery admission");
    assert!(winner.store_may_open());
    assert!(winner.write_gate_is_open());
    assert!(restore_registry_entries(&root).is_empty());
    assert_store_bound_restore_lock_child(
        &run_store_bound_restore_lock_child(&root, "expect-open"),
        "winner finalization releases the OS lock before the late clone resumes",
    );

    let current_after_winner =
        fs::read(root.join("datasources.db")).expect("record winner current bytes");
    let registry_directory = root.join(".hivegui-db-staging-v1");
    let registry_metadata_after_winner = fs::symlink_metadata(&registry_directory)
        .ok()
        .map(|metadata| (metadata.len(), metadata.modified().ok()));
    barriers[1].wait().await;
    let late_error = tokio::time::timeout(Duration::from_secs(10), late_recovery)
        .await
        .expect("late clone returns after admission release")
        .expect("late clone task does not panic")
        .expect_err("a stale IDLE observation cannot authorize a second cold replay");
    assert_import_store_owner_missing(late_error, &[root.to_string_lossy().as_ref(), PASSPHRASE]);

    assert!(restore_registry_entries(&root).is_empty());
    assert_eq!(
        fs::symlink_metadata(&registry_directory)
            .ok()
            .map(|metadata| (metadata.len(), metadata.modified().ok())),
        registry_metadata_after_winner,
        "late admission rejection must not recreate or retire registry state"
    );
    assert_eq!(
        fs::read(root.join("datasources.db")).expect("reread winner current bytes"),
        current_after_winner,
        "late admission rejection must not replay or rewrite current"
    );
    assert_store_bound_restore_lock_child(
        &run_store_bound_restore_lock_child(&root, "expect-open"),
        "late admission rejection must not re-close the gate or reclaim the OS lock",
    );
    let reopened = Store::open_local(StoreOpenOptions::for_root(&root))
        .await
        .expect("parent process reopens after the late loser returns");
    reopened
        .create(
            "write-after-late-recovery-loser",
            "127.0.0.1",
            3309,
            "late-recovery-user",
            b"late-recovery-password",
        )
        .await
        .expect("late loser must not re-close the parent process write gate");
    reopened.pool().close().await;
    drop(reopened);

    drop(prepared);
    drop(coordinator);
    drop(stale);
}

#[tokio::test(flavor = "current_thread")]
async fn stale_owner_observation_is_revalidated_after_restore_maintenance_acquire() {
    assert_restore_preview_owner_revalidation_source_contract();

    let workspace = TestWorkspace::new().expect("preview owner revalidation workspace");
    let archive = export_restore_archive(&workspace, "preview-owner-revalidation").await;
    let missing_archive = workspace
        .root()
        .join("must-not-open-after-stale-owner-observation.age.tar");
    let root = workspace.root().join("preview-owner-revalidation-root");
    let store = open_seeded_store(&root, "preview-owner-current").await;
    let stale = store.clone();
    let stale_preview_coordinator =
        RestoreCoordinator::from_store(store.clone()).expect("bind stale preview coordinator A");
    let recovery_coordinator =
        RestoreCoordinator::from_store(store).expect("bind exact recovery coordinator B");
    let prepared = recovery_coordinator
        .preview_restore(&archive, PASSPHRASE)
        .await
        .expect("coordinator B owns RestoreReady");

    let barriers: [Arc<tokio::sync::Barrier>; 2] =
        std::array::from_fn(|_| Arc::new(tokio::sync::Barrier::new(2)));
    let stale_preview_task_coordinator = stale_preview_coordinator.clone();
    let stale_preview_barriers = barriers.clone();
    let missing_for_task = missing_archive.clone();
    let stale_preview = tokio::spawn(async move {
        stale_preview_task_coordinator
            .preview_restore_with_owner_admission_interlock_for_test(
                &missing_for_task,
                PASSPHRASE,
                stale_preview_barriers,
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(10), barriers[0].wait())
        .await
        .expect("coordinator A observes the old Store owner before maintenance acquire");

    let confirmation = recovery_coordinator
        .begin_confirmation(&prepared)
        .expect("coordinator B crosses confirmation while A is paused");
    let plugin_tree_switch = crash_point(&RESTORE_SWITCH_CRASH_POINTS, "plugin_tree_switch");
    let interrupted = confirmation
        .finish_with_crash(plugin_tree_switch)
        .await
        .expect_err("interrupt B into terminal recovery");
    assert!(interrupted.reached_requested_boundary());
    let recovered = recovery_coordinator
        .recover_startup()
        .await
        .expect("coordinator B completes exact terminal recovery");
    assert!(recovered.store_may_open());
    assert!(recovered.write_gate_is_open());
    assert!(restore_registry_entries(&root).is_empty());
    assert_store_bound_restore_lock_child(
        &run_store_bound_restore_lock_child(&root, "expect-open"),
        "B recovery releases the OS lock before stale A resumes",
    );

    let current_after_recovery =
        fs::read(root.join("datasources.db")).expect("record recovered current bytes");
    let registry_directory = root.join(".hivegui-db-staging-v1");
    let registry_metadata_after_recovery = fs::symlink_metadata(&registry_directory)
        .ok()
        .map(|metadata| (metadata.len(), metadata.modified().ok()));
    assert!(!missing_archive.exists());
    barriers[1].wait().await;
    let stale_error = tokio::time::timeout(Duration::from_secs(10), stale_preview)
        .await
        .expect("stale preview returns after owner handoff")
        .expect("stale preview task does not panic")
        .expect_err("stale owner observation must be rejected after exact maintenance acquire");
    assert_import_store_owner_missing(
        stale_error,
        &[missing_archive.to_string_lossy().as_ref(), PASSPHRASE],
    );

    assert!(!missing_archive.exists());
    assert!(restore_registry_entries(&root).is_empty());
    assert_eq!(
        fs::symlink_metadata(&registry_directory)
            .ok()
            .map(|metadata| (metadata.len(), metadata.modified().ok())),
        registry_metadata_after_recovery,
        "stale preview must not create or retire a staging instance"
    );
    assert_eq!(
        fs::read(root.join("datasources.db")).expect("reread recovered current bytes"),
        current_after_recovery,
        "stale preview must not replay or rewrite current"
    );

    let reopened = Store::open_local(StoreOpenOptions::for_root(&root))
        .await
        .expect("fresh parent Store opens after stale preview rejection");
    let fresh_coordinator =
        RestoreCoordinator::from_store(reopened.clone()).expect("bind fresh Store owner");
    let fresh_prepared = fresh_coordinator
        .preview_restore(&archive, PASSPHRASE)
        .await
        .expect("fresh coordinator acquires RestoreReady after exact stale-lease release");
    fresh_coordinator
        .cancel_preview(&fresh_prepared)
        .await
        .expect("retire fresh proof preview");
    assert!(restore_registry_entries(&root).is_empty());
    reopened
        .create(
            "write-after-stale-preview-rejection",
            "127.0.0.1",
            3310,
            "stale-preview-user",
            b"stale-preview-password",
        )
        .await
        .expect("stale preview rejection must leave the parent write gate open");
    reopened.pool().close().await;
    drop(reopened);
    drop(prepared);
    drop(recovery_coordinator);
    drop(stale_preview_coordinator);
    drop(stale);
}

#[tokio::test(flavor = "current_thread")]
async fn backup_write_gate_crash_is_terminal_only_for_its_exact_root() {
    const CRASH_PASSPHRASE: &str = "must-not-leak-backup-terminal-passphrase";

    let workspace = TestWorkspace::new().expect("backup terminal maintenance workspace");
    let root_a = workspace.root().join("backup-terminal-root-a");
    let root_b = workspace.root().join("backup-terminal-root-b");
    let store_a = open_seeded_store(&root_a, "backup-terminal-current-a").await;
    let store_b = open_seeded_store(&root_b, "backup-terminal-current-b").await;
    let stale_a = store_a.clone();
    let backup_a = BackupCoordinator::from_store(store_a.clone()).expect("bind exact Backup A");
    let backup_contender =
        BackupCoordinator::from_store(store_a.clone()).expect("precreate same-root Backup");
    let restore_contender =
        RestoreCoordinator::from_store(store_a.clone()).expect("precreate same-root Restore");
    let backup_b = BackupCoordinator::from_store(store_b.clone()).expect("bind root B Backup");

    let target_a = workspace.root().join("backup-terminal-a.age.tar");
    let ready_a = backup_a
        .preview_export(&target_a)
        .await
        .expect("Backup A reaches Ready");
    let write_gate_close = crash_point(&BACKUP_CONFIRMATION_CRASH_POINTS, "write_gate_close");
    let interrupted = backup_a
        .confirm_with_crash(ready_a, CRASH_PASSPHRASE, write_gate_close)
        .await
        .expect_err("interrupt Backup A after write-gate close");
    assert!(interrupted.reached_requested_boundary());
    assert_write_gate_closed(&stale_a, "must-not-write-after-backup-terminal").await;

    let missing_archive = workspace
        .root()
        .join("must-not-open-backup-terminal-missing-archive.age.tar");
    let same_root_restore = restore_contender
        .preview_restore(&missing_archive, CRASH_PASSPHRASE)
        .await
        .expect_err("Backup A terminal must block same-root Restore before archive I/O");
    assert_import_maintenance_busy(
        same_root_restore,
        "backup_terminal",
        &[missing_archive.to_string_lossy().as_ref(), CRASH_PASSPHRASE],
    );
    let blocked_same_root_target = workspace.root().join("backup-terminal-blocked.age.tar");
    let same_root_backup = backup_contender
        .preview_export(&blocked_same_root_target)
        .await
        .expect_err("Backup A terminal must block another same-root Backup");
    assert_export_maintenance_busy(
        same_root_backup,
        "backup_terminal",
        &[blocked_same_root_target.to_string_lossy().as_ref()],
    );
    assert_write_gate_closed(&stale_a, "must-remain-closed-after-terminal-contenders").await;
    assert!(!missing_archive.exists());
    assert!(!blocked_same_root_target.exists());

    let target_b = workspace
        .root()
        .join("backup-terminal-independent-b.age.tar");
    let ready_b = backup_b
        .preview_export(&target_b)
        .await
        .expect("root B Backup may enter while root A is terminal");
    let root_a_while_root_b_ready = restore_contender
        .preview_restore(&missing_archive, CRASH_PASSPHRASE)
        .await
        .expect_err("root B Ready must not clear root A Backup terminal maintenance");
    assert_import_maintenance_busy(
        root_a_while_root_b_ready,
        "backup_terminal",
        &[missing_archive.to_string_lossy().as_ref(), CRASH_PASSPHRASE],
    );
    backup_b
        .cancel_preview(&ready_b)
        .await
        .expect("release independent root B Backup preview");
    assert!(!target_b.exists());
    let root_a_after_root_b_cancel = restore_contender
        .preview_restore(&missing_archive, CRASH_PASSPHRASE)
        .await
        .expect_err("root B cancellation must not clear root A Backup terminal maintenance");
    assert_import_maintenance_busy(
        root_a_after_root_b_cancel,
        "backup_terminal",
        &[missing_archive.to_string_lossy().as_ref(), CRASH_PASSPHRASE],
    );
    assert_write_gate_closed(&stale_a, "root B lifecycle must not reopen root A gate").await;

    store_b.pool().close().await;
    assert!(store_a.pool().is_closed());
}
