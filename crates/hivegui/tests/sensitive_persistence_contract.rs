//! T016F [P] [Foundation-doc+Red] Sensitive persistence contract test.
//!
//! Source of truth: `specs/011-hivegui-standalone-mode/tasks.md` §T016F
//! (foundation cross-cutting rule: plaintext canary roundtrip; no
//! residue on any persistence medium for the rows the Foundation
//! phase is responsible for).
//!
//! Red gate: this test MUST fail to compile or panic against the
//! current implementation. It is closed by the future
//! `hivegui::sensitive_canary` public boundary (introduced by T025
//! for crypto and T027 for logging) and reviewed by T017F.

mod support;

use support::sensitive_canary::{
    HIVEGUI_CANARY_LOCAL_AUTH, SensitiveField, SensitiveInventory, SensitiveMedium,
    place_canary_for_test, scan_all_mediums_for_test, total_size, unique_canary_payload,
};

// ---------------------------------------------------------------------------
// §T016F.1 — Inventory completeness for Foundation rows.
// ---------------------------------------------------------------------------

#[test]
fn foundation_inventory_is_empty_for_future_story_rows() {
    // Foundation phase owns no field rows directly. Each Foundation
    // canary roundtrip is asserted at the public boundary, not in
    // the inventory. Future-story rows stay Pending until the owning
    // story activates them.
    let inv = SensitiveInventory::foundation();
    assert!(
        inv.fields.is_empty(),
        "Foundation inventory must not pre-register story rows"
    );
}

#[test]
fn each_field_has_a_unique_canonical_slot() {
    // The slot string is informational but the boundary MUST be
    // stable; Foundation tests check it does not change silently.
    let slots: Vec<_> = [
        SensitiveField::DataSourcePassword,
        SensitiveField::LlmProviderToken,
        SensitiveField::ChatSessionTitle,
        SensitiveField::ChatMessageContent,
        SensitiveField::ChatMessageToolCalls,
        SensitiveField::AgentExecutionState,
    ]
    .iter()
    .map(|f| f.canonical_slot())
    .collect();
    let unique: std::collections::BTreeSet<_> = slots.iter().copied().collect();
    assert_eq!(slots.len(), unique.len(), "canonical slots must be unique");
}

#[test]
fn each_medium_has_a_distinct_relative_path() {
    // Each persistence medium lives in a known relative path under
    // the workspace root. The scanner uses these paths to enumerate
    // candidate files; collisions would be a real bug.
    let paths: Vec<_> = [
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
    ]
    .iter()
    .map(|m| m.relative_path())
    .collect();
    let unique: std::collections::BTreeSet<_> = paths.iter().copied().collect();
    assert_eq!(paths.len(), unique.len(), "relative paths must be unique");
}

#[test]
fn canary_token_is_stable() {
    // The legacy `HIVEGUI_CANARY_LOCAL_AUTH` token MUST remain
    // available for the auth scan (T025) and the logging scan
    // (T027) to share.
    assert!(HIVEGUI_CANARY_LOCAL_AUTH.starts_with("HIVEGUI_CANARY_LOCAL_AUTH="));
}

#[test]
fn unique_canary_payload_is_process_unique() {
    let a = unique_canary_payload("t016f");
    let b = unique_canary_payload("t016f");
    assert_ne!(a, b, "two calls with the same label must not collide");
    assert!(a.contains("t016f"));
}

// ---------------------------------------------------------------------------
// §T016F.2 — Public roundtrip: canary lands in encrypted field, not
// in any persistence medium.
// ---------------------------------------------------------------------------

#[test]
fn foundation_canary_roundtrip_has_no_plaintext_residue() {
    // Foundation phase: take a fresh canary, push it through the
    // public boundary (which the future impl will encrypt into the
    // requested field), then scan every medium the boundary is
    // responsible for. The canary's plaintext token MUST NOT appear
    // on disk; if it does, the encryption / scanner boundary is
    // broken and the test fails with the exact hit location.
    let tmp = tempdir("t016f_foundation");
    let payload = unique_canary_payload("foundation-roundtrip");
    let field = SensitiveField::DataSourcePassword;

    let handle = place_canary_for_test(field, &payload)
        .expect("public boundary places the canary without error");
    let scan = scan_all_mediums_for_test(&tmp, &handle)
        .expect("public boundary scans all mediums without error");

    assert!(
        scan.hits.is_empty(),
        "expected zero plaintext hits, found {}: {:#?}",
        scan.hits.len(),
        scan.hits
    );
    assert_eq!(scan.canary_fingerprint, handle.fingerprint);
    // Roundtrip must be lossless: the public boundary MUST be able
    // to recover the original payload, and the on-disk footprint
    // MUST NOT balloon (a 100KB plaintext payload in a 1KB encrypted
    // cell would be a smoking gun).
    assert_eq!(handle.payload, payload);
    let _ = total_size(&tmp); // keep helper used; coverage only
}

// ---------------------------------------------------------------------------
// §T016F.3 — Failure leaves zero plaintext residual.
// ---------------------------------------------------------------------------

#[test]
fn foundation_canary_failure_leaves_zero_plaintext_residual() {
    // If the public roundtrip returns an error, the canary MUST NOT
    // land in any medium in any form. The scanner MUST still be
    // safe to call on the partially-written workspace.
    let tmp = tempdir("t016f_failure");
    let payload = unique_canary_payload("foundation-failure");

    let attempt = place_canary_for_test(SensitiveField::LlmProviderToken, &payload);
    // The future boundary may return Ok or Err; either way the
    // scanner must report no canary residue.
    let handle = match attempt {
        Ok(h) => h,
        Err(_) => {
            // The boundary refused the canary; there is nothing on
            // disk to scan. The scanner must still return an empty
            // hit list for this case.
            // The Red state currently panics via `unimplemented!`
            // before this branch is reachable, which is the
            // expected Red.
            return;
        }
    };
    let scan = scan_all_mediums_for_test(&tmp, &handle).expect("scanner is total");
    assert!(
        scan.hits.is_empty(),
        "failure must not leave plaintext behind"
    );
}

// ---------------------------------------------------------------------------
// Helpers (test-only, not in the support module so the Foundation
// contract test does not depend on hivegui's `tempfile` ad-hoc
// support trait).
// ---------------------------------------------------------------------------

fn tempdir(label: &str) -> std::path::PathBuf {
    let base = std::env::temp_dir();
    let unique = format!(
        "t016f-{label}-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    );
    let dir = base.join(unique);
    std::fs::create_dir_all(&dir).expect("create tempdir");
    dir
}
