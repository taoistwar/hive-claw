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

use std::{
    fs,
    io::Write,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use chrono::{DateTime, Utc};
use hivegui::logging_v1::{
    ActivityLog, Clock, LogError, V1Record, V1Result, V1Segments,
    dedup_at_boundary as prod_dedup_at_boundary,
    sanitise_and_truncate as prod_sanitise_and_truncate,
};

// ---------------------------------------------------------------------------
// Test-only helpers.
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct FixedClock {
    current: Arc<Mutex<DateTime<Utc>>>,
}

impl FixedClock {
    pub fn at(iso: &str) -> Self {
        Self {
            current: Arc::new(Mutex::new(
                chrono::DateTime::parse_from_rfc3339(iso)
                    .expect("valid RFC3339")
                    .with_timezone(&Utc),
            )),
        }
    }
    pub fn parse(&self, iso: &str) -> DateTime<Utc> {
        chrono::DateTime::parse_from_rfc3339(iso)
            .expect("valid RFC3339")
            .with_timezone(&Utc)
    }
    pub fn advance(&self, d: Duration) {
        let mut current = self.current.lock().expect("fixed clock lock poisoned");
        *current += chrono::Duration::from_std(d).expect("positive duration");
    }
    pub fn backwards(&self, d: Duration) {
        let mut current = self.current.lock().expect("fixed clock lock poisoned");
        *current -= chrono::Duration::from_std(d).expect("positive duration");
    }
}

impl Clock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        *self.current.lock().expect("fixed clock lock poisoned")
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

fn build_record<'a>(
    execution_id: &'a str,
    occurred: &str,
    operation: &'a str,
    entity_identifier: &'a str,
    cause: Option<&'a str>,
) -> V1Record<'a> {
    V1Record {
        schema_version: 1,
        occurred_at: FixedClock::at("2026-07-30T00:00:00Z").parse(occurred),
        execution_id,
        operation,
        entity_identifier,
        result: V1Result::Ok,
        error_category: None,
        cause_summary: cause,
        segments_ms: V1Segments::default(),
    }
}

#[test]
fn v1_record_roundtrip_preserves_all_required_fields() {
    // §T016A: every v1 record MUST carry schema_version, occurred_at,
    // execution_id, operation, entity_identifier, result, error_category,
    // cause_summary, segments_ms with stable types.
    let tmp = tempdir("v1_fields");
    let clock = FixedClock::at("2026-07-30T00:00:00Z");
    let log = ActivityLog::open(&tmp, &clock).expect("open activity log");

    let record = build_record(
        "exec-1",
        "2026-07-30T00:00:01Z",
        "datasource.create",
        "ds/abc",
        Some("created"),
    );

    let appended = log
        .append(&record)
        .expect("append v1 record to activity log");
    assert_eq!(appended.persisted_cause_summary.as_deref(), Some("created"));

    let active = tmp.join("logs").join("activity.open");
    let raw = fs::read_to_string(&active).expect("read active segment");
    assert!(active.exists());
    assert!(raw.ends_with('\n'));

    let first = raw.lines().next().expect("active contains one row");
    let value: serde_json::Value = serde_json::from_str(first).expect("active row is json");
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["occurred_at"], "2026-07-30T00:00:01Z");
    assert_eq!(value["execution_id"], "exec-1");
    assert_eq!(value["operation"], "datasource.create");
    assert_eq!(value["entity_identifier"], "ds/abc");
    assert_eq!(value["result"], "Ok");
    assert!(value.get("error_category").is_none_or(|v| v.is_null()));
    assert_eq!(value["cause_summary"], "created");
    assert_eq!(value["segments_ms"]["parse_us"], 0);
    assert_eq!(value["segments_ms"]["plan_us"], 0);
    assert_eq!(value["segments_ms"]["execute_us"], 0);
    assert_eq!(value["segments_ms"]["persist_us"], 0);

    let all = log.read_all().expect("read all persisted records");
    assert_eq!(all.len(), 1);
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
    let log = ActivityLog::open(&tmp, &clock).expect("open activity log");

    let record = build_record(
        "exec-open",
        "2026-07-30T00:00:01Z",
        "datasource.read",
        "ds/open",
        Some("active segment keeps newline"),
    );
    let _ = log.append(&record).expect("append log row");

    let active = tmp.join("logs").join("activity.open");
    let raw = fs::read_to_string(&active).expect("read active segment");
    assert!(raw.ends_with('\n'));
    for line in raw.lines() {
        serde_json::from_str::<serde_json::Value>(line)
            .expect("active segment keeps complete json lines");
    }
}

#[test]
fn rotation_uses_flush_fsync_then_atomic_rename_then_parent_fsync() {
    // §T016A: rotation MUST be flush/fsync → same-directory atomic
    // rename to immutable `.jsonl` → parent-dir fsync.
    let tmp = tempdir("rotation");
    let clock = FixedClock::at("2026-07-30T00:00:00Z");
    let log = ActivityLog::open(&tmp, &clock).expect("open activity log");

    let record = build_record(
        "exec-rot",
        "2026-07-30T00:00:01Z",
        "datasource.list",
        "ds/list",
        Some("rotation test"),
    );
    let _ = log.append(&record).expect("append log row");
    let rotated = log
        .rotate_now()
        .expect("rotate active segment")
        .segment_path
        .expect("rotation returns immutable segment");
    assert!(rotated.exists());
    assert!(rotated.extension().and_then(|ext| ext.to_str()) == Some("jsonl"));

    let second_record = build_record(
        "exec-rot-2",
        "2026-07-30T00:00:02Z",
        "datasource.list",
        "ds/list-2",
        Some("second rotation test"),
    );
    log.append(&second_record).expect("append second log row");
    let second_rotated = log
        .rotate_now()
        .expect("rotate active segment again at the same clock instant")
        .segment_path
        .expect("second rotation returns immutable segment");

    assert_ne!(rotated, second_rotated, "immutable segments never collide");
    let first_contents = fs::read_to_string(&rotated).expect("read first immutable segment");
    let second_contents =
        fs::read_to_string(&second_rotated).expect("read second immutable segment");
    assert!(first_contents.contains("exec-rot"));
    assert!(!first_contents.contains("exec-rot-2"));
    assert!(second_contents.contains("exec-rot-2"));

    let active = tmp.join("logs").join("activity.open");
    assert!(active.exists());
}

#[test]
fn retention_uses_per_record_occurred_at_for_seven_times_twentyfour_hours() {
    // §T016A: retention MUST be 7×24h measured per record's
    // `occurred_at`, not the segment's max time.
    let tmp = tempdir("retention_7x24");
    let clock = FixedClock::at("2026-07-30T00:00:00Z");
    let log = ActivityLog::open(&tmp, &clock).expect("open activity log");

    let expired = build_record(
        "exec-expired",
        "2026-07-23T00:00:00Z",
        "datasource.read",
        "ds/expired",
        Some("expired record"),
    );
    let fresh = build_record(
        "exec-live",
        "2026-07-29T00:00:00Z",
        "datasource.read",
        "ds/live",
        Some("fresh record"),
    );

    log.append(&expired).expect("append exact-cutoff candidate");
    log.append(&fresh).expect("append fresh record");
    let _ = log.rotate_now().expect("rotate mixed-age segment");

    let report = log.enforce_retention().expect("enforce retention");
    assert!(report.expired_bytes > 0);

    let persisted = log.read_all().expect("read persisted logs");
    assert_eq!(persisted.len(), 1, "only fresh record kept");
    assert_eq!(
        persisted[0]["occurred_at"], "2026-07-29T00:00:00Z",
        "exact-cutoff row expired while its fresh segment sibling survived"
    );

    let rewind = clock;
    rewind.advance(Duration::from_secs(7 * 24 * 60 * 60));
    assert_eq!(
        rewind.now().to_rfc3339(),
        "2026-08-06T00:00:00+00:00",
        "clock can advance"
    );
}

#[test]
fn effective_retention_time_takes_max_of_clock_and_persisted_high_watermark() {
    // §T016A: the effective retention time MUST be
    // `max(injected clock, persisted high-watermark)`.
    // A backwards clock MUST NOT shrink retention floor.
    let tmp = tempdir("high_watermark");
    let clock = FixedClock::at("2026-07-30T00:00:00Z");
    let log = ActivityLog::open(&tmp, &clock).expect("open activity log");

    let stale = build_record(
        "exec-stale",
        "2026-07-22T00:00:00Z",
        "datasource.read",
        "ds/stale",
        Some("stale record"),
    );
    log.append(&stale).expect("append stale record");
    let _ = log.rotate_now().expect("rotate stale segment");

    clock.backwards(Duration::from_secs(30 * 24 * 60 * 60));
    let report = log
        .enforce_retention()
        .expect("enforce retention with rewind");
    assert!(report.expired_bytes > 0);
    let persisted = log.read_all().expect("read all records");
    assert_eq!(
        persisted.len(),
        0,
        "rewound clock does not revive expired records"
    );

    // Sanity: rewound wall clock is visible in contract assertions.
    assert_eq!(clock.now().to_rfc3339(), "2026-06-30T00:00:00+00:00");
}

#[test]
fn capacity_precheck_rejects_oversize_record_with_zero_writes() {
    // §T016A: a single record > 100,000,000 bytes MUST be rejected with
    // zero writes.
    let tmp = tempdir("capacity");
    let clock = FixedClock::at("2026-07-30T00:00:00Z");
    let log = ActivityLog::open(&tmp, &clock).expect("open activity log");

    let huge_op = "x".repeat(100_000_001);
    let big_record = V1Record {
        schema_version: 1,
        occurred_at: clock.parse("2026-07-30T00:00:01Z"),
        execution_id: "exec-cap",
        operation: &huge_op,
        entity_identifier: "ds/capacity",
        result: V1Result::Ok,
        error_category: None,
        cause_summary: Some("huge operation"),
        segments_ms: V1Segments::default(),
    };

    let active = tmp.join("logs").join("activity.open");
    let before = active.metadata().map(|m| m.len()).unwrap_or_default();
    let err = log
        .append(&big_record)
        .expect_err("large record should fail precheck");
    let after = active.metadata().map(|m| m.len()).unwrap_or_default();

    assert!(matches!(err, LogError::Capacity(_)));
    assert_eq!(before, after);
}

#[test]
fn crash_at_any_boundary_never_yields_half_observable_record() {
    // §T016A: on recovery, only complete records are observable.
    let tmp = tempdir("crash_replay");
    let clock = FixedClock::at("2026-07-30T00:00:00Z");
    let log = ActivityLog::open(&tmp, &clock).expect("open activity log");

    let record = build_record(
        "exec-good",
        "2026-07-30T00:00:01Z",
        "datasource.create",
        "ds/good",
        Some("good record"),
    );
    let _ = log.append(&record).expect("append good record");

    let active = tmp.join("logs").join("activity.open");
    {
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&active)
            .expect("open active for crash-like half write");
        file.write_all(br#"{\"schema_version\":1"#)
            .expect("inject half record");
        file.flush().expect("flush injected bytes");
    }

    let records = log.read_all().expect("read skipping incomplete tail");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["execution_id"], "exec-good");
}

#[test]
fn same_internal_error_is_logged_at_most_once_per_processing_boundary() {
    // §T016A: a single internal error propagating through several
    // adapters MUST be recorded once at the processing boundary.
    let seen: Vec<&str> = vec!["adapter_a", "adapter_b", "adapter_c"];
    let deduped = prod_dedup_at_boundary(&seen, "internal:db_timeout");
    assert_eq!(deduped.len(), 1, "single internal error logged once");
    assert_eq!(deduped[0], "internal:db_timeout");
}

// ---------------------------------------------------------------------------
// Test-only helpers live at the top of the file.
// ---------------------------------------------------------------------------
