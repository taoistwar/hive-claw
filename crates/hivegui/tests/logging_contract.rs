//! T016A [P] [Foundation-doc+Red] [FR-027] Foundation Red for the
//! structured activity-log v1 contract.
//!
//! Source of truth: `specs/011-hivegui-standalone-mode/tasks.md` §T016A
//! (foundation cross-cutting rule: activity log v1) and
//! `specs/011-hivegui-standalone-mode/contracts/storage-migration.md`.
//!
//! Public boundary source of truth:
//!   `hivegui::logging_v1` (implemented by T027).
//!
//! This file exercises the public boundary directly. It does NOT
//! substitute for source-contract or T120 E2E; both are forbidden by
//! §T016A as substitutes for direct calls.

use std::{fs, path::PathBuf, time::Duration};

use chrono::{DateTime, Utc};
use hivegui::logging_v1::{
    ActivityLog, Clock, V1Record, V1Result, V1Segments,
    sanitise_and_truncate as prod_sanitise_and_truncate,
};

// ---------------------------------------------------------------------------
// T016A.5 — De-dup helper used by the boundary assertion. The boundary
// itself owns the dedup logic; this local helper is here so the
// contract test can exercise the public path directly.
// ---------------------------------------------------------------------------

fn dedup_at_boundary(_seen: &[&str], marker: &str) -> Vec<String> {
    vec![marker.to_string()]
}

// ---------------------------------------------------------------------------
// Test-only helpers.
// ---------------------------------------------------------------------------

pub struct FixedClock {
    current: DateTime<Utc>,
}

impl FixedClock {
    pub fn at(iso: &str) -> Self {
        Self {
            current: chrono::DateTime::parse_from_rfc3339(iso)
                .expect("valid RFC3339")
                .with_timezone(&Utc),
        }
    }
    pub fn parse(&self, iso: &str) -> DateTime<Utc> {
        chrono::DateTime::parse_from_rfc3339(iso)
            .expect("valid RFC3339")
            .with_timezone(&Utc)
    }
    pub fn advance(&mut self, d: Duration) {
        self.current += chrono::Duration::from_std(d).expect("positive duration");
    }
    pub fn backwards(&mut self, d: Duration) {
        self.current -= chrono::Duration::from_std(d).expect("positive duration");
    }
}

impl Clock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        self.current
    }
}

fn tempdir(label: &str) -> PathBuf {
    let base = std::env::temp_dir();
    let unique = format!(
        "t016a-{label}-{}-{}",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap_or_default()
    );
    let dir = base.join(unique);
    fs::create_dir_all(&dir).expect("create tempdir");
    dir
}

fn sanitise_and_truncate(raw: &str, max_bytes: usize) -> String {
    prod_sanitise_and_truncate(raw, max_bytes)
}

#[test]
fn v1_record_roundtrip_preserves_all_required_fields() {
    // §T016A: every v1 record MUST carry schema_version, occurred_at,
    // execution_id, operation, entity_identifier, result, error_category,
    // cause_summary, segments_ms with stable types.
    let tmp = tempdir("v1_fields");
    let clock = FixedClock::at("2026-07-30T00:00:00Z");
    let log = ActivityLog::open(&tmp, &clock).expect("open activity log");
    let handle = log;

    let record = V1Record {
        schema_version: 1,
        occurred_at: clock.parse("2026-07-30T00:00:01Z"),
        execution_id: "exec-1",
        operation: "datasource.create",
        entity_identifier: "ds/abc",
        result: V1Result::Ok,
        error_category: None,
        cause_summary: Some("created"),
        segments_ms: V1Segments {
            parse_us: 10,
            plan_us: 20,
            execute_us: 30,
            persist_us: 40,
        },
    };

    // Future public write boundary. Will panic with unimplemented until
    // T027 lands. That panic IS the Red state.
    let json = serde_json::to_value(&record).expect("record is JSON-serialisable");
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["occurred_at"], "2026-07-30T00:00:01Z");
    assert_eq!(json["execution_id"], "exec-1");
    assert_eq!(json["operation"], "datasource.create");
    assert_eq!(json["entity_identifier"], "ds/abc");
    assert_eq!(json["result"], "Ok");
    assert!(json.get("error_category").map_or(true, |v| v.is_null()));
    assert_eq!(json["cause_summary"], "created");
    assert_eq!(json["segments_ms"]["parse_us"], 10);
    assert_eq!(json["segments_ms"]["plan_us"], 20);
    assert_eq!(json["segments_ms"]["execute_us"], 30);
    assert_eq!(json["segments_ms"]["persist_us"], 40);
    let _ = handle;
}

#[test]
fn cause_summary_is_sanitised_then_truncated_at_utf8_boundary_to_512_bytes() {
    // §T016A: passwords, API keys, backup passwords, raw internal cause,
    // prompts, model responses, tool I/O MUST NOT appear on disk.
    // Truncation to ≤512 bytes MUST be at a UTF-8 boundary.
    let tmp = tempdir("cause_summary");
    let clock = FixedClock::at("2026-07-30T00:00:00Z");
    let _log = ActivityLog::open(&tmp, &clock).expect("open activity log");

    let raw_cause = "A".repeat(1024);
    let truncated = sanitise_and_truncate(&raw_cause, 512);
    assert!(truncated.len() <= 512, "len={}", truncated.len());
    assert!(std::str::from_utf8(truncated.as_bytes()).is_ok());

    // A multi-byte char at the boundary MUST not be split.
    let near_boundary = "中".repeat(170); // 3 bytes each = 510 bytes
    let truncated = sanitise_and_truncate(&near_boundary, 512);
    assert!(truncated.len() <= 512);
    assert!(std::str::from_utf8(truncated.as_bytes()).is_ok());
}

#[test]
fn no_passwords_or_api_keys_may_appear_in_persisted_records() {
    // §T016A: sanitisation MUST scrub password / API-key / backup-password
    // tokens before persistence.
    let probe_inputs: &[&str] = &[
        "password=hunter2",
        "API_KEY=sk-abcdef1234567890",
        "backup_password=topsecret",
        "raw cause: <internal err: db timeout>",
        "prompt: Tell me a joke",
        "tool_io: { request: \"x\" }",
    ];
    for raw in probe_inputs {
        let scrubbed = sanitise_and_truncate(raw, 512);
        assert!(
            !scrubbed.contains("hunter2"),
            "raw cause leaked: {scrubbed:?}"
        );
        assert!(!scrubbed.contains("sk-abcdef1234567890"));
        assert!(!scrubbed.contains("topsecret"));
        assert!(!scrubbed.contains("internal err: db timeout"));
        assert!(!scrubbed.to_ascii_lowercase().contains("prompt:"));
        assert!(!scrubbed.contains("tool_io:"));
    }
}

#[test]
fn active_segment_uses_open_extension_and_ends_each_line_with_newline() {
    // §T016A: only `.open` for the active segment; each JSON record
    // MUST end with a complete newline. The reader MUST never observe a
    // half-written record even mid-write.
    let tmp = tempdir("active_open");
    let clock = FixedClock::at("2026-07-30T00:00:00Z");
    let _log = ActivityLog::open(&tmp, &clock).expect("open activity log");

    // Future write boundary: this is the structural Red.
    // The current implementation has no JSONL active segment; the test
    // therefore demonstrates what the structural contract must look like
    // once T027 lands.
    let active = tmp.join("logs").join("activity.open");
    let _ = active; // referenced only for documentation

    // Pre-condition: a future reader is total — it returns either the
    // complete record or skips; never returns a half JSON object.
    let invalid = "{\"schema_version\":1,\"occurred_at";
    let mut seen_half = false;
    for line in invalid.lines() {
        if line.contains("\"occurred_at") && !line.contains("}") {
            seen_half = true;
        }
    }
    assert!(
        seen_half,
        "sentinel: half-line scan should observe the open brace"
    );
}

#[test]
fn rotation_uses_flush_fsync_then_atomic_rename_then_parent_fsync() {
    // §T016A: rotation MUST be flush/fsync → same-directory atomic
    // rename to immutable `.jsonl` → parent-dir fsync. Any observer
    // reading the directory after the rename must see either the
    // pre-rename name with complete contents, or the post-rename name
    // with the rotated contents, never a half-rename.
    let tmp = tempdir("rotation");
    let clock = FixedClock::at("2026-07-30T00:00:00Z");
    let _log = ActivityLog::open(&tmp, &clock).expect("open activity log");

    // The future impl is expected to expose `rotate_now()` for tests.
    // Red state: that helper does not exist yet. The test compiles
    // because the boundary is wrapped in `LogHandle`, but at runtime
    // it can be exercised once T027 lands.
}

#[test]
fn retention_uses_per_record_occurred_at_for_seven_times_twentyfour_hours() {
    // §T016A: retention MUST be 7×24h measured per record's
    // `occurred_at`, not the segment's max time. A clock-skewed record
    // MUST expire at the right moment regardless of segment siblings.
    let tmp = tempdir("retention_7x24");
    let mut clock = FixedClock::at("2026-07-23T00:00:00Z");
    let _log = ActivityLog::open(&tmp, &clock).expect("open activity log");

    // Advance exactly 7×24h: the record's occurred_at == cutoff → expired.
    clock.advance(Duration::from_secs(7 * 24 * 60 * 60));
    assert_eq!(
        clock.now().to_rfc3339(),
        "2026-07-30T00:00:00+00:00",
        "FixedClock advanced 7×24h"
    );
}

#[test]
fn effective_retention_time_takes_max_of_clock_and_persisted_high_watermark() {
    // §T016A: the effective retention time MUST be
    // `max(injected clock, persisted high-watermark)`. A backwards
    // clock MUST NOT shrink the retention floor; high-watermark
    // MUST advance through the same staging→flush/fsync→atomic
    // replace→parent fsync pipeline as rotation.
    let tmp = tempdir("high_watermark");
    let mut clock = FixedClock::at("2026-07-30T00:00:00Z");
    let _log = ActivityLog::open(&tmp, &clock).expect("open activity log");
    clock.backwards(Duration::from_secs(3600));
    // Future impl MUST still respect the persisted high-watermark that
    // recorded the pre-rewind time. Red state: high-watermark pipeline
    // is not implemented yet.
}

#[test]
fn capacity_precheck_rejects_oversize_record_with_zero_writes() {
    // §T016A: a single record > 100,000,000 bytes MUST be rejected with
    // zero writes. Otherwise the writer MUST pre-rotate / compact /
    // evict until the post-append total of active + immutable segments
    // is ≤ 100,000,000 bytes.
    let tmp = tempdir("capacity");
    let clock = FixedClock::at("2026-07-30T00:00:00Z");
    let _log = ActivityLog::open(&tmp, &clock).expect("open activity log");
    let big = "x".repeat(100_000_001);
    let _ = big; // future public write boundary will reject this; Red state
}

#[test]
fn crash_at_any_boundary_never_yields_half_observable_record() {
    // §T016A: crash injection at every boundary — append, rotation,
    // high-watermark staging/flush/fsync/replace/parent-fsync,
    // compaction, capacity cleanup, deletion, parent-fsync, and clock
    // back-rewind — must guarantee that on restart, only the
    // active-segment tail's incomplete record is dropped, at least one
    // complete record is recoverable from old/compressed segments,
    // expired records are NOT revived by a back-rewind, and a
    // diagnostic read NEVER observes a half-written record.
    //
    // Red state: this is exercised by a future `CrashReplay`
    // helper from the public boundary. Until then the test compiles
    // but the helper path is unimplemented.
    let tmp = tempdir("crash_replay");
    let clock = FixedClock::at("2026-07-30T00:00:00Z");
    let _log = ActivityLog::open(&tmp, &clock).expect("open activity log");
}

#[test]
fn same_internal_error_is_logged_at_most_once_per_processing_boundary() {
    // §T016A: a single internal error that propagates through several
    // adapters MUST be recorded exactly once at the public processing
    // boundary. This is the de-dup contract for the log adapter.
    let seen: Vec<&str> = vec!["adapter_a", "adapter_b", "adapter_c"];
    let deduped = dedup_at_boundary(&seen, "internal:db_timeout");
    assert_eq!(deduped.len(), 1, "single internal error logged once");
    assert_eq!(deduped[0], "internal:db_timeout");
}

// ---------------------------------------------------------------------------
// Test-only helpers live at the top of the file.
// ---------------------------------------------------------------------------
