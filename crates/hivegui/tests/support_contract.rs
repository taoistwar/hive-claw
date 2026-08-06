mod support;

use std::{fs, io};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use chrono::NaiveDate;
use support::{
    CapturedHttpServer, FIXTURE_DEVICE_KEY, FaultInjector, MockHttpResponse, TestWorkspace,
    performance::{
        AGENT_ACTION_DISPATCH_ID, BASELINE_SCHEMA_VERSION, BaselineApproval, BenchmarkBaseline,
        BenchmarkReport, ComparisonOutcome, CompatibilityError, EnvironmentFingerprint,
        FIXTURE_VERSION, Percentile, PercentilesNs, RegressionException, TOOL_DISPATCH_ID,
        WORKFLOW_100_NODE_ID, compare_to_baseline, target_specs,
    },
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn workspace_isolates_paths_key_and_sqlite() {
    let workspace = TestWorkspace::new().expect("create isolated workspace");

    assert!(workspace.database_path().starts_with(workspace.root()));
    assert!(workspace.device_key_path().starts_with(workspace.root()));
    assert!(workspace.plugin_root().starts_with(workspace.root()));
    assert!(workspace.log_root().starts_with(workspace.root()));

    let environment = workspace.environment();
    assert_eq!(environment.len(), 5);
    assert!(
        environment
            .iter()
            .all(|(_, value)| { std::path::Path::new(value).starts_with(workspace.root()) })
    );

    workspace
        .ensure_fixture_device_key()
        .expect("create fixture device key");
    workspace
        .ensure_fixture_device_key()
        .expect("matching key is idempotent");
    assert_eq!(
        fs::read(workspace.device_key_path()).expect("read fixture key"),
        FIXTURE_DEVICE_KEY
    );
    #[cfg(unix)]
    assert_eq!(
        fs::metadata(workspace.device_key_path())
            .expect("device key metadata")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );

    let pool = workspace.sqlite_pool().await.expect("open sqlite fixture");
    let value: i64 = sqlx::query_scalar("SELECT 42")
        .fetch_one(&pool)
        .await
        .expect("query fixture database");
    assert_eq!(value, 42);
}

#[test]
fn fault_injector_fails_exactly_the_requested_checks() {
    let injector = FaultInjector::default();
    injector.fail_next("before_commit", 2);

    assert_eq!(
        injector.check("before_commit").unwrap_err().point,
        "before_commit"
    );
    assert!(injector.check("before_commit").is_err());
    assert!(injector.check("before_commit").is_ok());
    assert!(injector.check("unconfigured").is_ok());
}

#[tokio::test]
async fn mock_server_captures_request_and_returns_queued_json() -> io::Result<()> {
    let server = CapturedHttpServer::spawn(vec![MockHttpResponse::json(
        200,
        serde_json::json!({"reply": "local"}),
    )])
    .await?;

    let body = br#"{"model":"fixture","messages":[]}"#;
    let mut stream = tokio::net::TcpStream::connect(server.address()).await?;
    stream
        .write_all(
            format!(
                "POST /v1/chat/completions HTTP/1.1\r\nhost: fixture\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n",
                body.len()
            )
            .as_bytes(),
        )
        .await?;
    stream.write_all(body).await?;

    let mut response = Vec::new();
    stream.read_to_end(&mut response).await?;
    let response = String::from_utf8(response).expect("mock response is UTF-8");
    assert!(response.starts_with("HTTP/1.1 200 OK"));
    assert!(response.contains(r#"{"reply":"local"}"#));

    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    let request = String::from_utf8(requests[0].clone()).expect("captured request is UTF-8");
    assert!(request.starts_with("POST /v1/chat/completions HTTP/1.1"));
    assert!(request.ends_with(r#"{"model":"fixture","messages":[]}"#));
    assert_eq!(server.base_url(), format!("http://{}", server.address()));

    Ok(())
}

#[test]
fn performance_targets_have_fixed_boundaries_samples_and_budgets() {
    let targets = target_specs();

    assert_eq!(targets.len(), 3);
    assert_eq!(targets[0].id, AGENT_ACTION_DISPATCH_ID);
    assert_eq!(targets[0].owner_task, "T122");
    assert_eq!(targets[0].p95_budget_ns, 200_000_000);
    assert_eq!(targets[1].id, TOOL_DISPATCH_ID);
    assert_eq!(targets[1].owner_task, "T103");
    assert_eq!(targets[1].p95_budget_ns, 50_000_000);
    assert_eq!(targets[2].id, WORKFLOW_100_NODE_ID);
    assert_eq!(targets[2].owner_task, "T093");
    assert_eq!(targets[2].p95_budget_ns, 100_000_000);
    assert_eq!(targets[2].workflow_node_count, Some(100));
    assert!(
        targets
            .iter()
            .all(|target| target.measured_samples == 100 && target.warmup_iterations == 10)
    );
    assert!(
        targets
            .iter()
            .all(|target| !target.excluded_time.is_empty())
    );
}

#[test]
fn percentiles_use_nearest_rank_and_require_the_fixed_sample_count() {
    let samples = (1..=100).collect::<Vec<_>>();
    assert_eq!(
        PercentilesNs::from_samples(&samples).unwrap(),
        PercentilesNs {
            p50: 50,
            p95: 95,
            p99: 99,
        }
    );

    let result =
        BenchmarkReport::from_samples(target_specs()[0].clone(), fixture_environment(), &[1, 2, 3]);
    assert!(result.is_err());
}

#[test]
fn comparator_accepts_exactly_ten_percent_and_blocks_any_larger_percentile() {
    let baseline = fixture_baseline(PercentilesNs {
        p50: 100,
        p95: 100,
        p99: 100,
    });
    let exact_limit = fixture_report(PercentilesNs {
        p50: 110,
        p95: 110,
        p99: 110,
    });
    let result = compare_to_baseline(
        &exact_limit,
        Some(&baseline),
        None,
        NaiveDate::from_ymd_opt(2026, 7, 22).unwrap(),
    )
    .unwrap();
    assert_eq!(result.outcome, ComparisonOutcome::Passed);

    for (percentile, current) in [
        (
            Percentile::P50,
            PercentilesNs {
                p50: 111,
                p95: 100,
                p99: 100,
            },
        ),
        (
            Percentile::P95,
            PercentilesNs {
                p50: 100,
                p95: 111,
                p99: 100,
            },
        ),
        (
            Percentile::P99,
            PercentilesNs {
                p50: 100,
                p95: 100,
                p99: 111,
            },
        ),
    ] {
        let result = compare_to_baseline(
            &fixture_report(current),
            Some(&baseline),
            None,
            NaiveDate::from_ymd_opt(2026, 7, 22).unwrap(),
        )
        .unwrap();
        assert_eq!(result.outcome, ComparisonOutcome::Blocked);
        assert_eq!(result.regressions[0].percentile, percentile);
    }
}

#[test]
fn regression_exception_requires_signer_reason_scope_and_current_review_date() {
    let baseline = fixture_baseline(PercentilesNs {
        p50: 100,
        p95: 100,
        p99: 100,
    });
    let current = fixture_report(PercentilesNs {
        p50: 100,
        p95: 111,
        p99: 100,
    });
    let valid = RegressionException {
        signer: "performance-reviewer".into(),
        reason: "documented temporary tradeoff".into(),
        impact_scope: "tool dispatch on linux-x86_64".into(),
        review_due: "2026-08-01".into(),
    };
    let result = compare_to_baseline(
        &current,
        Some(&baseline),
        Some(&valid),
        NaiveDate::from_ymd_opt(2026, 7, 22).unwrap(),
    )
    .unwrap();
    assert_eq!(result.outcome, ComparisonOutcome::ApprovedException);

    let expired = RegressionException {
        review_due: "2026-07-21".into(),
        ..valid
    };
    let result = compare_to_baseline(
        &current,
        Some(&baseline),
        Some(&expired),
        NaiveDate::from_ymd_opt(2026, 7, 22).unwrap(),
    )
    .unwrap();
    assert_eq!(result.outcome, ComparisonOutcome::Blocked);
    assert_eq!(
        result.exception_rejection.as_deref(),
        Some("regression exception review_due has expired")
    );
}

#[test]
fn comparator_rejects_fixture_and_environment_mismatches() {
    let baseline = fixture_baseline(PercentilesNs {
        p50: 100,
        p95: 100,
        p99: 100,
    });
    let mut current = fixture_report(PercentilesNs {
        p50: 100,
        p95: 100,
        p99: 100,
    });
    current.fixture_version = "different-fixture".into();
    assert_eq!(
        compare_to_baseline(
            &current,
            Some(&baseline),
            None,
            NaiveDate::from_ymd_opt(2026, 7, 22).unwrap(),
        )
        .unwrap_err(),
        CompatibilityError::FixtureVersion
    );

    let mut current = fixture_report(PercentilesNs {
        p50: 100,
        p95: 100,
        p99: 100,
    });
    current.environment.cpu_model = "different CPU".into();
    assert_eq!(
        compare_to_baseline(
            &current,
            Some(&baseline),
            None,
            NaiveDate::from_ymd_opt(2026, 7, 22).unwrap(),
        )
        .unwrap_err(),
        CompatibilityError::Environment
    );
}

#[test]
fn report_schema_round_trips_and_missing_baseline_stays_pending() {
    let report = fixture_report(PercentilesNs {
        p50: 10,
        p95: 20,
        p99: 30,
    });
    let encoded = serde_json::to_string(&report).unwrap();
    assert_eq!(
        serde_json::from_str::<BenchmarkReport>(&encoded).unwrap(),
        report
    );

    let result = compare_to_baseline(
        &report,
        None,
        None,
        NaiveDate::from_ymd_opt(2026, 7, 22).unwrap(),
    )
    .unwrap();
    assert_eq!(result.outcome, ComparisonOutcome::PendingBaseline);
}

fn fixture_environment() -> EnvironmentFingerprint {
    EnvironmentFingerprint {
        os: "linux".into(),
        architecture: "x86_64".into(),
        rustc: "rustc 1.97.1".into(),
        build_profile: "release".into(),
        cpu_model: "fixture CPU".into(),
        logical_cpus: 8,
    }
}

fn fixture_report(percentiles_ns: PercentilesNs) -> BenchmarkReport {
    BenchmarkReport {
        schema_version: BASELINE_SCHEMA_VERSION,
        fixture_version: FIXTURE_VERSION.into(),
        target: target_specs()[1].clone(),
        environment: fixture_environment(),
        sample_count: 100,
        percentiles_ns,
    }
}

fn fixture_baseline(percentiles_ns: PercentilesNs) -> BenchmarkBaseline {
    BenchmarkBaseline {
        report: fixture_report(percentiles_ns),
        approval: BaselineApproval {
            reviewer: "performance-reviewer".into(),
            approved_at: "2026-07-22".into(),
            source_revision: "fixture-revision".into(),
        },
    }
}
